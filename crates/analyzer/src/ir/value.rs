use crate::ir::bigint::{gen_mask, inv, select};
use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{FromPrimitive, ToPrimitive};
use std::fmt;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Value {
    pub payload: BigUint,
    pub mask_x: BigUint,
    pub mask_z: BigUint,
    pub signed: bool,
    pub width: usize,
}

impl Value {
    pub fn new(payload: BigUint, width: usize, signed: bool) -> Value {
        Value {
            payload,
            mask_x: BigUint::from(0u32),
            mask_z: BigUint::from(0u32),
            signed,
            width,
        }
    }

    pub fn new_x(width: usize, signed: bool) -> Value {
        let mask_x = gen_mask(width);
        Value {
            payload: BigUint::from(0u32),
            mask_x,
            mask_z: BigUint::from(0u32),
            signed,
            width,
        }
    }

    pub fn new_z(mask_z: BigUint, width: usize, signed: bool) -> Value {
        Value {
            payload: BigUint::from(0u32),
            mask_x: BigUint::from(0u32),
            mask_z,
            signed,
            width,
        }
    }

    pub fn is_x(&self) -> bool {
        self.mask_x != BigUint::from(0u32)
    }

    pub fn is_z(&self) -> bool {
        self.mask_z != BigUint::from(0u32)
    }

    pub fn to_bigint(&self) -> BigInt {
        if self.signed {
            let (sign, payload) = if self.payload.bit(self.width as u64 - 1) {
                let payload = inv(self.payload.clone(), self.width) + BigUint::from(1u32);
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
        self.payload.to_usize().unwrap()
    }

    pub fn select(&self, beg: Value, end: Value) -> Value {
        let beg = beg.payload.to_usize().unwrap();
        let end = end.payload.to_usize().unwrap();
        let width = beg.saturating_sub(end) + 1;
        let payload = select(self.payload.clone(), beg, end);
        let mask_x = select(self.mask_x.clone(), beg, end);
        let mask_z = select(self.mask_z.clone(), beg, end);

        Value {
            payload,
            mask_x,
            mask_z,
            signed: false,
            width,
        }
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value {
            payload: BigUint::from_u32(value).unwrap(),
            mask_x: 0u32.into(),
            mask_z: 0u32.into(),
            signed: false,
            width: 32,
        }
    }
}

impl fmt::LowerHex for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Display;

        let len = self.width.div_ceil(4);
        let payload = format!("{:01$x}", self.payload, len);
        let mask_x = format!("{:01$x}", self.mask_x, len);
        let mask_z = format!("{:01$x}", self.mask_z, len);

        let payload: Vec<_> = payload.chars().collect();
        let mask_x: Vec<_> = mask_x.chars().collect();
        let mask_z: Vec<_> = mask_z.chars().collect();

        let mut ret = String::new();
        if len == 0 {
            if mask_x[0] != '0' {
                ret.push_str("all x");
            } else if mask_z[0] != '0' {
                ret.push_str("all z");
            } else if payload[0] != '0' {
                ret.push_str("all 1");
            } else {
                ret.push_str("all 0");
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

        let mut x03 = Value::new(BigUint::from(0x123u32), 10, false);
        x03.mask_x = BigUint::from(0x101u32);

        assert_eq!(&format!("{:x}", x03), "x2x");

        let mut x04 = Value::new(BigUint::from(0x345u32), 10, false);
        x04.mask_z = BigUint::from(0x101u32);

        assert_eq!(&format!("{:x}", x04), "z4z");

        let mut x05 = Value::new(BigUint::from(0x345u32), 10, false);
        x05.mask_x = BigUint::from(0x003u32);
        x05.mask_z = BigUint::from(0x010u32);

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
}
