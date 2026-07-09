use crate::{Result, bail, sys};
use smallvec::SmallVec;

/// Value crossing the host boundary: parameters, port values, method
/// arguments and returns.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Value {
    /// LSB-first 64-bit words holding `width` bits; excess high bits are zero.
    ///
    /// `mask_xz` is the parallel four-state mask (always the same length as
    /// `words`): a set bit marks that payload bit as X (payload 0) or Z
    /// (payload 1). It is all-zero for a two-state value; it only carries X/Z
    /// under a four-state simulation ([`crate::SimCtx::is_4state`]).
    Bits {
        words: SmallVec<[u64; 2]>,
        mask_xz: SmallVec<[u64; 2]>,
        width: u32,
    },
    Str(String),
    Unit,
}

pub(crate) fn words_for(width: u32) -> usize {
    (width as usize).div_ceil(64).max(1)
}

fn mask_top_word(words: &mut [u64], width: u32) {
    if width == 0 {
        if let Some(last) = words.last_mut() {
            *last = 0;
        }
        return;
    }
    let rem = width % 64;
    if rem != 0
        && let Some(last) = words.last_mut()
    {
        *last &= u64::MAX >> (64 - rem);
    }
}

impl Value {
    pub fn unit() -> Self {
        Value::Unit
    }

    /// Bits of exactly `width` holding `v`: masked when `width` is below
    /// 64, zero-extended when above.
    pub fn from_u64(v: u64, width: u32) -> Self {
        let mut words = SmallVec::new();
        words.push(v);
        words.resize(words_for(width), 0);
        mask_top_word(&mut words, width);
        let mask_xz = SmallVec::from_elem(0, words.len());
        Value::Bits {
            words,
            mask_xz,
            width,
        }
    }

    /// Bits with a four-state `mask_xz` parallel to `words`. Both are resized to
    /// `width` and their excess high bits cleared.
    pub fn from_bits(
        mut words: SmallVec<[u64; 2]>,
        mut mask_xz: SmallVec<[u64; 2]>,
        width: u32,
    ) -> Self {
        let n = words_for(width);
        words.resize(n, 0);
        mask_xz.resize(n, 0);
        mask_top_word(&mut words, width);
        mask_top_word(&mut mask_xz, width);
        Value::Bits {
            words,
            mask_xz,
            width,
        }
    }

    /// The four-state mask: a set bit marks an X or Z payload bit. Empty for
    /// non-bits values.
    pub fn mask_xz(&self) -> &[u64] {
        match self {
            Value::Bits { mask_xz, .. } => mask_xz,
            _ => &[],
        }
    }

    /// True if any bit is X or Z.
    pub fn has_unknown(&self) -> bool {
        self.mask_xz().iter().any(|w| *w != 0)
    }

    /// True if any bit is X (mask_xz set, payload clear).
    pub fn has_x(&self) -> bool {
        match self {
            Value::Bits { words, mask_xz, .. } => {
                mask_xz.iter().zip(words).any(|(m, w)| m & !w != 0)
            }
            _ => false,
        }
    }

    /// True if any bit is Z (mask_xz set, payload set).
    pub fn has_z(&self) -> bool {
        match self {
            Value::Bits { words, mask_xz, .. } => {
                mask_xz.iter().zip(words).any(|(m, w)| m & w != 0)
            }
            _ => false,
        }
    }

    /// State of bit `i`: `Some(false)` = X, `Some(true)` = Z, `None` = known
    /// (0 or 1). Out-of-range or non-bits bits are `None`.
    pub fn unknown_at(&self, i: u32) -> Option<bool> {
        let Value::Bits { words, mask_xz, .. } = self else {
            return None;
        };
        let (w, b) = (i as usize / 64, i as usize % 64);
        if mask_xz.get(w).is_some_and(|m| m >> b & 1 != 0) {
            Some(words.get(w).is_some_and(|p| p >> b & 1 != 0))
        } else {
            None
        }
    }

    pub fn as_u64(&self) -> Result<u64> {
        match self {
            Value::Bits { words, width, .. } => {
                if *width > 64 {
                    bail!("value is {width} bits wide, does not fit in u64");
                }
                Ok(words.first().copied().unwrap_or(0))
            }
            Value::Str(_) => bail!("value is a string, not bits"),
            Value::Unit => bail!("value is unit, not bits"),
        }
    }

    /// Bits reinterpreted as a signed integer: the bit at `width - 1` is
    /// sign-extended to 64 bits.
    pub fn as_i64(&self) -> Result<i64> {
        match self {
            Value::Bits { words, width, .. } => {
                if *width == 0 || *width > 64 {
                    bail!("value is {width} bits wide, does not fit in i64");
                }
                let shift = 64 - *width;
                let raw = words.first().copied().unwrap_or(0);
                Ok(((raw << shift) as i64) >> shift)
            }
            Value::Str(_) => bail!("value is a string, not bits"),
            Value::Unit => bail!("value is unit, not bits"),
        }
    }

    /// True if any bit is set. Strings and unit are false.
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bits { words, .. } => words.iter().any(|w| *w != 0),
            _ => false,
        }
    }

    /// True if any bit is set; errors when the value is not bits.
    pub fn as_bool_strict(&self) -> Result<bool> {
        match self {
            Value::Bits { words, .. } => Ok(words.iter().any(|w| *w != 0)),
            Value::Str(_) => bail!("value is a string, not bits"),
            Value::Unit => bail!("value is unit, not bits"),
        }
    }

    pub fn as_str(&self) -> Result<&str> {
        match self {
            Value::Str(s) => Ok(s),
            _ => bail!("value is not a string"),
        }
    }

    pub fn width(&self) -> u32 {
        match self {
            Value::Bits { width, .. } => *width,
            _ => 0,
        }
    }

    /// Copies a boundary value into an owned `Value`.
    ///
    /// # Safety
    /// The pointers inside `v` must be valid for its declared lengths.
    pub(crate) unsafe fn from_vrl(v: &sys::VrlValue) -> Self {
        match v.kind {
            sys::VRL_VALUE_BITS => {
                let read = |ptr: *const u64| -> SmallVec<[u64; 2]> {
                    if v.nwords == 0 || ptr.is_null() {
                        SmallVec::from_elem(0, v.nwords)
                    } else {
                        unsafe { std::slice::from_raw_parts(ptr, v.nwords) }.into()
                    }
                };
                Value::from_bits(read(v.words), read(v.mask_xz), v.width)
            }
            sys::VRL_VALUE_STRING => Value::Str(unsafe { v.str_.as_str() }.to_string()),
            _ => Value::Unit,
        }
    }

    /// Resizes the payload to exactly `width`, zero-extending or truncating.
    pub(crate) fn to_port_words(&self, width: u32) -> Result<SmallVec<[u64; 2]>> {
        let Value::Bits { words, .. } = self else {
            bail!("cannot write a non-bits value to a port");
        };
        let mut out: SmallVec<[u64; 2]> = words.clone();
        out.resize(words_for(width), 0);
        mask_top_word(&mut out, width);
        Ok(out)
    }

    /// Resizes the four-state mask to exactly `width` (parallel to
    /// [`Self::to_port_words`]).
    pub(crate) fn to_port_mask_xz(&self, width: u32) -> Result<SmallVec<[u64; 2]>> {
        let Value::Bits { mask_xz, .. } = self else {
            bail!("cannot write a non-bits value to a port");
        };
        let mut out: SmallVec<[u64; 2]> = mask_xz.clone();
        out.resize(words_for(width), 0);
        mask_top_word(&mut out, width);
        Ok(out)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::from_u64(v as u64, 1)
    }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self {
        Value::from_u64(v as u64, 8)
    }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self {
        Value::from_u64(v as u64, 16)
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::from_u64(v as u64, 32)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::from_u64(v, 64)
    }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self {
        Value::from_u64(v as u8 as u64, 8)
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self {
        Value::from_u64(v as u16 as u64, 16)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::from_u64(v as u32 as u64, 32)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::from_u64(v as u64, 64)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Str(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Str(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_u64_masks_and_extends() {
        let zero_width = Value::from_u64(1, 0);
        assert!(!zero_width.as_bool());
        assert_eq!(zero_width.as_u64().unwrap(), 0);

        let wide = Value::from_u64(5, 128);
        assert_eq!(wide.width(), 128);
        let Value::Bits { words, .. } = &wide else {
            unreachable!()
        };
        assert_eq!(words.as_slice(), &[5, 0]);

        assert_eq!(Value::from_u64(0xFF, 4).as_u64().unwrap(), 0xF);
    }

    #[test]
    fn signed_from_impls_store_twos_complement() {
        assert_eq!(Value::from(-1i8), Value::from_u64(0xFF, 8));
        assert_eq!(Value::from(-1i16), Value::from_u64(0xFFFF, 16));
        assert_eq!(Value::from(-2i32), Value::from_u64(0xFFFF_FFFE, 32));
        assert_eq!(Value::from(-1i64), Value::from_u64(u64::MAX, 64));
        assert_eq!(Value::from(127i8), Value::from_u64(0x7F, 8));
    }

    #[test]
    fn as_i64_sign_extends_by_width() {
        assert_eq!(Value::from_u64(0xFFFF_FFFF, 32).as_i64().unwrap(), -1);
        assert_eq!(Value::from_u64(0xF, 4).as_i64().unwrap(), -1);
        assert_eq!(Value::from_u64(0x7, 4).as_i64().unwrap(), 7);
        assert_eq!(Value::from_u64(u64::MAX, 64).as_i64().unwrap(), -1);
        assert_eq!(
            Value::from_u64(0xFFFF_FFFF, 64).as_i64().unwrap(),
            0xFFFF_FFFF
        );

        assert!(Value::from_u64(0, 0).as_i64().is_err());
        assert!(Value::from_u64(0, 65).as_i64().is_err());
        assert!(Value::Str("x".into()).as_i64().is_err());
        assert!(Value::Unit.as_i64().is_err());
    }

    #[test]
    fn as_bool_strict_rejects_non_bits() {
        assert!(!Value::from_u64(0, 1).as_bool_strict().unwrap());
        assert!(Value::from_u64(1, 1).as_bool_strict().unwrap());
        assert!(Value::Str("x".into()).as_bool_strict().is_err());
        assert!(Value::Unit.as_bool_strict().is_err());
    }
}
