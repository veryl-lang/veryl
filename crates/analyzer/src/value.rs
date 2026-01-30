use crate::ir::{Shape, Type, TypeKind};
use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{FromPrimitive, Num, ToPrimitive};
use std::fmt;
use veryl_parser::veryl_grammar_trait as syntax_tree;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Value {
    pub payload: BigUint,
    pub mask_xz: BigUint,
    pub signed: bool,
    pub width: usize,
}

impl Value {
    pub fn new(payload: BigUint, width: usize, signed: bool) -> Value {
        Value {
            payload,
            mask_xz: BigUint::from(0u32),
            signed,
            width,
        }
    }

    pub fn new_x(width: usize, signed: bool) -> Value {
        Value {
            payload: BigUint::from(0u32),
            mask_xz: gen_mask(width),
            signed,
            width,
        }
    }

    pub fn new_z(width: usize, signed: bool) -> Value {
        Value {
            payload: gen_mask(width),
            mask_xz: gen_mask(width),
            signed,
            width,
        }
    }

    pub fn is_xz(&self) -> bool {
        self.mask_xz != BigUint::from(0u32)
    }

    pub fn to_bigint(&self) -> BigInt {
        if self.signed {
            let (sign, payload) = if self.payload.bit(self.width as u64 - 1) {
                let payload = inv(&self.payload, self.width) + BigUint::from(1u32);
                (Sign::Minus, payload)
            } else {
                (Sign::Plus, self.payload.clone())
            };
            BigInt::from_biguint(sign, payload)
        } else {
            BigInt::from_biguint(Sign::Plus, self.payload.clone())
        }
    }

    pub fn to_usize(&self) -> usize {
        self.payload.to_usize().unwrap_or(0)
    }

    pub fn select(&self, beg: usize, end: usize) -> Value {
        let width = beg.saturating_sub(end) + 1;
        let payload = select(self.payload.clone(), beg, end);
        let mask_xz = select(self.mask_xz.clone(), beg, end);

        Value {
            payload,
            mask_xz,
            signed: false,
            width,
        }
    }

    pub fn to_ir_type(&self) -> Type {
        let kind = if self.is_xz() {
            TypeKind::Logic
        } else {
            TypeKind::Bit
        };
        Type {
            kind,
            signed: self.signed,
            width: Shape::new(vec![Some(self.width)]),
            ..Default::default()
        }
    }

    pub fn trunc(&mut self, width: usize) {
        if self.width > width {
            let mask = gen_mask(width);
            self.payload &= &mask;
            self.mask_xz &= &mask;
        }
        self.width = width;
    }

    pub fn expand(&mut self, width: usize) {
        self.width = width;
    }

    pub fn concat(mut self, x: &Value) -> Value {
        self.payload <<= x.width;
        self.mask_xz <<= x.width;

        self.payload |= &x.payload;
        self.mask_xz |= &x.mask_xz;

        self.width += x.width;

        self
    }

    pub fn assign(&mut self, mut x: Value, beg: usize, end: usize) {
        x.payload <<= end;
        x.mask_xz <<= end;

        let mask = gen_mask_range(beg, end);
        let inv_mask = inv(&mask, self.width);

        self.payload = (&self.payload & &inv_mask) | (x.payload & &mask);
        self.mask_xz = (&self.mask_xz & &inv_mask) | (x.mask_xz & &mask);
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value {
            payload: BigUint::from_u32(value).unwrap(),
            mask_xz: 0u32.into(),
            signed: false,
            width: 32,
        }
    }
}

impl From<&syntax_tree::Based> for Value {
    fn from(value: &syntax_tree::Based) -> Self {
        let x = value.based_token.to_string().replace('_', "");
        let (width, rest) = x.split_once('\'').unwrap();

        let signed = &rest[0..1] == "s";
        let rest = if signed { &rest[1..] } else { rest };
        let (base, value) = rest.split_at(1);
        let (radix, all1_char) = match base {
            "b" => (2, '1'),
            "o" => (8, '7'),
            "d" => (10, '0'),
            "h" => (16, 'f'),
            _ => unreachable!(),
        };

        let payload = value.replace(['x', 'X', 'z', 'Z'], "0");
        let mask_x: String = value
            .chars()
            .map(|x| if x == 'x' || x == 'X' { all1_char } else { '0' })
            .collect();
        let mask_z: String = value
            .chars()
            .map(|x| if x == 'z' || x == 'Z' { all1_char } else { '0' })
            .collect();

        let payload = BigUint::from_str_radix(&payload, radix).unwrap_or(BigUint::from(0u32));
        let mask_x = BigUint::from_str_radix(&mask_x, radix).unwrap_or(BigUint::from(0u32));
        let mask_z = BigUint::from_str_radix(&mask_z, radix).unwrap_or(BigUint::from(0u32));

        let actual_width = payload.bits().max(mask_x.bits()).max(mask_z.bits()) as usize;

        let width = if let Ok(x) = str::parse::<usize>(width) {
            x
        } else {
            actual_width
        };

        let mask_xz = &mask_x | &mask_z;
        let payload = (payload & inv(&mask_xz, actual_width)) | mask_z;

        Value {
            payload,
            mask_xz,
            width,
            signed,
        }
    }
}

impl From<&syntax_tree::BaseLess> for Value {
    fn from(value: &syntax_tree::BaseLess) -> Self {
        let x = value.base_less_token.to_string().replace('_', "");
        let x = str::parse::<BigUint>(&x).unwrap();
        Value::new(x, 32, true)
    }
}

impl From<&syntax_tree::AllBit> for Value {
    fn from(value: &syntax_tree::AllBit) -> Self {
        fn zero() -> BigUint {
            BigUint::from(0u32)
        }

        fn one() -> BigUint {
            BigUint::from(1u32)
        }

        let x = value.all_bit_token.to_string();
        let (width, rest) = x.split_once('\'').unwrap();
        let (payload, mask_xz, width) = if width.is_empty() {
            let width = 0;
            match rest {
                "0" => (zero(), zero(), width),
                "1" => (one(), zero(), width),
                "x" | "X" => (zero(), one(), width),
                "z" | "Z" => (one(), one(), width),
                _ => unreachable!(),
            }
        } else {
            let width = str::parse::<usize>(width).unwrap();
            let mask = gen_mask(width);
            match rest {
                "0" => (zero(), zero(), width),
                "1" => (mask, zero(), width),
                "x" | "X" => (zero(), mask, width),
                "z" | "Z" => (mask.clone(), mask, width),
                _ => unreachable!(),
            }
        };

        Value {
            payload,
            mask_xz,
            width,
            signed: false,
        }
    }
}

impl From<&syntax_tree::FixedPoint> for Value {
    fn from(value: &syntax_tree::FixedPoint) -> Self {
        let x = value.fixed_point_token.to_string();
        let (payload, mask_xz) = if let Ok(value) = x.parse::<f64>() {
            (BigUint::from(value.to_bits()), BigUint::from(0u32))
        } else {
            (BigUint::from(0u32), gen_mask(64))
        };
        Value {
            payload,
            mask_xz,
            width: 64,
            signed: false,
        }
    }
}

impl From<&syntax_tree::Exponent> for Value {
    fn from(value: &syntax_tree::Exponent) -> Self {
        let x = value.exponent_token.to_string();
        let (payload, mask_xz) = if let Ok(value) = x.parse::<f64>() {
            (BigUint::from(value.to_bits()), BigUint::from(0u32))
        } else {
            (BigUint::from(0u32), gen_mask(64))
        };
        Value {
            payload,
            mask_xz,
            width: 64,
            signed: false,
        }
    }
}

impl fmt::LowerHex for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Display;

        let len = self.width.div_ceil(4);

        let mask_width = if self.width == 0 { 1 } else { self.width };
        let mask_x = &self.mask_xz & inv(&self.payload, mask_width);
        let mask_z = &self.mask_xz & &self.payload;

        let payload = format!("{:01$x}", self.payload, len);
        let mask_x = format!("{:01$x}", mask_x, len);
        let mask_z = format!("{:01$x}", mask_z, len);

        let payload: Vec<_> = payload.chars().collect();
        let mask_x: Vec<_> = mask_x.chars().collect();
        let mask_z: Vec<_> = mask_z.chars().collect();

        let mut ret = String::new();
        if len == 0 {
            if mask_x[0] != '0' {
                ret.push_str("'x");
            } else if mask_z[0] != '0' {
                ret.push_str("'z");
            } else if payload[0] != '0' {
                ret.push_str("'1");
            } else {
                ret.push_str("'0");
            }
        } else {
            for i in 0..len {
                if mask_x[i] != '0' {
                    ret.push('x');
                } else if mask_z[i] != '0' {
                    ret.push('z');
                } else {
                    ret.push(payload[i]);
                }
            }
        }

        ret.fmt(f)
    }
}

pub fn gen_mask(width: usize) -> BigUint {
    let mut ret = Vec::new();
    let mut remaining = width;
    loop {
        if remaining >= 32 {
            ret.push(0xffffffff);
            remaining -= 32;
        } else {
            ret.push((1u32 << remaining) - 1);
            break;
        }
    }
    BigUint::from_slice(&ret)
}

pub fn gen_mask_range(beg: usize, end: usize) -> BigUint {
    let width = beg + 1;
    let beg = gen_mask(width);
    let end = gen_mask(end);
    let end = inv(&end, width);
    beg & end
}

pub fn inv(value: &BigUint, width: usize) -> BigUint {
    let mut ret = Vec::new();
    let mut remaining = width;
    let values = value.to_u32_digits();
    let mut i = 0;
    loop {
        if remaining >= 32 {
            let value = values.get(i).unwrap_or(&0);
            ret.push(!value);
            remaining -= 32;
            i += 1;
        } else {
            let value = values.get(i).unwrap_or(&0);
            let mask = (1u32 << remaining) - 1;
            ret.push((!value) & mask);
            break;
        }
    }
    BigUint::from_slice(&ret)
}

pub fn to_biguint(value: BigInt, width: usize) -> BigUint {
    if value.sign() == Sign::Plus {
        let value = value.magnitude().clone();
        if value.bits() as usize > width {
            value & gen_mask(width)
        } else {
            value
        }
    } else {
        let payload = value.magnitude().clone();
        let mask = gen_mask(width);
        (inv(&payload, width) + BigUint::from(1u32)) & mask
    }
}

pub fn select(value: BigUint, beg: usize, end: usize) -> BigUint {
    let ret = value >> end;
    let mask = gen_mask(beg.saturating_sub(end) + 1);
    ret & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_format() {
        let x00 = Value::new(BigUint::from(0x000u32), 10, false);
        let x01 = Value::new(BigUint::from(0x01au32), 10, false);
        let x02 = Value::new(BigUint::from(0x3ffu32), 10, false);

        assert_eq!(&format!("{:x}", x00), "000");
        assert_eq!(&format!("{:x}", x01), "01a");
        assert_eq!(&format!("{:x}", x02), "3ff");

        let mut x03 = Value::new(BigUint::from(0x020u32), 10, false);
        x03.mask_xz = BigUint::from(0x101u32);

        assert_eq!(&format!("{:x}", x03), "x2x");

        let mut x04 = Value::new(BigUint::from(0x345u32), 10, false);
        x04.mask_xz = BigUint::from(0x101u32);

        assert_eq!(&format!("{:x}", x04), "z4z");

        let mut x05 = Value::new(BigUint::from(0x3f0u32), 10, false);
        x05.mask_xz = BigUint::from(0x0ffu32);

        assert_eq!(&format!("{:x}", x05), "3zx");
    }

    #[test]
    fn value_bigint() {
        let x0 = Value::new(BigUint::from(0x000u32), 10, false);
        let x1 = Value::new(BigUint::from(0x01au32), 10, false);
        let x2 = Value::new(BigUint::from(0x3ffu32), 10, false);
        let x3 = Value::new(BigUint::from(0x000u32), 10, true);
        let x4 = Value::new(BigUint::from(0x01au32), 10, true);
        let x5 = Value::new(BigUint::from(0x3ffu32), 10, true);

        let x0 = x0.to_bigint();
        let x1 = x1.to_bigint();
        let x2 = x2.to_bigint();
        let x3 = x3.to_bigint();
        let x4 = x4.to_bigint();
        let x5 = x5.to_bigint();

        assert_eq!(&format!("{:x}", x0), "0");
        assert_eq!(&format!("{:x}", x1), "1a");
        assert_eq!(&format!("{:x}", x2), "3ff");
        assert_eq!(&format!("{:x}", x3), "0");
        assert_eq!(&format!("{:x}", x4), "1a");
        assert_eq!(&format!("{:x}", x5), "-1");
    }

    #[test]
    fn test_mask() {
        assert_eq!(format!("{:x}", gen_mask(1)), "1");
        assert_eq!(format!("{:x}", gen_mask(2)), "3");
        assert_eq!(format!("{:x}", gen_mask(3)), "7");
        assert_eq!(format!("{:x}", gen_mask(10)), "3ff");
        assert_eq!(format!("{:x}", gen_mask(59)), "7ffffffffffffff");
        assert_eq!(format!("{:x}", gen_mask(90)), "3ffffffffffffffffffffff");
    }

    #[test]
    fn test_mask_range() {
        assert_eq!(format!("{:x}", gen_mask_range(1, 0)), "3");
        assert_eq!(format!("{:x}", gen_mask_range(10, 2)), "7fc");
        assert_eq!(
            format!("{:x}", gen_mask_range(100, 10)),
            "1ffffffffffffffffffffffc00"
        );
    }

    #[test]
    fn test_inv() {
        assert_eq!(format!("{:x}", inv(&BigUint::from(1u32), 1)), "0");
        assert_eq!(format!("{:x}", inv(&BigUint::from(1u32), 2)), "2");
        assert_eq!(format!("{:x}", inv(&BigUint::from(1u32), 3)), "6");
        assert_eq!(format!("{:x}", inv(&BigUint::from(1u32), 10)), "3fe");
        assert_eq!(
            format!("{:x}", inv(&BigUint::from(1u32), 59)),
            "7fffffffffffffe"
        );
        assert_eq!(
            format!("{:x}", inv(&BigUint::from(1u32), 90)),
            "3fffffffffffffffffffffe"
        );
    }

    #[test]
    fn test_to_biguint() {
        assert_eq!(format!("{:x}", to_biguint(BigInt::from(1), 10)), "1");
        assert_eq!(format!("{:x}", to_biguint(BigInt::from(2), 10)), "2");
        assert_eq!(format!("{:x}", to_biguint(BigInt::from(3), 10)), "3");
        assert_eq!(format!("{:x}", to_biguint(BigInt::from(-1), 10)), "3ff");
        assert_eq!(format!("{:x}", to_biguint(BigInt::from(-2), 10)), "3fe");
        assert_eq!(format!("{:x}", to_biguint(BigInt::from(-3), 10)), "3fd");
    }

    #[test]
    fn test_select() {
        assert_eq!(format!("{:x}", select(BigUint::from(0xffu32), 0, 0)), "1");
        assert_eq!(format!("{:x}", select(BigUint::from(0xffu32), 1, 0)), "3");
        assert_eq!(format!("{:x}", select(BigUint::from(0xffu32), 3, 0)), "f");
        assert_eq!(format!("{:x}", select(BigUint::from(0xf0u32), 3, 0)), "0");
        assert_eq!(format!("{:x}", select(BigUint::from(0xf0u32), 4, 1)), "8");
        assert_eq!(format!("{:x}", select(BigUint::from(0xf0u32), 7, 2)), "3c");
    }
}
