//! Runtime arguments given to `veryl test` with `--runtime-arg`.

use crate::sys;
use num_bigint::{BigInt, BigUint, Sign};
use std::fmt;

/// A runtime argument as given on the command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeArg {
    pub name: String,
    pub value: ArgValue,
}

/// A runtime argument's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue {
    /// `--runtime-arg=NAME`
    Flag,
    /// `--runtime-arg=NAME:VALUE`
    Text(String),
}

impl ArgValue {
    pub fn is_flag(&self) -> bool {
        matches!(self, ArgValue::Flag)
    }

    /// The text of the value; `None` for a flag.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ArgValue::Flag => None,
            ArgValue::Text(x) => Some(x),
        }
    }

    fn text(&self) -> Result<&str, ArgError> {
        self.as_str().ok_or(ArgError::Flag)
    }

    fn int(&self) -> Result<sys::RuntimeArgInt, ArgError> {
        let text = self.text()?;
        sys::parse_runtime_arg_int(text).ok_or_else(|| ArgError::NotInteger(text.to_string()))
    }
}

/// Why a runtime argument's value cannot be read as a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgError {
    /// The argument was given as a flag, without a value.
    Flag,
    /// The value is not an integer.
    NotInteger(String),
    /// The value is an integer that does not fit the type.
    OutOfRange { value: String, r#type: &'static str },
    /// A conversion defined outside the SDK rejected the value.
    Invalid(String),
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgError::Flag => write!(f, "given as a flag, without a value"),
            ArgError::NotInteger(x) => write!(f, "value `{x}` is not an integer"),
            ArgError::OutOfRange { value, r#type } => {
                write!(f, "value `{value}` does not fit `{type}`")
            }
            ArgError::Invalid(x) => x.fmt(f),
        }
    }
}

impl std::error::Error for ArgError {}

impl TryFrom<&ArgValue> for String {
    type Error = ArgError;

    fn try_from(value: &ArgValue) -> Result<Self, Self::Error> {
        value.text().map(|x| x.to_string())
    }
}

/// The magnitude of an integer read from text, if it fits 128 bits.
fn magnitude_u128(int: &sys::RuntimeArgInt) -> Option<u128> {
    match int.magnitude.as_slice() {
        [] => Some(0),
        [lo] => Some(*lo as u128),
        [lo, hi] => Some((*hi as u128) << 64 | *lo as u128),
        _ => None,
    }
}

fn to_i128(int: &sys::RuntimeArgInt) -> Option<i128> {
    let mag = magnitude_u128(int)?;
    if int.negative {
        if mag == 1u128 << 127 {
            Some(i128::MIN)
        } else {
            i128::try_from(mag).ok().map(|x| -x)
        }
    } else {
        i128::try_from(mag).ok()
    }
}

fn to_u128(int: &sys::RuntimeArgInt) -> Option<u128> {
    if int.negative {
        None
    } else {
        magnitude_u128(int)
    }
}

macro_rules! impl_try_from_int {
    ($via:ident: $($ty:ty),*) => {
        $(
            impl TryFrom<&ArgValue> for $ty {
                type Error = ArgError;

                fn try_from(value: &ArgValue) -> Result<Self, Self::Error> {
                    let int = value.int()?;
                    $via(&int)
                        .and_then(|x| <$ty>::try_from(x).ok())
                        .ok_or_else(|| ArgError::OutOfRange {
                            value: value.as_str().unwrap_or_default().to_string(),
                            r#type: stringify!($ty),
                        })
                }
            }
        )*
    };
}

impl_try_from_int!(to_i128: i8, i16, i32, i64, i128, isize);
impl_try_from_int!(to_u128: u8, u16, u32, u64, u128, usize);

fn words_to_bytes(words: &[u64]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

impl TryFrom<&ArgValue> for BigUint {
    type Error = ArgError;

    fn try_from(value: &ArgValue) -> Result<Self, Self::Error> {
        let int = value.int()?;
        if int.negative {
            return Err(ArgError::OutOfRange {
                value: value.as_str().unwrap_or_default().to_string(),
                r#type: "BigUint",
            });
        }
        Ok(BigUint::from_bytes_le(&words_to_bytes(&int.magnitude)))
    }
}

impl TryFrom<&ArgValue> for BigInt {
    type Error = ArgError;

    fn try_from(value: &ArgValue) -> Result<Self, Self::Error> {
        let int = value.int()?;
        let sign = if int.negative {
            Sign::Minus
        } else {
            Sign::Plus
        };
        Ok(BigInt::from_bytes_le(sign, &words_to_bytes(&int.magnitude)))
    }
}

/// The argument that wins for `name`: the last one given.
pub(crate) fn find<'a>(args: &'a [RuntimeArg], name: &str) -> Option<&'a ArgValue> {
    args.iter().rev().find(|x| x.name == name).map(|x| &x.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(x: &str) -> ArgValue {
        ArgValue::Text(x.to_string())
    }

    #[test]
    fn convert_integers() {
        assert_eq!(u32::try_from(&text("4")), Ok(4));
        assert_eq!(i8::try_from(&text("-128")), Ok(-128));
        assert_eq!(
            u128::try_from(&text("0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffff")),
            Ok(!0)
        );
        assert_eq!(
            i128::try_from(&text("-0x8000_0000_0000_0000_0000_0000_0000_0000")),
            Ok(i128::MIN)
        );
        assert_eq!(u8::try_from(&text("0b1010")), Ok(10));

        assert!(matches!(
            u8::try_from(&text("256")),
            Err(ArgError::OutOfRange { .. })
        ));
        assert!(matches!(
            u8::try_from(&text("-1")),
            Err(ArgError::OutOfRange { .. })
        ));
        assert!(matches!(
            i8::try_from(&text("128")),
            Err(ArgError::OutOfRange { .. })
        ));
        assert!(matches!(
            u32::try_from(&text("abc")),
            Err(ArgError::NotInteger(_))
        ));
        assert_eq!(u32::try_from(&ArgValue::Flag), Err(ArgError::Flag));
    }

    #[test]
    fn convert_big_integers() {
        let big = "0x1_0000_0000_0000_0000_0000_0000_0000_0000";
        assert_eq!(BigUint::try_from(&text(big)), Ok(BigUint::from(1u8) << 128));
        assert_eq!(BigInt::try_from(&text("-5")), Ok(BigInt::from(-5)));
        assert!(BigUint::try_from(&text("-5")).is_err());
    }

    #[test]
    fn convert_string() {
        assert_eq!(String::try_from(&text("a:b")), Ok("a:b".to_string()));
        assert_eq!(String::try_from(&ArgValue::Flag), Err(ArgError::Flag));
    }

    #[test]
    fn last_one_wins() {
        let args = vec![
            RuntimeArg {
                name: "A".into(),
                value: text("1"),
            },
            RuntimeArg {
                name: "A".into(),
                value: ArgValue::Flag,
            },
        ];
        assert_eq!(find(&args, "A"), Some(&ArgValue::Flag));
        assert_eq!(find(&args, "B"), None);
    }
}
