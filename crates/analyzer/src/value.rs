use crate::ir::{Shape, Type, TypeKind};
use crate::{BigInt, BigUint, HashMap, Sign};
use num_traits::{Num, One, ToPrimitive, Zero, one, zero};
use std::borrow::Cow;
use std::{fmt, str};
use veryl_parser::veryl_grammar_trait as syntax_tree;

// repr(C) is necessary for pointer access from Cranelift
#[repr(C)]
#[derive(Clone, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValueU64 {
    pub payload: u64,
    pub mask_xz: u64,
    pub width: u32,
    pub signed: bool,
}

impl ValueU64 {
    pub fn new(payload: u64, width: usize, signed: bool) -> Self {
        Self {
            payload,
            mask_xz: 0,
            width: width as u32,
            signed,
        }
    }

    pub fn new_x(width: usize, signed: bool) -> Self {
        let mask = Self::gen_mask(width);
        Self {
            payload: 0,
            mask_xz: mask,
            width: width as u32,
            signed,
        }
    }

    pub fn new_z(width: usize, signed: bool) -> Self {
        let mask = Self::gen_mask(width);
        Self {
            payload: mask,
            mask_xz: mask,
            width: width as u32,
            signed,
        }
    }

    pub fn new_bit_1x(is_one: bool, is_x: bool) -> Self {
        if is_one {
            Self::new(1, 1, false)
        } else if is_x {
            Self::new_x(1, false)
        } else {
            Self::new(0, 1, false)
        }
    }

    pub fn new_bit_0x(is_zero: bool, is_x: bool) -> Self {
        if is_zero {
            Self::new(0, 1, false)
        } else if is_x {
            Self::new_x(1, false)
        } else {
            Self::new(1, 1, false)
        }
    }

    pub fn new_bit_x1(is_x: bool, is_one: bool) -> Self {
        if is_x {
            Self::new_x(1, false)
        } else if is_one {
            Self::new(1, 1, false)
        } else {
            Self::new(0, 1, false)
        }
    }

    pub fn new_bit_x0(is_x: bool, is_zero: bool) -> Self {
        if is_x {
            Self::new_x(1, false)
        } else if is_zero {
            Self::new(0, 1, false)
        } else {
            Self::new(1, 1, false)
        }
    }

    pub fn is_xz(&self) -> bool {
        self.mask_xz != 0
    }

    pub fn gen_mask(width: usize) -> u64 {
        if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        }
    }

    pub fn gen_mask_range(beg: usize, end: usize) -> u64 {
        let width = beg + 1;
        let beg = Self::gen_mask(width);
        let end = !Self::gen_mask(end);
        beg & end
    }

    pub fn trunc(&mut self, width: usize) {
        let mask = Self::gen_mask(width);
        self.payload &= mask;
        self.mask_xz &= mask;
        self.width = width as u32;
    }

    pub fn select(&self, beg: usize, end: usize) -> Self {
        if beg < end {
            Self::default()
        } else {
            let width = beg - end + 1;
            let mask = Self::gen_mask(width);
            let mut ret = self.clone();

            ret.payload >>= end;
            ret.mask_xz >>= end;
            ret.payload &= mask;
            ret.mask_xz &= mask;
            ret.width = width as u32;
            ret.signed = false;

            ret
        }
    }

    pub fn assign(&mut self, mut value: Self, beg: usize, end: usize) {
        value.payload <<= end;
        value.mask_xz <<= end;

        let mask = Self::gen_mask(self.width as usize);
        let mask_range = Self::gen_mask_range(beg, end);
        let inv_mask = mask ^ mask_range;

        self.payload = (self.payload & inv_mask) | (value.payload & mask);
        self.mask_xz = (self.mask_xz & inv_mask) | (value.mask_xz & mask);
    }

    pub fn to_usize(&self) -> Option<usize> {
        if self.mask_xz != 0 {
            None
        } else {
            self.payload.to_usize()
        }
    }

    pub fn to_u32(&self) -> Option<u32> {
        if self.mask_xz != 0 {
            None
        } else {
            self.payload.to_u32()
        }
    }

    pub fn to_u64(&self) -> Option<u64> {
        if self.mask_xz != 0 {
            None
        } else {
            Some(self.payload)
        }
    }

    pub fn to_i64(&self) -> Option<i64> {
        if self.mask_xz != 0 {
            None
        } else if self.signed {
            let mask = Self::gen_mask(self.width as usize);
            let msb = ((self.payload >> (self.width - 1)) & 1) == 1;
            let ret = if msb {
                self.payload | !mask
            } else {
                self.payload
            };
            Some(ret as i64)
        } else {
            self.payload.to_i64()
        }
    }
}

fn gen_hex_string(payload: u64, mask_xz: u64, width: u32) -> String {
    let len = width.div_ceil(4) as usize;

    let first_full_bit_char = match width % 4 {
        0 => 'f',
        1 => '1',
        2 => '3',
        3 => '7',
        _ => unreachable!(),
    };

    let mask_x = mask_xz & !payload;
    let mask_z = mask_xz & payload;

    let payload = format!("{:01$x}", payload, len);
    let mask_x = format!("{:01$x}", mask_x, len);
    let mask_z = format!("{:01$x}", mask_z, len);

    let payload: Vec<_> = payload.chars().collect();
    let mask_x: Vec<_> = mask_x.chars().collect();
    let mask_z: Vec<_> = mask_z.chars().collect();

    let mut ret = String::new();
    for i in 0..len {
        if mask_x[i] != '0' {
            if mask_x[i] == 'f' || (i == 0 && mask_x[i] == first_full_bit_char) {
                ret.push('x');
            } else {
                ret.push('X');
            }
        } else if mask_z[i] != '0' {
            if mask_z[i] == 'f' || (i == 0 && mask_z[i] == first_full_bit_char) {
                ret.push('z');
            } else {
                ret.push('Z');
            }
        } else {
            ret.push(payload[i]);
        }
    }

    ret
}

fn gen_bin_string(payload: u64, mask_xz: u64, width: u32) -> String {
    let len = width as usize;

    let mask_x = mask_xz & !payload;
    let mask_z = mask_xz & payload;

    let payload = format!("{:01$b}", payload, len);
    let mask_x = format!("{:01$b}", mask_x, len);
    let mask_z = format!("{:01$b}", mask_z, len);

    let payload: Vec<_> = payload.chars().collect();
    let mask_x: Vec<_> = mask_x.chars().collect();
    let mask_z: Vec<_> = mask_z.chars().collect();

    let mut ret = String::new();
    for i in 0..len {
        if mask_x[i] != '0' {
            ret.push('x');
        } else if mask_z[i] != '0' {
            ret.push('z');
        } else {
            ret.push(payload[i]);
        }
    }

    ret
}

impl fmt::LowerHex for ValueU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Display;

        let ret = if self.width == 0 {
            if self.mask_xz == 0 {
                if self.payload == 0 {
                    "'0".to_string()
                } else {
                    "'1".to_string()
                }
            } else if self.payload == 0 {
                "'x".to_string()
            } else {
                "'z".to_string()
            }
        } else {
            let ret = gen_hex_string(self.payload, self.mask_xz, self.width);
            let signed = if self.signed { "s" } else { "" };
            format!("{}'{signed}h{ret}", self.width)
        };

        ret.fmt(f)
    }
}

impl fmt::Binary for ValueU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Display;

        let ret = if self.width == 0 {
            if self.mask_xz == 0 {
                if self.payload == 0 {
                    "'0".to_string()
                } else {
                    "'1".to_string()
                }
            } else if self.payload == 0 {
                "'x".to_string()
            } else {
                "'z".to_string()
            }
        } else {
            let ret = gen_bin_string(self.payload, self.mask_xz, self.width);
            let signed = if self.signed { "s" } else { "" };
            format!("{}'{signed}b{ret}", self.width)
        };

        ret.fmt(f)
    }
}

#[derive(Clone, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValueBigUint {
    pub payload: Box<BigUint>,
    pub mask_xz: Box<BigUint>,
    pub width: u32,
    pub signed: bool,
}

impl ValueBigUint {
    pub fn new(payload: u64, width: usize, signed: bool) -> Self {
        let payload = Box::new(BigUint::from(payload));
        let mask_xz = Box::new(BigUint::zero());
        Self {
            payload,
            mask_xz,
            width: width as u32,
            signed,
        }
    }

    pub fn new_biguint(payload: BigUint, width: usize, signed: bool) -> Self {
        let payload = Box::new(payload);
        let mask_xz = Box::new(BigUint::zero());
        Self {
            payload,
            mask_xz,
            width: width as u32,
            signed,
        }
    }

    pub fn new_bigint(payload: BigInt, width: usize, signed: bool) -> Self {
        let payload = if payload.sign() == Sign::Minus {
            let val = payload.magnitude();
            let mask = Self::gen_mask(width);
            ((val ^ &mask) + BigUint::one()) & &mask
        } else {
            payload.magnitude().clone()
        };

        let payload = Box::new(payload);
        let mask_xz = Box::new(BigUint::zero());
        Self {
            payload,
            mask_xz,
            width: width as u32,
            signed,
        }
    }

    pub fn new_x(width: usize, signed: bool) -> Self {
        let payload = Box::new(BigUint::zero());
        let mask_xz = Box::new(Self::gen_mask(width));
        Self {
            payload,
            mask_xz,
            width: width as u32,
            signed,
        }
    }

    pub fn new_z(width: usize, signed: bool) -> Self {
        let payload = Box::new(Self::gen_mask(width));
        let mask_xz = Box::new(Self::gen_mask(width));
        Self {
            payload,
            mask_xz,
            width: width as u32,
            signed,
        }
    }

    pub fn is_xz(&self) -> bool {
        *self.mask_xz != BigUint::zero()
    }

    pub fn payload(&self) -> &BigUint {
        self.payload.as_ref()
    }

    pub fn mask_xz(&self) -> &BigUint {
        self.mask_xz.as_ref()
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
        let beg = Self::gen_mask(width);
        let mut end = Self::gen_mask(end);
        end ^= &beg;
        beg & end
    }

    pub fn trunc(&mut self, width: usize) {
        let mask = Self::gen_mask(width);
        *self.payload &= &mask;
        *self.mask_xz &= &mask;
        self.width = width as u32;
    }

    pub fn select(&self, beg: usize, end: usize) -> Self {
        if beg < end {
            Self::default()
        } else {
            let width = beg - end + 1;
            let mask = Self::gen_mask(width);
            let mut ret = self.clone();

            *ret.payload >>= end;
            *ret.mask_xz >>= end;
            *ret.payload &= &mask;
            *ret.mask_xz &= &mask;
            ret.width = width as u32;
            ret.signed = false;

            ret
        }
    }

    pub fn assign(&mut self, mut value: Self, beg: usize, end: usize) {
        *value.payload <<= end;
        *value.mask_xz <<= end;

        let mask = Self::gen_mask(self.width as usize);
        let mask_range = Self::gen_mask_range(beg, end);
        let inv_mask = &mask ^ &mask_range;

        *self.payload = (self.payload() & &inv_mask) | (value.payload() & &mask);
        *self.mask_xz = (self.mask_xz() & &inv_mask) | (value.mask_xz() & &mask);
    }

    pub fn to_usize(&self) -> Option<usize> {
        if *self.mask_xz != BigUint::zero() {
            None
        } else {
            self.payload.to_usize()
        }
    }

    pub fn to_u32(&self) -> Option<u32> {
        if *self.mask_xz != BigUint::zero() {
            None
        } else {
            self.payload.to_u32()
        }
    }

    pub fn to_bigint(&self) -> Option<BigInt> {
        if *self.mask_xz != BigUint::zero() {
            None
        } else {
            let msb = self.payload.bit((self.width - 1) as u64);
            let sign = if msb { Sign::Minus } else { Sign::Plus };

            if msb {
                let mask = Self::gen_mask(self.width as usize);
                let val = ((self.payload.as_ref() ^ &mask) + BigUint::one()) & &mask;
                Some(BigInt::from_biguint(sign, val))
            } else {
                Some(BigInt::from_biguint(sign, self.payload.as_ref().clone()))
            }
        }
    }

    pub fn to_value_u64(&self) -> Option<ValueU64> {
        if self.width <= 64 {
            Some(ValueU64 {
                payload: self.payload.to_u64().unwrap(),
                mask_xz: self.mask_xz.to_u64().unwrap(),
                width: self.width,
                signed: self.signed,
            })
        } else {
            None
        }
    }
}

impl fmt::LowerHex for ValueBigUint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Display;

        let payload = self.payload.to_u64_digits();
        let mask_xz = self.mask_xz.to_u64_digits();

        let mut remaining = self.width;
        let mut i = 0;
        let mut ret = String::new();

        while remaining != 0 {
            let width = if remaining < 64 { remaining } else { 64 };
            let payload = payload.get(i).unwrap_or(&0);
            let mask_xz = mask_xz.get(i).unwrap_or(&0);
            ret = format!("{}{ret}", gen_hex_string(*payload, *mask_xz, width));
            remaining -= width;
            i += 1;
        }

        let signed = if self.signed { "s" } else { "" };
        format!("{}'{signed}h{ret}", self.width).fmt(f)
    }
}

impl fmt::Binary for ValueBigUint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Display;

        let payload = self.payload.to_u64_digits();
        let mask_xz = self.mask_xz.to_u64_digits();

        let mut remaining = self.width;
        let mut i = 0;
        let mut ret = String::new();

        while remaining != 0 {
            let width = if remaining < 64 { remaining } else { 64 };
            let payload = payload.get(i).unwrap_or(&0);
            let mask_xz = mask_xz.get(i).unwrap_or(&0);
            ret = format!("{}{ret}", gen_bin_string(*payload, *mask_xz, width));
            remaining -= width;
            i += 1;
        }

        let signed = if self.signed { "s" } else { "" };
        format!("{}'{signed}b{ret}", self.width).fmt(f)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    U64(ValueU64),
    BigUint(ValueBigUint),
}

impl Value {
    pub fn new(payload: u64, width: usize, signed: bool) -> Self {
        if width <= 64 {
            Self::U64(ValueU64::new(payload, width, signed))
        } else {
            Self::BigUint(ValueBigUint::new(payload, width, signed))
        }
    }

    pub fn new_biguint(payload: BigUint, width: usize, signed: bool) -> Self {
        if width <= 64 {
            Self::U64(ValueU64::new(payload.to_u64().unwrap(), width, signed))
        } else {
            Self::BigUint(ValueBigUint::new_biguint(payload, width, signed))
        }
    }

    pub fn new_x(width: usize, signed: bool) -> Self {
        if width <= 64 {
            Self::U64(ValueU64::new_x(width, signed))
        } else {
            Self::BigUint(ValueBigUint::new_x(width, signed))
        }
    }

    pub fn new_z(width: usize, signed: bool) -> Self {
        if width <= 64 {
            Self::U64(ValueU64::new_z(width, signed))
        } else {
            Self::BigUint(ValueBigUint::new_z(width, signed))
        }
    }

    #[inline(always)]
    pub fn is_xz(&self) -> bool {
        match self {
            Self::U64(x) => x.is_xz(),
            Self::BigUint(x) => x.is_xz(),
        }
    }

    #[inline(always)]
    pub fn payload(&self) -> Cow<'_, BigUint> {
        match self {
            Self::U64(x) => {
                let ret = BigUint::from(x.payload);
                Cow::Owned(ret)
            }
            Self::BigUint(x) => Cow::Borrowed(&x.payload),
        }
    }

    #[inline(always)]
    pub fn mask_xz(&self) -> Cow<'_, BigUint> {
        match self {
            Self::U64(x) => {
                let ret = BigUint::from(x.mask_xz);
                Cow::Owned(ret)
            }
            Self::BigUint(x) => Cow::Borrowed(&x.mask_xz),
        }
    }

    pub fn select(&self, beg: usize, end: usize) -> Self {
        match self {
            Self::U64(x) => Self::U64(x.select(beg, end)),
            Self::BigUint(x) => {
                let ret = x.select(beg, end);
                if let Some(x) = ret.to_value_u64() {
                    Self::U64(x)
                } else {
                    Self::BigUint(ret)
                }
            }
        }
    }

    pub fn trunc(&mut self, width: usize) {
        let self_width = self.width();
        if self_width == 0 {
            let Self::U64(x) = self else {
                unreachable!();
            };

            if width > 64 {
                let payload = if x.payload != 0 {
                    ValueBigUint::gen_mask(width)
                } else {
                    zero()
                };
                let mask_xz = if x.mask_xz != 0 {
                    ValueBigUint::gen_mask(width)
                } else {
                    zero()
                };
                *self = Self::BigUint(ValueBigUint {
                    payload: Box::new(payload),
                    mask_xz: Box::new(mask_xz),
                    width: width as u32,
                    signed: false,
                });
            } else {
                let payload = if x.payload != 0 {
                    ValueU64::gen_mask(width)
                } else {
                    0
                };
                let mask_xz = if x.mask_xz != 0 {
                    ValueU64::gen_mask(width)
                } else {
                    0
                };
                *self = Self::U64(ValueU64 {
                    payload,
                    mask_xz,
                    width: width as u32,
                    signed: false,
                });
            }
        } else {
            if self_width <= width {
                return;
            }

            match self {
                Self::U64(x) => x.trunc(width),
                Self::BigUint(x) => {
                    x.trunc(width);

                    if let Some(y) = x.to_value_u64() {
                        *self = Self::U64(y);
                    }
                }
            }
        }
    }

    pub fn concat(&self, x: &Value) -> Value {
        let width = self.width() + x.width();

        if width > 64 {
            let (mut payload, mut mask_xz) = match self {
                Self::U64(x) => (BigUint::from(x.payload), BigUint::from(x.mask_xz)),
                Self::BigUint(x) => (x.payload().clone(), x.mask_xz().clone()),
            };

            payload <<= x.width();
            mask_xz <<= x.width();

            match x {
                Self::U64(x) => {
                    payload |= BigUint::from(x.payload);
                    mask_xz |= BigUint::from(x.mask_xz);
                }
                Self::BigUint(x) => {
                    payload |= x.payload();
                    mask_xz |= x.mask_xz();
                }
            }

            Value::BigUint(ValueBigUint {
                payload: Box::new(payload),
                mask_xz: Box::new(mask_xz),
                width: width as u32,
                signed: false,
            })
        } else {
            let (mut payload, mut mask_xz) = if let Self::U64(x) = self {
                (x.payload, x.mask_xz)
            } else {
                unreachable!();
            };

            let shift = x.width();
            if shift != 64 {
                payload <<= shift;
                mask_xz <<= shift;
            }

            if let Self::U64(x) = x {
                payload |= x.payload;
                mask_xz |= x.mask_xz;
            } else {
                unreachable!();
            }

            Value::U64(ValueU64 {
                payload,
                mask_xz,
                width: width as u32,
                signed: false,
            })
        }
    }

    pub fn expand(&self, width: usize, use_sign: bool) -> Cow<'_, Self> {
        if self.width() == 0 || self.width() >= width {
            Cow::Borrowed(self)
        } else if width > 64 {
            let ret = match self {
                Self::U64(x) => {
                    let mut payload = Box::new(BigUint::from(x.payload));
                    let mut mask_xz = Box::new(BigUint::from(x.mask_xz));

                    if x.signed && use_sign {
                        let msb = payload.bit((x.width - 1) as u64);
                        let msb_xz = mask_xz.bit((x.width - 1) as u64);
                        if msb | msb_xz {
                            let mask0 = ValueBigUint::gen_mask(width);
                            let mask1 = ValueBigUint::gen_mask(x.width as usize);
                            let mask = mask0 ^ mask1;

                            if msb {
                                *payload |= &mask;
                            }
                            if msb_xz {
                                *mask_xz |= &mask;
                            }
                        }
                    }

                    let signed = if use_sign { x.signed } else { false };

                    ValueBigUint {
                        payload,
                        mask_xz,
                        width: width as u32,
                        signed,
                    }
                }
                Self::BigUint(x) => {
                    let mut ret = x.clone();

                    if x.signed && use_sign {
                        let msb = ret.payload.bit((x.width - 1) as u64);
                        let msb_xz = ret.mask_xz.bit((x.width - 1) as u64);
                        if msb | msb_xz {
                            let mask0 = ValueBigUint::gen_mask(width);
                            let mask1 = ValueBigUint::gen_mask(x.width as usize);
                            let mask = mask0 ^ mask1;

                            if msb {
                                *ret.payload |= &mask;
                            }
                            if msb_xz {
                                *ret.mask_xz |= &mask;
                            }
                        }
                    }

                    if !use_sign {
                        ret.signed = false;
                    }

                    ret.width = width as u32;
                    ret
                }
            };
            Cow::Owned(Value::BigUint(ret))
        } else if let Self::U64(x) = self {
            let mut payload = x.payload;
            let mut mask_xz = x.mask_xz;

            if x.signed && use_sign {
                let msb = ((payload >> (x.width - 1)) & 1) == 1;
                let msb_xz = ((mask_xz >> (x.width - 1)) & 1) == 1;
                if msb | msb_xz {
                    let mask0 = ValueU64::gen_mask(width);
                    let mask1 = ValueU64::gen_mask(x.width as usize);
                    let mask = mask0 ^ mask1;

                    if msb {
                        payload |= mask;
                    }
                    if msb_xz {
                        mask_xz |= mask;
                    }
                }
            }

            let signed = if use_sign { x.signed } else { false };

            Cow::Owned(Value::U64(ValueU64 {
                payload,
                mask_xz,
                width: width as u32,
                signed,
            }))
        } else {
            unreachable!();
        }
    }

    pub fn assign(&mut self, value: Value, beg: usize, end: usize) {
        match self {
            Self::U64(x) => {
                let Value::U64(value) = value else {
                    unreachable!();
                };
                x.assign(value, beg, end)
            }
            Self::BigUint(x) => {
                let Value::BigUint(value) = value else {
                    unreachable!();
                };
                x.assign(value, beg, end)
            }
        }
    }

    pub fn set_value(&mut self, mut value: Value) {
        value.trunc(self.width());
        match self {
            Self::U64(x) => {
                let Value::U64(value) = value else {
                    unreachable!();
                };
                x.payload = value.payload;
                x.mask_xz = value.mask_xz;
            }
            Self::BigUint(x) => match value {
                Value::U64(value) => {
                    *x.payload = BigUint::from(value.payload);
                    *x.mask_xz = BigUint::from(value.mask_xz);
                }
                Value::BigUint(value) => {
                    x.payload = value.payload;
                    x.mask_xz = value.mask_xz;
                }
            },
        }
    }

    #[inline(always)]
    pub fn clear_xz(&mut self) {
        match self {
            Self::U64(x) => {
                x.mask_xz = 0;
            }
            Self::BigUint(x) => {
                *x.mask_xz = 0u32.into();
            }
        }
    }

    #[inline(always)]
    pub fn width(&self) -> usize {
        match self {
            Self::U64(x) => x.width as usize,
            Self::BigUint(x) => x.width as usize,
        }
    }

    #[inline(always)]
    pub fn signed(&self) -> bool {
        match self {
            Self::U64(x) => x.signed,
            Self::BigUint(x) => x.signed,
        }
    }

    #[inline(always)]
    pub fn set_signed(&mut self, signed: bool) {
        match self {
            Self::U64(x) => x.signed = signed,
            Self::BigUint(x) => x.signed = signed,
        }
    }

    #[inline(always)]
    pub fn to_usize(&self) -> Option<usize> {
        match self {
            Self::U64(x) => x.to_usize(),
            Self::BigUint(_) => None,
        }
    }

    #[inline(always)]
    pub fn to_u32(&self) -> Option<u32> {
        match self {
            Self::U64(x) => x.to_u32(),
            Self::BigUint(_) => None,
        }
    }

    #[inline(always)]
    pub fn to_u64(&self) -> Option<u64> {
        match self {
            Self::U64(x) => x.to_u64(),
            Self::BigUint(_) => None,
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
            signed: self.signed(),
            width: Shape::new(vec![Some(self.width())]),
            ..Default::default()
        }
    }

    pub fn to_vcd_value(&self, i: u64) -> vcd::Value {
        if self.mask_xz().bit(i) {
            if self.payload().bit(i) {
                vcd::Value::Z
            } else {
                vcd::Value::X
            }
        } else if self.payload().bit(i) {
            vcd::Value::V1
        } else {
            vcd::Value::V0
        }
    }

    pub fn as_u64_ptr(&mut self) -> Option<*mut ValueU64> {
        if let Value::U64(x) = self {
            Some(x)
        } else {
            None
        }
    }
}

impl fmt::LowerHex for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::U64(x) => x.fmt(f),
            Self::BigUint(x) => x.fmt(f),
        }
    }
}

impl fmt::Binary for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::U64(x) => x.fmt(f),
            Self::BigUint(x) => x.fmt(f),
        }
    }
}

fn from_based_str(s: &str) -> Value {
    let x = s.replace('_', "");

    let (width, rest) = x.split_once('\'').unwrap();
    let signed = &rest[0..1] == "s";
    let rest = if signed { &rest[1..] } else { rest };
    let (base, value) = rest.split_at(1);
    let (radix, char_len, all1_char) = match base {
        "b" => (2, 1, '1'),
        "o" => (8, 3, '7'),
        "d" => (10, 0, '0'),
        "h" => (16, 4, 'f'),
        _ => unreachable!(),
    };
    let lexical_width = value.len() * char_len;

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
    let mut mask_x = BigUint::from_str_radix(&mask_x, radix).unwrap_or(BigUint::from(0u32));
    let mut mask_z = BigUint::from_str_radix(&mask_z, radix).unwrap_or(BigUint::from(0u32));

    let actual_width = payload.bits().max(mask_x.bits()).max(mask_z.bits()) as usize;

    let width = if let Ok(x) = str::parse::<usize>(width) {
        if x > lexical_width && lexical_width != 0 {
            let mask_x_msb = mask_x.bit(lexical_width as u64 - 1);
            let mask_z_msb = mask_z.bit(lexical_width as u64 - 1);

            if mask_x_msb {
                let mask = ValueBigUint::gen_mask(x);
                let inv_mask = ValueBigUint::gen_mask(lexical_width);
                let msb_expand = mask ^ inv_mask;
                mask_x |= msb_expand;
            }
            if mask_z_msb {
                let mask = ValueBigUint::gen_mask(x);
                let inv_mask = ValueBigUint::gen_mask(lexical_width);
                let msb_expand = mask ^ inv_mask;
                mask_z |= msb_expand;
            }
        }
        x
    } else {
        actual_width
    };

    let mask_xz = &mask_x | &mask_z;
    let inv_mask = ValueBigUint::gen_mask(actual_width);
    let payload = (payload & (&mask_xz ^ inv_mask)) | mask_z;

    let ret = ValueBigUint {
        payload: Box::new(payload),
        mask_xz: Box::new(mask_xz),
        width: width as u32,
        signed,
    };

    if let Some(x) = ret.to_value_u64() {
        Value::U64(x)
    } else {
        Value::BigUint(ret)
    }
}

fn from_base_less_str(s: &str) -> Value {
    let x = s.replace('_', "");
    let x = str::parse::<u64>(&x).unwrap();
    Value::new(x, 32, true)
}

fn from_all_bit_str(s: &str) -> Value {
    let (width, rest) = s.split_once('\'').unwrap();
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
        let mask = ValueBigUint::gen_mask(width);
        match rest {
            "0" => (zero(), zero(), width),
            "1" => (mask, zero(), width),
            "x" | "X" => (zero(), mask, width),
            "z" | "Z" => (mask.clone(), mask, width),
            _ => unreachable!(),
        }
    };

    let ret = ValueBigUint {
        payload: Box::new(payload),
        mask_xz: Box::new(mask_xz),
        width: width as u32,
        signed: false,
    };

    if let Some(x) = ret.to_value_u64() {
        Value::U64(x)
    } else {
        Value::BigUint(ret)
    }
}

fn from_fixed_point_str(s: &str) -> Value {
    if let Ok(x) = str::parse::<f64>(s) {
        Value::new(x.to_bits(), 64, false)
    } else {
        Value::new(0, 64, false)
    }
}

fn from_exponent_str(s: &str) -> Value {
    if let Ok(x) = str::parse::<f64>(s) {
        Value::new(x.to_bits(), 64, false)
    } else {
        Value::new(0, 64, false)
    }
}

impl str::FromStr for Value {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ret = if s.contains('\'') {
            let (_, rest) = s.split_once('\'').unwrap();
            if rest.starts_with(['s', 'b', 'o', 'd', 'h']) {
                from_based_str(s)
            } else {
                from_all_bit_str(s)
            }
        } else if s.contains('.') {
            from_fixed_point_str(s)
        } else {
            from_base_less_str(s)
        };
        Ok(ret)
    }
}

impl From<&syntax_tree::Based> for Value {
    fn from(value: &syntax_tree::Based) -> Self {
        let text = value.based_token.to_string();
        from_based_str(&text)
    }
}

impl From<&syntax_tree::BaseLess> for Value {
    fn from(value: &syntax_tree::BaseLess) -> Self {
        let text = value.base_less_token.to_string();
        from_base_less_str(&text)
    }
}

impl From<&syntax_tree::AllBit> for Value {
    fn from(value: &syntax_tree::AllBit) -> Self {
        let text = value.all_bit_token.to_string();
        from_all_bit_str(&text)
    }
}

impl From<&syntax_tree::FixedPoint> for Value {
    fn from(value: &syntax_tree::FixedPoint) -> Self {
        let text = value.fixed_point_token.to_string();
        from_fixed_point_str(&text)
    }
}

impl From<&syntax_tree::Exponent> for Value {
    fn from(value: &syntax_tree::Exponent) -> Self {
        let text = value.exponent_token.to_string();
        from_exponent_str(&text)
    }
}

impl From<&Value> for vcd::Value {
    fn from(value: &Value) -> Self {
        value.to_vcd_value(0)
    }
}

impl IntoIterator for &Value {
    type Item = vcd::Value;
    type IntoIter = VcdValueIter;
    fn into_iter(self) -> Self::IntoIter {
        VcdValueIter {
            pos: 0,
            value: self.clone(),
        }
    }
}

pub struct VcdValueIter {
    pos: u64,
    value: Value,
}

impl Iterator for VcdValueIter {
    type Item = vcd::Value;
    fn next(&mut self) -> Option<Self::Item> {
        let width = self.value.width() as u64;
        if self.pos < width {
            let value = self.value.to_vcd_value(width - self.pos - 1);
            self.pos += 1;
            Some(value)
        } else {
            None
        }
    }
}

/// Type for packed logic array defined by IEEE 1800-2023 H.10.1.2
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SvLogicVecVal {
    pub aval: u32,
    pub bval: u32,
}

impl From<&[SvLogicVecVal]> for Value {
    fn from(value: &[SvLogicVecVal]) -> Self {
        let width = (value.len() * 32) as u32;

        if width > 64 {
            let mut payload = BigUint::zero();
            let mut mask_xz = BigUint::zero();

            for val in value.iter().rev() {
                payload <<= 32;
                mask_xz <<= 32;
                payload |= BigUint::from(val.aval ^ val.bval);
                mask_xz |= BigUint::from(val.bval);
            }

            Value::BigUint(ValueBigUint {
                payload: Box::new(payload),
                mask_xz: Box::new(mask_xz),
                width,
                signed: false,
            })
        } else {
            let mut payload = 0u64;
            let mut mask_xz = 0u64;

            for val in value.iter().rev() {
                payload <<= 32;
                mask_xz <<= 32;
                payload |= (val.aval ^ val.bval) as u64;
                mask_xz |= val.bval as u64;
            }

            Value::U64(ValueU64 {
                payload,
                mask_xz,
                width,
                signed: false,
            })
        }
    }
}

impl From<&Value> for Vec<SvLogicVecVal> {
    fn from(value: &Value) -> Self {
        let mut ret = vec![];
        let len = if value.width().is_multiple_of(32) {
            value.width() / 32
        } else {
            value.width() / 32 + 1
        };

        match value {
            Value::U64(x) => {
                let mut payload = x.payload;
                let mut mask_xz = x.mask_xz;

                for _ in 0..len {
                    let payload_u32 = (payload & 0xffffffff) as u32;
                    let mask_xz_u32 = (mask_xz & 0xffffffff) as u32;
                    let aval = payload_u32 ^ mask_xz_u32;
                    let bval = mask_xz_u32;
                    ret.push(SvLogicVecVal { aval, bval });

                    payload >>= 32;
                    mask_xz >>= 32;
                }
            }
            Value::BigUint(x) => {
                let payload = x.payload.to_u32_digits();
                let mask_xz = x.mask_xz.to_u32_digits();

                for i in 0..len {
                    let payload_u32 = *payload.get(i).unwrap_or(&0);
                    let mask_xz_u32 = *mask_xz.get(i).unwrap_or(&0);
                    let aval = payload_u32 ^ mask_xz_u32;
                    let bval = mask_xz_u32;
                    ret.push(SvLogicVecVal { aval, bval });
                }
            }
        }

        ret
    }
}

#[derive(Clone, Debug, Default)]
pub struct MaskCache {
    table: HashMap<usize, BigUint>,
}

impl MaskCache {
    pub fn get(&mut self, width: usize) -> &BigUint {
        self.table
            .entry(width)
            .or_insert_with(|| ValueBigUint::gen_mask(width));

        self.table.get(&width).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Op;
    use std::str::FromStr;

    #[test]
    fn select() {
        let x00 = Value::new(0x000, 10, false);
        let x01 = Value::new(0x01a, 10, false);
        let x02 = Value::new(0x3ff, 10, false);

        let x00 = x00.select(1, 0);
        let x01 = x01.select(4, 1);
        let x02 = x02.select(9, 8);

        assert_eq!(&format!("{:x}", x00), "2'h0");
        assert_eq!(&format!("{:x}", x01), "4'hd");
        assert_eq!(&format!("{:x}", x02), "2'h3");

        let x10 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);
        let x11 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);
        let x12 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);

        let x10 = x10.select(15, 2);
        let x11 = x11.select(31, 3);
        let x12 = x12.select(79, 8);

        assert_eq!(&format!("{:x}", x10), "14'h3c3c");
        assert_eq!(&format!("{:x}", x11), "29'h00001e1e");
        assert_eq!(&format!("{:x}", x12), "72'hd0d00000e0e00000f0");
    }

    #[test]
    fn trunc() {
        let mut x00 = Value::new(0x000, 10, false);
        let mut x01 = Value::new(0x01a, 10, false);
        let mut x02 = Value::new(0x3ff, 10, false);

        x00.trunc(4);
        x01.trunc(5);
        x02.trunc(6);

        assert_eq!(&format!("{:x}", x00), "4'h0");
        assert_eq!(&format!("{:x}", x01), "5'h1a");
        assert_eq!(&format!("{:x}", x02), "6'h3f");

        let mut x10 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);
        let mut x11 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);
        let mut x12 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);

        x10.trunc(16);
        x11.trunc(32);
        x12.trunc(72);

        assert_eq!(&format!("{:x}", x10), "16'hf0f0");
        assert_eq!(&format!("{:x}", x11), "32'h0000f0f0");
        assert_eq!(&format!("{:x}", x12), "72'hd00000e0e00000f0f0");
    }

    #[test]
    fn concat() {
        let x00 = Value::new(0x000, 10, false);
        let x01 = Value::new(0x01a, 10, false);
        let x02 = Value::new(0x3ff, 10, false);

        let x00 = x00.concat(&x01);
        let x01 = x01.concat(&x02);
        let x02 = x02.concat(&x00);

        assert_eq!(&format!("{:x}", x00), "20'h0001a");
        assert_eq!(&format!("{:x}", x01), "20'h06bff");
        assert_eq!(&format!("{:x}", x02), "30'h3ff0001a");

        let x10 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);
        let x11 = Value::new_biguint(BigUint::from_slice(&[0xc0c0, 0xb0b0, 0xa0a0]), 80, false);
        let x12 = Value::new_biguint(BigUint::from_slice(&[0x9090, 0x8080, 0x7070]), 80, false);

        let x10 = x10.concat(&x11);
        let x11 = x11.concat(&x12);
        let x12 = x12.concat(&x10);

        assert_eq!(
            &format!("{:x}", x10),
            "160'hd0d00000e0e00000f0f0a0a00000b0b00000c0c0"
        );
        assert_eq!(
            &format!("{:x}", x11),
            "160'ha0a00000b0b00000c0c070700000808000009090"
        );
        assert_eq!(
            &format!("{:x}", x12),
            "240'h70700000808000009090d0d00000e0e00000f0f0a0a00000b0b00000c0c0"
        );
    }

    #[test]
    fn value_format() {
        let x00 = Value::new(0x000, 10, false);
        let x01 = Value::new(0x01a, 10, false);
        let x02 = Value::new(0x3ff, 10, false);

        assert_eq!(&format!("{:x}", x00), "10'h000");
        assert_eq!(&format!("{:x}", x01), "10'h01a");
        assert_eq!(&format!("{:x}", x02), "10'h3ff");
        assert_eq!(&format!("{:b}", x00), "10'b0000000000");
        assert_eq!(&format!("{:b}", x01), "10'b0000011010");
        assert_eq!(&format!("{:b}", x02), "10'b1111111111");

        let x10 = Value::new(0x000, 80, false);
        let x11 = Value::new(0x01a, 80, false);
        let x12 = Value::new(0x3ff, 80, false);
        let x13 = Value::new_biguint(BigUint::from_slice(&[0xf0f0, 0xe0e0, 0xd0d0]), 80, false);

        assert_eq!(&format!("{:x}", x10), "80'h00000000000000000000");
        assert_eq!(&format!("{:x}", x11), "80'h0000000000000000001a");
        assert_eq!(&format!("{:x}", x12), "80'h000000000000000003ff");
        assert_eq!(&format!("{:x}", x13), "80'hd0d00000e0e00000f0f0");
        assert_eq!(
            &format!("{:b}", x10),
            "80'b00000000000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            &format!("{:b}", x11),
            "80'b00000000000000000000000000000000000000000000000000000000000000000000000000011010"
        );
        assert_eq!(
            &format!("{:b}", x12),
            "80'b00000000000000000000000000000000000000000000000000000000000000000000001111111111"
        );
        assert_eq!(
            &format!("{:b}", x13),
            "80'b11010000110100000000000000000000111000001110000000000000000000001111000011110000"
        );

        let x20 = Value::new_x(10, false);
        let x21 = Value::new_z(10, false);
        let x22 = Value::new_x(80, false);
        let x23 = Value::new_z(80, false);

        assert_eq!(&format!("{:x}", x20), "10'hxxx");
        assert_eq!(&format!("{:x}", x21), "10'hzzz");
        assert_eq!(&format!("{:x}", x22), "80'hxxxxxxxxxxxxxxxxxxxx");
        assert_eq!(&format!("{:x}", x23), "80'hzzzzzzzzzzzzzzzzzzzz");
        assert_eq!(&format!("{:b}", x20), "10'bxxxxxxxxxx");
        assert_eq!(&format!("{:b}", x21), "10'bzzzzzzzzzz");
        assert_eq!(
            &format!("{:b}", x22),
            "80'bxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        );
        assert_eq!(
            &format!("{:b}", x23),
            "80'bzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
        );
    }

    #[test]
    fn xz_parse_format() {
        let x0 = Value::from_str("1'bx").unwrap();
        let x1 = Value::from_str("2'bx").unwrap();
        let x2 = Value::from_str("3'bx").unwrap();
        let x3 = Value::from_str("4'bx").unwrap();
        let x4 = Value::from_str("2'b0x").unwrap();
        let x5 = Value::from_str("3'b0x").unwrap();
        let x6 = Value::from_str("4'b0x").unwrap();

        assert_eq!(&format!("{:x}", x0), "1'hx");
        assert_eq!(&format!("{:x}", x1), "2'hx");
        assert_eq!(&format!("{:x}", x2), "3'hx");
        assert_eq!(&format!("{:x}", x3), "4'hx");
        assert_eq!(&format!("{:x}", x4), "2'hX");
        assert_eq!(&format!("{:x}", x5), "3'hX");
        assert_eq!(&format!("{:x}", x6), "4'hX");
        assert_eq!(&format!("{:b}", x0), "1'bx");
        assert_eq!(&format!("{:b}", x1), "2'bxx");
        assert_eq!(&format!("{:b}", x2), "3'bxxx");
        assert_eq!(&format!("{:b}", x3), "4'bxxxx");
        assert_eq!(&format!("{:b}", x4), "2'b0x");
        assert_eq!(&format!("{:b}", x5), "3'b00x");
        assert_eq!(&format!("{:b}", x6), "4'b000x");
    }

    #[test]
    fn parse() {
        let x00 = Value::from_str("10'b101").unwrap();
        let x01 = Value::from_str("10'o123").unwrap();
        let x02 = Value::from_str("10'd123").unwrap();
        let x03 = Value::from_str("10'h12f").unwrap();
        let x04 = Value::from_str("10'sb101").unwrap();
        let x05 = Value::from_str("10'so123").unwrap();
        let x06 = Value::from_str("10'sd123").unwrap();
        let x07 = Value::from_str("10'sh12f").unwrap();
        let x08 = Value::from_str("'0").unwrap();
        let x09 = Value::from_str("'1").unwrap();
        let x10 = Value::from_str("'x").unwrap();
        let x11 = Value::from_str("'z").unwrap();
        let x12 = Value::from_str("10'0").unwrap();
        let x13 = Value::from_str("10'1").unwrap();
        let x14 = Value::from_str("10'x").unwrap();
        let x15 = Value::from_str("10'z").unwrap();
        let x16 = Value::from_str("1").unwrap();
        let x17 = Value::from_str("10").unwrap();
        let x18 = Value::from_str("100").unwrap();
        let x19 = Value::from_str("1000").unwrap();
        let x20 = Value::from_str("1.0").unwrap();
        let x21 = Value::from_str("1.11").unwrap();
        let x22 = Value::from_str("1.0e10").unwrap();
        let x23 = Value::from_str("1.11e10").unwrap();

        assert_eq!(&format!("{:x}", x00), "10'h005");
        assert_eq!(&format!("{:x}", x01), "10'h053");
        assert_eq!(&format!("{:x}", x02), "10'h07b");
        assert_eq!(&format!("{:x}", x03), "10'h12f");
        assert_eq!(&format!("{:x}", x04), "10'sh005");
        assert_eq!(&format!("{:x}", x05), "10'sh053");
        assert_eq!(&format!("{:x}", x06), "10'sh07b");
        assert_eq!(&format!("{:x}", x07), "10'sh12f");
        assert_eq!(&format!("{:x}", x08), "'0");
        assert_eq!(&format!("{:x}", x09), "'1");
        assert_eq!(&format!("{:x}", x10), "'x");
        assert_eq!(&format!("{:x}", x11), "'z");
        assert_eq!(&format!("{:x}", x12), "10'h000");
        assert_eq!(&format!("{:x}", x13), "10'h3ff");
        assert_eq!(&format!("{:x}", x14), "10'hxxx");
        assert_eq!(&format!("{:x}", x15), "10'hzzz");
        assert_eq!(&format!("{:x}", x16), "32'sh00000001");
        assert_eq!(&format!("{:x}", x17), "32'sh0000000a");
        assert_eq!(&format!("{:x}", x18), "32'sh00000064");
        assert_eq!(&format!("{:x}", x19), "32'sh000003e8");
        assert_eq!(&format!("{:x}", x20), "64'h3ff0000000000000");
        assert_eq!(&format!("{:x}", x21), "64'h3ff1c28f5c28f5c3");
        assert_eq!(&format!("{:x}", x22), "64'h4202a05f20000000");
        assert_eq!(&format!("{:x}", x23), "64'h4204ace478000000");
    }

    #[test]
    fn test_mask() {
        assert_eq!(format!("{:x}", ValueU64::gen_mask(1)), "1");
        assert_eq!(format!("{:x}", ValueU64::gen_mask(2)), "3");
        assert_eq!(format!("{:x}", ValueU64::gen_mask(3)), "7");
        assert_eq!(format!("{:x}", ValueU64::gen_mask(10)), "3ff");
        assert_eq!(format!("{:x}", ValueU64::gen_mask(59)), "7ffffffffffffff");
        assert_eq!(
            format!("{:x}", ValueBigUint::gen_mask(90)),
            "3ffffffffffffffffffffff"
        );
    }

    #[test]
    fn bit_expand() {
        // x = 8'h11 ; $display("%b", x); // 0000000000010001
        // x = 8'hf2 ; $display("%b", x); // 0000000011110010
        // x = 8'hx3 ; $display("%b", x); // 00000000xxxx0011
        // x = 8'hz4 ; $display("%b", x); // 00000000zzzz0100
        // x = 8'sh15; $display("%b", x); // 0000000000010101
        // x = 8'shf6; $display("%b", x); // 1111111111110110
        // x = 8'shx7; $display("%b", x); // xxxxxxxxxxxx0111
        // x = 8'shz8; $display("%b", x); // zzzzzzzzzzzz1000

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = x00.expand(16, true);
        let x01 = x01.expand(16, true);
        let x02 = x02.expand(16, true);
        let x03 = x03.expand(16, true);
        let x04 = x04.expand(16, true);
        let x05 = x05.expand(16, true);
        let x06 = x06.expand(16, true);
        let x07 = x07.expand(16, true);
        assert_eq!(format!("{:b}", x00.as_ref()), "16'b0000000000010001");
        assert_eq!(format!("{:b}", x01.as_ref()), "16'b0000000011110010");
        assert_eq!(format!("{:b}", x02.as_ref()), "16'b00000000xxxx0011");
        assert_eq!(format!("{:b}", x03.as_ref()), "16'b00000000zzzz0100");
        assert_eq!(format!("{:b}", x04.as_ref()), "16'sb0000000000010101");
        assert_eq!(format!("{:b}", x05.as_ref()), "16'sb1111111111110110");
        assert_eq!(format!("{:b}", x06.as_ref()), "16'sbxxxxxxxxxxxx0111");
        assert_eq!(format!("{:b}", x07.as_ref()), "16'sbzzzzzzzzzzzz1000");

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = x00.expand(68, true);
        let x01 = x01.expand(68, true);
        let x02 = x02.expand(68, true);
        let x03 = x03.expand(68, true);
        let x04 = x04.expand(68, true);
        let x05 = x05.expand(68, true);
        let x06 = x06.expand(68, true);
        let x07 = x07.expand(68, true);
        assert_eq!(format!("{:x}", x00.as_ref()), "68'h00000000000000011");
        assert_eq!(format!("{:x}", x01.as_ref()), "68'h000000000000000f2");
        assert_eq!(format!("{:x}", x02.as_ref()), "68'h000000000000000x3");
        assert_eq!(format!("{:x}", x03.as_ref()), "68'h000000000000000z4");
        assert_eq!(format!("{:x}", x04.as_ref()), "68'sh00000000000000015");
        assert_eq!(format!("{:x}", x05.as_ref()), "68'shffffffffffffffff6");
        assert_eq!(format!("{:x}", x06.as_ref()), "68'shxxxxxxxxxxxxxxxx7");
        assert_eq!(format!("{:x}", x07.as_ref()), "68'shzzzzzzzzzzzzzzzz8");
    }

    #[test]
    fn signed_unsigned() {
        //x = 8'shf2 + 8'hf2           ; $display("%b", x); // 0000000111100100
        //x = (8'shf2 + 8'shf2) + 8'hf2; $display("%b", x); // 0000001011010110

        let op = Op::Add;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'shf2").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x10 = op.eval_binary(&x00, &x01, Some(16), false, &mut cache);
        let x11 = op.eval_binary(
            &op.eval_binary(&x00, &x00, Some(16), false, &mut cache),
            &x01,
            Some(16),
            false,
            &mut cache,
        );
        assert_eq!(format!("{:b}", x10), "16'b0000000111100100");
        assert_eq!(format!("{:b}", x11), "16'b0000001011010110");
    }

    #[test]
    fn sv_logic_vec_val() {
        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("68'h11").unwrap();
        let x05 = Value::from_str("68'hf2").unwrap();
        let x06 = Value::from_str("68'hx3").unwrap();
        let x07 = Value::from_str("68'hz4").unwrap();

        let x00: Vec<SvLogicVecVal> = (&x00).into();
        let x01: Vec<SvLogicVecVal> = (&x01).into();
        let x02: Vec<SvLogicVecVal> = (&x02).into();
        let x03: Vec<SvLogicVecVal> = (&x03).into();
        let x04: Vec<SvLogicVecVal> = (&x04).into();
        let x05: Vec<SvLogicVecVal> = (&x05).into();
        let x06: Vec<SvLogicVecVal> = (&x06).into();
        let x07: Vec<SvLogicVecVal> = (&x07).into();

        assert_eq!(x00[0].aval, 0x11);
        assert_eq!(x00[0].bval, 0);
        assert_eq!(x01[0].aval, 0xf2);
        assert_eq!(x01[0].bval, 0);
        assert_eq!(x02[0].aval, 0xf3);
        assert_eq!(x02[0].bval, 0xf0);
        assert_eq!(x03[0].aval, 0x4);
        assert_eq!(x03[0].bval, 0xf0);
        assert_eq!(x04[0].aval, 0x11);
        assert_eq!(x04[0].bval, 0);
        assert_eq!(x04[1].aval, 0);
        assert_eq!(x04[1].bval, 0);
        assert_eq!(x05[0].aval, 0xf2);
        assert_eq!(x05[0].bval, 0);
        assert_eq!(x05[1].aval, 0);
        assert_eq!(x05[1].bval, 0);
        assert_eq!(x06[0].aval, 0xfffffff3);
        assert_eq!(x06[0].bval, 0xfffffff0);
        assert_eq!(x06[1].aval, 0xffffffff);
        assert_eq!(x06[1].bval, 0xffffffff);
        assert_eq!(x07[0].aval, 0x4);
        assert_eq!(x07[0].bval, 0xfffffff0);
        assert_eq!(x07[1].aval, 0);
        assert_eq!(x07[1].bval, 0xffffffff);

        let x00: Value = x00.as_slice().into();
        let x01: Value = x01.as_slice().into();
        let x02: Value = x02.as_slice().into();
        let x03: Value = x03.as_slice().into();
        let x04: Value = x04.as_slice().into();
        let x05: Value = x05.as_slice().into();
        let x06: Value = x06.as_slice().into();
        let x07: Value = x07.as_slice().into();

        assert_eq!(format!("{:x}", x00), "32'h00000011");
        assert_eq!(format!("{:x}", x01), "32'h000000f2");
        assert_eq!(format!("{:x}", x02), "32'h000000x3");
        assert_eq!(format!("{:x}", x03), "32'h000000z4");
        assert_eq!(format!("{:x}", x04), "96'h000000000000000000000011");
        assert_eq!(format!("{:x}", x05), "96'h0000000000000000000000f2");
        assert_eq!(format!("{:x}", x06), "96'h0000000xxxxxxxxxxxxxxxx3");
        assert_eq!(format!("{:x}", x07), "96'h0000000zzzzzzzzzzzzzzzz4");
    }

    #[test]
    fn unary_add() {
        //x = +8'h11 ; $display("%b", x); // 0000000000010001
        //x = +8'hf2 ; $display("%b", x); // 0000000011110010
        //x = +8'hx3 ; $display("%b", x); // 00000000xxxx0011
        //x = +8'hz4 ; $display("%b", x); // 00000000zzzz0100
        //x = +8'sh15; $display("%b", x); // 0000000000010101
        //x = +8'shf6; $display("%b", x); // 1111111111110110
        //x = +8'shx7; $display("%b", x); // xxxxxxxxxxxx0111
        //x = +8'shz8; $display("%b", x); // zzzzzzzzzzzz1000

        let op = Op::Add;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), true, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), true, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), true, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000010001");
        assert_eq!(format!("{:b}", x01), "16'b0000000011110010");
        assert_eq!(format!("{:b}", x02), "16'b00000000xxxx0011");
        assert_eq!(format!("{:b}", x03), "16'b00000000zzzz0100");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000010101");
        assert_eq!(format!("{:b}", x05), "16'sb1111111111110110");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxxxx0111");
        assert_eq!(format!("{:b}", x07), "16'sbzzzzzzzzzzzz1000");

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), true, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), true, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), true, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000011");
        assert_eq!(format!("{:x}", x01), "68'h000000000000000f2");
        assert_eq!(format!("{:x}", x02), "68'h000000000000000x3");
        assert_eq!(format!("{:x}", x03), "68'h000000000000000z4");
        assert_eq!(format!("{:x}", x04), "68'sh00000000000000015");
        assert_eq!(format!("{:x}", x05), "68'shffffffffffffffff6");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxx7");
        assert_eq!(format!("{:x}", x07), "68'shzzzzzzzzzzzzzzzz8");
    }

    #[test]
    fn unary_sub() {
        //x = -8'h11 ; $display("%b", x); // 1111111111101111
        //x = -8'hf2 ; $display("%b", x); // 1111111100001110
        //x = -8'hx3 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = -8'hz4 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = -8'sh15; $display("%b", x); // 1111111111101011
        //x = -8'shf6; $display("%b", x); // 0000000000001010
        //x = -8'shx7; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = -8'shz8; $display("%b", x); // xxxxxxxxxxxxxxxx

        let op = Op::Sub;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), true, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), true, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), true, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b1111111111101111");
        assert_eq!(format!("{:b}", x01), "16'b1111111100001110");
        assert_eq!(format!("{:b}", x02), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x03), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x04), "16'sb1111111111101011");
        assert_eq!(format!("{:b}", x05), "16'sb0000000000001010");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x07), "16'sbxxxxxxxxxxxxxxxx");

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), true, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), true, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), true, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'hfffffffffffffffef");
        assert_eq!(format!("{:x}", x01), "68'hfffffffffffffff0e");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x04), "68'shfffffffffffffffeb");
        assert_eq!(format!("{:x}", x05), "68'sh0000000000000000a");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x07), "68'shxxxxxxxxxxxxxxxxx");
    }

    #[test]
    fn unary_bit_not() {
        //x = ~8'h11 ; $display("%b", x); // 1111111111101110
        //x = ~8'hf2 ; $display("%b", x); // 1111111100001101
        //x = ~8'hx3 ; $display("%b", x); // 11111111xxxx1100
        //x = ~8'hz4 ; $display("%b", x); // 11111111xxxx1011
        //x = ~8'sh15; $display("%b", x); // 1111111111101010
        //x = ~8'shf6; $display("%b", x); // 0000000000001001
        //x = ~8'shx7; $display("%b", x); // xxxxxxxxxxxx1000
        //x = ~8'shz8; $display("%b", x); // xxxxxxxxxxxx0111

        let op = Op::BitNot;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), true, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), true, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), true, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b1111111111101110");
        assert_eq!(format!("{:b}", x01), "16'b1111111100001101");
        assert_eq!(format!("{:b}", x02), "16'b11111111xxxx1100");
        assert_eq!(format!("{:b}", x03), "16'b11111111xxxx1011");
        assert_eq!(format!("{:b}", x04), "16'sb1111111111101010");
        assert_eq!(format!("{:b}", x05), "16'sb0000000000001001");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxxxx1000");
        assert_eq!(format!("{:b}", x07), "16'sbxxxxxxxxxxxx0111");

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh15").unwrap();
        let x05 = Value::from_str("8'shf6").unwrap();
        let x06 = Value::from_str("8'shx7").unwrap();
        let x07 = Value::from_str("8'shz8").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), true, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), true, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), true, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'hfffffffffffffffee");
        assert_eq!(format!("{:x}", x01), "68'hfffffffffffffff0d");
        assert_eq!(format!("{:x}", x02), "68'hfffffffffffffffxc");
        assert_eq!(format!("{:x}", x03), "68'hfffffffffffffffxb");
        assert_eq!(format!("{:x}", x04), "68'shfffffffffffffffea");
        assert_eq!(format!("{:x}", x05), "68'sh00000000000000009");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxx8");
        assert_eq!(format!("{:x}", x07), "68'shxxxxxxxxxxxxxxxx7");
    }

    #[test]
    fn unary_bit_and() {
        //x = &8'h11; $display("%b", x); // 0000000000000000
        //x = &8'hff; $display("%b", x); // 0000000000000001
        //x = &8'hxx; $display("%b", x); // 000000000000000x
        //x = &8'hzz; $display("%b", x); // 000000000000000x
        //x = &8'h1x; $display("%b", x); // 0000000000000000
        //x = &8'h1z; $display("%b", x); // 0000000000000000
        //x = &8'hfx; $display("%b", x); // 000000000000000x
        //x = &8'hxz; $display("%b", x); // 000000000000000x

        let op = Op::BitAnd;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hff").unwrap();
        let x02 = Value::from_str("8'hxx").unwrap();
        let x03 = Value::from_str("8'hzz").unwrap();
        let x04 = Value::from_str("8'h1x").unwrap();
        let x05 = Value::from_str("8'h1z").unwrap();
        let x06 = Value::from_str("8'hfx").unwrap();
        let x07 = Value::from_str("8'hxz").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h11111111111111111").unwrap();
        let x01 = Value::from_str("68'hfffffffffffffffff").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x04 = Value::from_str("68'h1111111111111111x").unwrap();
        let x05 = Value::from_str("68'h1111111111111111z").unwrap();
        let x06 = Value::from_str("68'hffffffffffffffffx").unwrap();
        let x07 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn unary_bit_nand() {
        //x = ~&8'h11; $display("%b", x); // 0000000000000001
        //x = ~&8'hff; $display("%b", x); // 0000000000000000
        //x = ~&8'hxx; $display("%b", x); // 000000000000000x
        //x = ~&8'hzz; $display("%b", x); // 000000000000000x
        //x = ~&8'h1x; $display("%b", x); // 0000000000000001
        //x = ~&8'h1z; $display("%b", x); // 0000000000000001
        //x = ~&8'hfx; $display("%b", x); // 000000000000000x
        //x = ~&8'hxz; $display("%b", x); // 000000000000000x

        let op = Op::BitNand;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h11").unwrap();
        let x01 = Value::from_str("8'hff").unwrap();
        let x02 = Value::from_str("8'hxx").unwrap();
        let x03 = Value::from_str("8'hzz").unwrap();
        let x04 = Value::from_str("8'h1x").unwrap();
        let x05 = Value::from_str("8'h1z").unwrap();
        let x06 = Value::from_str("8'hfx").unwrap();
        let x07 = Value::from_str("8'hxz").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h11111111111111111").unwrap();
        let x01 = Value::from_str("68'hfffffffffffffffff").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x04 = Value::from_str("68'h1111111111111111x").unwrap();
        let x05 = Value::from_str("68'h1111111111111111z").unwrap();
        let x06 = Value::from_str("68'hffffffffffffffffx").unwrap();
        let x07 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn unary_bit_or() {
        //x = |8'h00; $display("%b", x); // 0000000000000000
        //x = |8'h11; $display("%b", x); // 0000000000000001
        //x = |8'hxx; $display("%b", x); // 000000000000000x
        //x = |8'hzz; $display("%b", x); // 000000000000000x
        //x = |8'h0x; $display("%b", x); // 000000000000000x
        //x = |8'h0z; $display("%b", x); // 000000000000000x
        //x = |8'h1x; $display("%b", x); // 0000000000000001
        //x = |8'h1z; $display("%b", x); // 0000000000000001

        let op = Op::BitOr;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'h11").unwrap();
        let x02 = Value::from_str("8'hxx").unwrap();
        let x03 = Value::from_str("8'hzz").unwrap();
        let x04 = Value::from_str("8'h0x").unwrap();
        let x05 = Value::from_str("8'h0z").unwrap();
        let x06 = Value::from_str("8'h1x").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x06), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x07), "16'b0000000000000001");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'h11111111111111111").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x04 = Value::from_str("68'h0000000000000000x").unwrap();
        let x05 = Value::from_str("68'h0000000000000000z").unwrap();
        let x06 = Value::from_str("68'h1111111111111111x").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x06), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x07), "68'h00000000000000001");
    }

    #[test]
    fn unary_bit_nor() {
        //x = ~|8'h00; $display("%b", x); // 0000000000000001
        //x = ~|8'h11; $display("%b", x); // 0000000000000000
        //x = ~|8'hxx; $display("%b", x); // 000000000000000x
        //x = ~|8'hzz; $display("%b", x); // 000000000000000x
        //x = ~|8'h0x; $display("%b", x); // 000000000000000x
        //x = ~|8'h0z; $display("%b", x); // 000000000000000x
        //x = ~|8'h1x; $display("%b", x); // 0000000000000000
        //x = ~|8'h1z; $display("%b", x); // 0000000000000000

        let op = Op::BitNor;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'h11").unwrap();
        let x02 = Value::from_str("8'hxx").unwrap();
        let x03 = Value::from_str("8'hzz").unwrap();
        let x04 = Value::from_str("8'h0x").unwrap();
        let x05 = Value::from_str("8'h0z").unwrap();
        let x06 = Value::from_str("8'h1x").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x06), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x07), "16'b0000000000000000");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'h11111111111111111").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x04 = Value::from_str("68'h0000000000000000x").unwrap();
        let x05 = Value::from_str("68'h0000000000000000z").unwrap();
        let x06 = Value::from_str("68'h1111111111111111x").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x06), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x07), "68'h00000000000000000");
    }

    #[test]
    fn unary_bit_xor() {
        //x = ^8'h00; $display("%b", x); // 0000000000000000
        //x = ^8'h01; $display("%b", x); // 0000000000000001
        //x = ^8'hxx; $display("%b", x); // 000000000000000x
        //x = ^8'hzz; $display("%b", x); // 000000000000000x
        //x = ^8'h0x; $display("%b", x); // 000000000000000x
        //x = ^8'h0z; $display("%b", x); // 000000000000000x
        //x = ^8'h1x; $display("%b", x); // 000000000000000x
        //x = ^8'h1z; $display("%b", x); // 000000000000000x

        let op = Op::BitXor;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'h01").unwrap();
        let x02 = Value::from_str("8'hxx").unwrap();
        let x03 = Value::from_str("8'hzz").unwrap();
        let x04 = Value::from_str("8'h0x").unwrap();
        let x05 = Value::from_str("8'h0z").unwrap();
        let x06 = Value::from_str("8'h1x").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'h11111111111111111").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x04 = Value::from_str("68'h0000000000000000x").unwrap();
        let x05 = Value::from_str("68'h0000000000000000z").unwrap();
        let x06 = Value::from_str("68'h1111111111111111x").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn unary_bit_xnor() {
        //x = ~^8'h00; $display("%b", x); // 0000000000000001
        //x = ~^8'h01; $display("%b", x); // 0000000000000000
        //x = ~^8'hxx; $display("%b", x); // 000000000000000x
        //x = ~^8'hzz; $display("%b", x); // 000000000000000x
        //x = ~^8'h0x; $display("%b", x); // 000000000000000x
        //x = ~^8'h0z; $display("%b", x); // 000000000000000x
        //x = ~^8'h1x; $display("%b", x); // 000000000000000x
        //x = ~^8'h1z; $display("%b", x); // 000000000000000x

        let op = Op::BitXnor;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'h01").unwrap();
        let x02 = Value::from_str("8'hxx").unwrap();
        let x03 = Value::from_str("8'hzz").unwrap();
        let x04 = Value::from_str("8'h0x").unwrap();
        let x05 = Value::from_str("8'h0z").unwrap();
        let x06 = Value::from_str("8'h1x").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'h11111111111111111").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x04 = Value::from_str("68'h0000000000000000x").unwrap();
        let x05 = Value::from_str("68'h0000000000000000z").unwrap();
        let x06 = Value::from_str("68'h1111111111111111x").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn unary_logical_not() {
        //x = !8'h00; $display("%b", x); // 0000000000000001
        //x = !8'h01; $display("%b", x); // 0000000000000000
        //x = !8'hxx; $display("%b", x); // 000000000000000x
        //x = !8'hzz; $display("%b", x); // 000000000000000x
        //x = !8'h0x; $display("%b", x); // 000000000000000x
        //x = !8'h0z; $display("%b", x); // 000000000000000x
        //x = !8'h1x; $display("%b", x); // 0000000000000000
        //x = !8'h1z; $display("%b", x); // 0000000000000000

        let op = Op::LogicNot;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'h01").unwrap();
        let x02 = Value::from_str("8'hxx").unwrap();
        let x03 = Value::from_str("8'hzz").unwrap();
        let x04 = Value::from_str("8'h0x").unwrap();
        let x05 = Value::from_str("8'h0z").unwrap();
        let x06 = Value::from_str("8'h1x").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x00 = op.eval_unary(&x00, Some(16), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(16), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(16), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(16), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(16), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(16), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(16), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x06), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x07), "16'b0000000000000000");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'h11111111111111111").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x04 = Value::from_str("68'h0000000000000000x").unwrap();
        let x05 = Value::from_str("68'h0000000000000000z").unwrap();
        let x06 = Value::from_str("68'h1111111111111111x").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x00 = op.eval_unary(&x00, Some(68), false, &mut cache);
        let x01 = op.eval_unary(&x01, Some(68), false, &mut cache);
        let x02 = op.eval_unary(&x02, Some(68), false, &mut cache);
        let x03 = op.eval_unary(&x03, Some(68), false, &mut cache);
        let x04 = op.eval_unary(&x04, Some(68), false, &mut cache);
        let x05 = op.eval_unary(&x05, Some(68), false, &mut cache);
        let x06 = op.eval_unary(&x06, Some(68), false, &mut cache);
        let x07 = op.eval_unary(&x07, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x06), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x07), "68'h00000000000000000");
    }

    #[test]
    fn binary_add() {
        //x = 8'h01  + 8'h01 ; $display("%b", x); // 0000000000000010
        //x = 8'hf2  + 8'hf2 ; $display("%b", x); // 0000000111100100
        //x = 8'hx3  + 8'hx3 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'hz4  + 8'hz4 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'sh01 + 8'sh01; $display("%b", x); // 0000000000000010
        //x = 8'shf2 + 8'shf2; $display("%b", x); // 1111111111100100
        //x = 8'shx3 + 8'shx3; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'shz4 + 8'shz4; $display("%b", x); // xxxxxxxxxxxxxxxx

        let op = Op::Add;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h01").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh01").unwrap();
        let x05 = Value::from_str("8'shf2").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'hf2").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'shf2").unwrap();
        let x16 = Value::from_str("8'shx3").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000010");
        assert_eq!(format!("{:b}", x01), "16'b0000000111100100");
        assert_eq!(format!("{:b}", x02), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x03), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000000010");
        assert_eq!(format!("{:b}", x05), "16'sb1111111111100100");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x07), "16'sbxxxxxxxxxxxxxxxx");

        let x00 = Value::from_str("8'h01").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh01").unwrap();
        let x05 = Value::from_str("8'shf2").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'hf2").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'shf2").unwrap();
        let x16 = Value::from_str("8'shx3").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000002");
        assert_eq!(format!("{:x}", x01), "68'h000000000000001e4");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x04), "68'sh00000000000000002");
        assert_eq!(format!("{:x}", x05), "68'shfffffffffffffffe4");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x07), "68'shxxxxxxxxxxxxxxxxx");
    }

    #[test]
    fn binary_sub() {
        //x = 8'h01  - 8'hf2 ; $display("%b", x); // 1111111100001111
        //x = 8'hf2  - 8'h03 ; $display("%b", x); // 0000000011101111
        //x = 8'hx3  - 8'hx4 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'hz4  - 8'hz5 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'sh01 - 8'shf2; $display("%b", x); // 0000000000001111
        //x = 8'shf2 - 8'sh03; $display("%b", x); // 1111111111101111
        //x = 8'shx3 - 8'shx4; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'shz4 - 8'shz5; $display("%b", x); // xxxxxxxxxxxxxxxx

        let op = Op::Sub;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h01").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh01").unwrap();
        let x05 = Value::from_str("8'shf2").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'hf2").unwrap();
        let x11 = Value::from_str("8'h03").unwrap();
        let x12 = Value::from_str("8'hx4").unwrap();
        let x13 = Value::from_str("8'hz5").unwrap();
        let x14 = Value::from_str("8'shf2").unwrap();
        let x15 = Value::from_str("8'sh03").unwrap();
        let x16 = Value::from_str("8'shx4").unwrap();
        let x17 = Value::from_str("8'shz5").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b1111111100001111");
        assert_eq!(format!("{:b}", x01), "16'b0000000011101111");
        assert_eq!(format!("{:b}", x02), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x03), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000001111");
        assert_eq!(format!("{:b}", x05), "16'sb1111111111101111");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x07), "16'sbxxxxxxxxxxxxxxxx");

        let x00 = Value::from_str("8'h01").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh01").unwrap();
        let x05 = Value::from_str("8'shf2").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'hf2").unwrap();
        let x11 = Value::from_str("8'h03").unwrap();
        let x12 = Value::from_str("8'hx4").unwrap();
        let x13 = Value::from_str("8'hz5").unwrap();
        let x14 = Value::from_str("8'shf2").unwrap();
        let x15 = Value::from_str("8'sh03").unwrap();
        let x16 = Value::from_str("8'shx4").unwrap();
        let x17 = Value::from_str("8'shz5").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'hfffffffffffffff0f");
        assert_eq!(format!("{:x}", x01), "68'h000000000000000ef");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x04), "68'sh0000000000000000f");
        assert_eq!(format!("{:x}", x05), "68'shfffffffffffffffef");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x07), "68'shxxxxxxxxxxxxxxxxx");
    }

    #[test]
    fn binary_mul() {
        //x = 8'h01  * 8'h01 ; $display("%b", x); // 0000000000000001
        //x = 8'hf2  * 8'hf2 ; $display("%b", x); // 1110010011000100
        //x = 8'hx3  * 8'hx3 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'hz4  * 8'hz4 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'sh01 * 8'sh01; $display("%b", x); // 0000000000000001
        //x = 8'shf2 * 8'shf2; $display("%b", x); // 0000000011000100
        //x = 8'shf3 * 8'sh03; $display("%b", x); // 1111111111011001
        //x = 8'shz4 * 8'shz4; $display("%b", x); // xxxxxxxxxxxxxxxx

        let op = Op::Mul;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h01").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh01").unwrap();
        let x05 = Value::from_str("8'shf2").unwrap();
        let x06 = Value::from_str("8'shf3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'hf2").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'shf2").unwrap();
        let x16 = Value::from_str("8'sh03").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b1110010011000100");
        assert_eq!(format!("{:b}", x02), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x03), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000000001");
        assert_eq!(format!("{:b}", x05), "16'sb0000000011000100");
        assert_eq!(format!("{:b}", x06), "16'sb1111111111011001");
        assert_eq!(format!("{:b}", x07), "16'sbxxxxxxxxxxxxxxxx");

        let x00 = Value::from_str("8'h01").unwrap();
        let x01 = Value::from_str("8'hf2").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh01").unwrap();
        let x05 = Value::from_str("8'shf2").unwrap();
        let x06 = Value::from_str("8'shf3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'hf2").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'shf2").unwrap();
        let x16 = Value::from_str("8'sh03").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h0000000000000e4c4");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x04), "68'sh00000000000000001");
        assert_eq!(format!("{:x}", x05), "68'sh000000000000000c4");
        assert_eq!(format!("{:x}", x06), "68'shfffffffffffffffd9");
        assert_eq!(format!("{:x}", x07), "68'shxxxxxxxxxxxxxxxxx");
    }

    #[test]
    fn binary_div() {
        //x = 8'h02  / 8'h01 ; $display("%b", x); // 0000000000000010
        //x = 8'hf0  / 8'h02 ; $display("%b", x); // 0000000001111000
        //x = 8'hx3  / 8'hx3 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'hz4  / 8'hz4 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'sh02 / 8'sh01; $display("%b", x); // 0000000000000010
        //x = 8'shf0 / 8'sh02; $display("%b", x); // 1111111111111000
        //x = 8'shf3 / 8'shf3; $display("%b", x); // 0000000000000001
        //x = 8'sh01 / 8'sh00; $display("%b", x); // xxxxxxxxxxxxxxxx

        let op = Op::Div;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h02").unwrap();
        let x01 = Value::from_str("8'hf0").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh02").unwrap();
        let x05 = Value::from_str("8'shf0").unwrap();
        let x06 = Value::from_str("8'shf3").unwrap();
        let x07 = Value::from_str("8'sh01").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shf3").unwrap();
        let x17 = Value::from_str("8'sh00").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000010");
        assert_eq!(format!("{:b}", x01), "16'b0000000001111000");
        assert_eq!(format!("{:b}", x02), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x03), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000000010");
        assert_eq!(format!("{:b}", x05), "16'sb1111111111111000");
        assert_eq!(format!("{:b}", x06), "16'sb0000000000000001");
        assert_eq!(format!("{:b}", x07), "16'sbxxxxxxxxxxxxxxxx");

        let x00 = Value::from_str("8'h02").unwrap();
        let x01 = Value::from_str("8'hf0").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh02").unwrap();
        let x05 = Value::from_str("8'shf0").unwrap();
        let x06 = Value::from_str("8'shf3").unwrap();
        let x07 = Value::from_str("8'sh01").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shf3").unwrap();
        let x17 = Value::from_str("8'sh00").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000002");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000078");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x04), "68'sh00000000000000002");
        assert_eq!(format!("{:x}", x05), "68'shffffffffffffffff8");
        assert_eq!(format!("{:x}", x06), "68'sh00000000000000001");
        assert_eq!(format!("{:x}", x07), "68'shxxxxxxxxxxxxxxxxx");
    }

    #[test]
    fn binary_rem() {
        //x = 8'h03  % 8'h01 ; $display("%b", x); // 0000000000000000
        //x = 8'hf1  % 8'h02 ; $display("%b", x); // 0000000000000001
        //x = 8'hx3  % 8'hx3 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'hz4  % 8'hz4 ; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'sh03 % 8'sh02; $display("%b", x); // 0000000000000001
        //x = 8'shf1 % 8'sh02; $display("%b", x); // 1111111111111111
        //x = 8'shf1 % 8'shfc; $display("%b", x); // 1111111111111101
        //x = 8'sh03 % 8'shfc; $display("%b", x); // 0000000000000011

        let op = Op::Rem;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shf1").unwrap();
        let x07 = Value::from_str("8'sh03").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh02").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shfc").unwrap();
        let x17 = Value::from_str("8'shfc").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x03), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000000001");
        assert_eq!(format!("{:b}", x05), "16'sb1111111111111111");
        assert_eq!(format!("{:b}", x06), "16'sb1111111111111101");
        assert_eq!(format!("{:b}", x07), "16'sb0000000000000011");

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shf1").unwrap();
        let x07 = Value::from_str("8'sh03").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh02").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shfc").unwrap();
        let x17 = Value::from_str("8'shfc").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x04), "68'sh00000000000000001");
        assert_eq!(format!("{:x}", x05), "68'shfffffffffffffffff");
        assert_eq!(format!("{:x}", x06), "68'shffffffffffffffffd");
        assert_eq!(format!("{:x}", x07), "68'sh00000000000000003");
    }

    #[test]
    fn binary_bit_and() {
        //x = 8'hf3 & 8'hc1; $display("%b", x); // 0000000011000001
        //x = 8'hf1 & 8'he2; $display("%b", x); // 0000000011100000
        //x = 8'hx1 & 8'hx2; $display("%b", x); // 00000000xxxx0000
        //x = 8'hz3 & 8'hz7; $display("%b", x); // 00000000xxxx0011
        //x = 8'h13 & 8'hx1; $display("%b", x); // 00000000000x0001
        //x = 8'h11 & 8'hz2; $display("%b", x); // 00000000000x0000
        //x = 8'hx1 & 8'hzd; $display("%b", x); // 00000000xxxx0001
        //x = 8'h1z & 8'hfx; $display("%b", x); // 000000000001xxxx

        let op = Op::BitAnd;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'hf3").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx1").unwrap();
        let x03 = Value::from_str("8'hz3").unwrap();
        let x04 = Value::from_str("8'h13").unwrap();
        let x05 = Value::from_str("8'h11").unwrap();
        let x06 = Value::from_str("8'hx1").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x10 = Value::from_str("8'hc1").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'hx2").unwrap();
        let x13 = Value::from_str("8'hz7").unwrap();
        let x14 = Value::from_str("8'hx1").unwrap();
        let x15 = Value::from_str("8'hz2").unwrap();
        let x16 = Value::from_str("8'hzd").unwrap();
        let x17 = Value::from_str("8'hfx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000011000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000011100000");
        assert_eq!(format!("{:b}", x02), "16'b00000000xxxx0000");
        assert_eq!(format!("{:b}", x03), "16'b00000000xxxx0011");
        assert_eq!(format!("{:b}", x04), "16'b00000000000x0001");
        assert_eq!(format!("{:b}", x05), "16'b00000000000x0000");
        assert_eq!(format!("{:b}", x06), "16'b00000000xxxx0001");
        assert_eq!(format!("{:b}", x07), "16'b000000000001xxxx");

        let x00 = Value::from_str("68'hffffffffffffffff3").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz3").unwrap();
        let x04 = Value::from_str("68'h11111111111111113").unwrap();
        let x05 = Value::from_str("68'h11111111111111111").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x10 = Value::from_str("68'hcccccccccccccccc1").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx2").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz7").unwrap();
        let x14 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x15 = Value::from_str("68'hzzzzzzzzzzzzzzzz2").unwrap();
        let x16 = Value::from_str("68'hzzzzzzzzzzzzzzzzd").unwrap();
        let x17 = Value::from_str("68'hffffffffffffffffx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'hcccccccccccccccc1");
        assert_eq!(format!("{:x}", x01), "68'heeeeeeeeeeeeeeee0");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxx0");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxx3");
        assert_eq!(format!("{:x}", x04), "68'hXXXXXXXXXXXXXXXX1");
        assert_eq!(format!("{:x}", x05), "68'hXXXXXXXXXXXXXXXX0");
        assert_eq!(format!("{:x}", x06), "68'hxxxxxxxxxxxxxxxx1");
        assert_eq!(format!("{:x}", x07), "68'h1111111111111111x");
    }

    #[test]
    fn binary_bit_or() {
        //x = 8'hf3 | 8'hc1; $display("%b", x); // 0000000011110011
        //x = 8'hf1 | 8'he2; $display("%b", x); // 0000000011110011
        //x = 8'hx1 | 8'hx2; $display("%b", x); // 00000000xxxx0011
        //x = 8'hz3 | 8'hz7; $display("%b", x); // 00000000xxxx0111
        //x = 8'h13 | 8'hx1; $display("%b", x); // 00000000xxx10011
        //x = 8'h11 | 8'hz2; $display("%b", x); // 00000000xxx10011
        //x = 8'hx1 | 8'hzd; $display("%b", x); // 00000000xxxx1101
        //x = 8'h1z | 8'hfx; $display("%b", x); // 000000001111xxxx

        let op = Op::BitOr;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'hf3").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx1").unwrap();
        let x03 = Value::from_str("8'hz3").unwrap();
        let x04 = Value::from_str("8'h13").unwrap();
        let x05 = Value::from_str("8'h11").unwrap();
        let x06 = Value::from_str("8'hx1").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x10 = Value::from_str("8'hc1").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'hx2").unwrap();
        let x13 = Value::from_str("8'hz7").unwrap();
        let x14 = Value::from_str("8'hx1").unwrap();
        let x15 = Value::from_str("8'hz2").unwrap();
        let x16 = Value::from_str("8'hzd").unwrap();
        let x17 = Value::from_str("8'hfx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000011110011");
        assert_eq!(format!("{:b}", x01), "16'b0000000011110011");
        assert_eq!(format!("{:b}", x02), "16'b00000000xxxx0011");
        assert_eq!(format!("{:b}", x03), "16'b00000000xxxx0111");
        assert_eq!(format!("{:b}", x04), "16'b00000000xxx10011");
        assert_eq!(format!("{:b}", x05), "16'b00000000xxx10011");
        assert_eq!(format!("{:b}", x06), "16'b00000000xxxx1101");
        assert_eq!(format!("{:b}", x07), "16'b000000001111xxxx");

        let x00 = Value::from_str("68'hffffffffffffffff3").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz3").unwrap();
        let x04 = Value::from_str("68'h11111111111111113").unwrap();
        let x05 = Value::from_str("68'h11111111111111111").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x10 = Value::from_str("68'hcccccccccccccccc1").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx2").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz7").unwrap();
        let x14 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x15 = Value::from_str("68'hzzzzzzzzzzzzzzzz2").unwrap();
        let x16 = Value::from_str("68'hzzzzzzzzzzzzzzzzd").unwrap();
        let x17 = Value::from_str("68'hffffffffffffffffx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'hffffffffffffffff3");
        assert_eq!(format!("{:x}", x01), "68'hffffffffffffffff3");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxx3");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxx7");
        assert_eq!(format!("{:x}", x04), "68'hXXXXXXXXXXXXXXXX3");
        assert_eq!(format!("{:x}", x05), "68'hXXXXXXXXXXXXXXXX3");
        assert_eq!(format!("{:x}", x06), "68'hxxxxxxxxxxxxxxxxd");
        assert_eq!(format!("{:x}", x07), "68'hffffffffffffffffx");
    }

    #[test]
    fn binary_bit_xor() {
        //x = 8'hf3 ^ 8'hc1; $display("%b", x); // 0000000000110010
        //x = 8'hf1 ^ 8'he2; $display("%b", x); // 0000000000010011
        //x = 8'hx1 ^ 8'hx2; $display("%b", x); // 00000000xxxx0011
        //x = 8'hz3 ^ 8'hz7; $display("%b", x); // 00000000xxxx0100
        //x = 8'h13 ^ 8'hx1; $display("%b", x); // 00000000xxxx0010
        //x = 8'h11 ^ 8'hz2; $display("%b", x); // 00000000xxxx0011
        //x = 8'hx1 ^ 8'hzd; $display("%b", x); // 00000000xxxx1100
        //x = 8'h1z ^ 8'hfx; $display("%b", x); // 000000001110xxxx

        let op = Op::BitXor;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'hf3").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx1").unwrap();
        let x03 = Value::from_str("8'hz3").unwrap();
        let x04 = Value::from_str("8'h13").unwrap();
        let x05 = Value::from_str("8'h11").unwrap();
        let x06 = Value::from_str("8'hx1").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x10 = Value::from_str("8'hc1").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'hx2").unwrap();
        let x13 = Value::from_str("8'hz7").unwrap();
        let x14 = Value::from_str("8'hx1").unwrap();
        let x15 = Value::from_str("8'hz2").unwrap();
        let x16 = Value::from_str("8'hzd").unwrap();
        let x17 = Value::from_str("8'hfx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000110010");
        assert_eq!(format!("{:b}", x01), "16'b0000000000010011");
        assert_eq!(format!("{:b}", x02), "16'b00000000xxxx0011");
        assert_eq!(format!("{:b}", x03), "16'b00000000xxxx0100");
        assert_eq!(format!("{:b}", x04), "16'b00000000xxxx0010");
        assert_eq!(format!("{:b}", x05), "16'b00000000xxxx0011");
        assert_eq!(format!("{:b}", x06), "16'b00000000xxxx1100");
        assert_eq!(format!("{:b}", x07), "16'b000000001110xxxx");

        let x00 = Value::from_str("68'hffffffffffffffff3").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz3").unwrap();
        let x04 = Value::from_str("68'h11111111111111113").unwrap();
        let x05 = Value::from_str("68'h11111111111111111").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x10 = Value::from_str("68'hcccccccccccccccc1").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx2").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz7").unwrap();
        let x14 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x15 = Value::from_str("68'hzzzzzzzzzzzzzzzz2").unwrap();
        let x16 = Value::from_str("68'hzzzzzzzzzzzzzzzzd").unwrap();
        let x17 = Value::from_str("68'hffffffffffffffffx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h33333333333333332");
        assert_eq!(format!("{:x}", x01), "68'h11111111111111113");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxx3");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxx4");
        assert_eq!(format!("{:x}", x04), "68'hxxxxxxxxxxxxxxxx2");
        assert_eq!(format!("{:x}", x05), "68'hxxxxxxxxxxxxxxxx3");
        assert_eq!(format!("{:x}", x06), "68'hxxxxxxxxxxxxxxxxc");
        assert_eq!(format!("{:x}", x07), "68'heeeeeeeeeeeeeeeex");
    }

    #[test]
    fn binary_bit_xnor() {
        //x = 8'hf3 ~^ 8'hc1; $display("%b", x); // 1111111111001101
        //x = 8'hf1 ~^ 8'he2; $display("%b", x); // 1111111111101100
        //x = 8'hx1 ~^ 8'hx2; $display("%b", x); // 11111111xxxx1100
        //x = 8'hz3 ~^ 8'hz7; $display("%b", x); // 11111111xxxx1011
        //x = 8'h13 ~^ 8'hx1; $display("%b", x); // 11111111xxxx1101
        //x = 8'h11 ~^ 8'hz2; $display("%b", x); // 11111111xxxx1100
        //x = 8'hx1 ~^ 8'hzd; $display("%b", x); // 11111111xxxx0011
        //x = 8'h1z ~^ 8'hfx; $display("%b", x); // 111111110001xxxx

        let op = Op::BitXnor;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'hf3").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx1").unwrap();
        let x03 = Value::from_str("8'hz3").unwrap();
        let x04 = Value::from_str("8'h13").unwrap();
        let x05 = Value::from_str("8'h11").unwrap();
        let x06 = Value::from_str("8'hx1").unwrap();
        let x07 = Value::from_str("8'h1z").unwrap();
        let x10 = Value::from_str("8'hc1").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'hx2").unwrap();
        let x13 = Value::from_str("8'hz7").unwrap();
        let x14 = Value::from_str("8'hx1").unwrap();
        let x15 = Value::from_str("8'hz2").unwrap();
        let x16 = Value::from_str("8'hzd").unwrap();
        let x17 = Value::from_str("8'hfx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b1111111111001101");
        assert_eq!(format!("{:b}", x01), "16'b1111111111101100");
        assert_eq!(format!("{:b}", x02), "16'b11111111xxxx1100");
        assert_eq!(format!("{:b}", x03), "16'b11111111xxxx1011");
        assert_eq!(format!("{:b}", x04), "16'b11111111xxxx1101");
        assert_eq!(format!("{:b}", x05), "16'b11111111xxxx1100");
        assert_eq!(format!("{:b}", x06), "16'b11111111xxxx0011");
        assert_eq!(format!("{:b}", x07), "16'b111111110001xxxx");

        let x00 = Value::from_str("68'hffffffffffffffff3").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz3").unwrap();
        let x04 = Value::from_str("68'h11111111111111113").unwrap();
        let x05 = Value::from_str("68'h11111111111111111").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x07 = Value::from_str("68'h1111111111111111z").unwrap();
        let x10 = Value::from_str("68'hcccccccccccccccc1").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx2").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz7").unwrap();
        let x14 = Value::from_str("68'hxxxxxxxxxxxxxxxx1").unwrap();
        let x15 = Value::from_str("68'hzzzzzzzzzzzzzzzz2").unwrap();
        let x16 = Value::from_str("68'hzzzzzzzzzzzzzzzzd").unwrap();
        let x17 = Value::from_str("68'hffffffffffffffffx").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'hccccccccccccccccd");
        assert_eq!(format!("{:x}", x01), "68'heeeeeeeeeeeeeeeec");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxc");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxb");
        assert_eq!(format!("{:x}", x04), "68'hxxxxxxxxxxxxxxxxd");
        assert_eq!(format!("{:x}", x05), "68'hxxxxxxxxxxxxxxxxc");
        assert_eq!(format!("{:x}", x06), "68'hxxxxxxxxxxxxxxxx3");
        assert_eq!(format!("{:x}", x07), "68'h1111111111111111x");
    }

    #[test]
    fn binary_eq() {
        //x = 8'h00 == 8'h00; $display("%b", x); // 0000000000000001
        //x = 8'hf1 == 8'he2; $display("%b", x); // 0000000000000000
        //x = 8'hx0 == 8'hx0; $display("%b", x); // 000000000000000x
        //x = 8'hx3 == 8'hx7; $display("%b", x); // 0000000000000000
        //x = 8'hz0 == 8'hz0; $display("%b", x); // 000000000000000x
        //x = 8'hz1 == 8'hz2; $display("%b", x); // 0000000000000000
        //x = 8'hxz == 8'hxz; $display("%b", x); // 000000000000000x
        //x = 8'hzx == 8'hxz; $display("%b", x); // 000000000000000x

        let op = Op::Eq;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx0").unwrap();
        let x03 = Value::from_str("8'hx3").unwrap();
        let x04 = Value::from_str("8'hz0").unwrap();
        let x05 = Value::from_str("8'hz1").unwrap();
        let x06 = Value::from_str("8'hxz").unwrap();
        let x07 = Value::from_str("8'hzx").unwrap();
        let x10 = Value::from_str("8'h00").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'hx0").unwrap();
        let x13 = Value::from_str("8'hx7").unwrap();
        let x14 = Value::from_str("8'hz0").unwrap();
        let x15 = Value::from_str("8'hz2").unwrap();
        let x16 = Value::from_str("8'hxz").unwrap();
        let x17 = Value::from_str("8'hxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x03 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x04 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x05 = Value::from_str("68'hzzzzzzzzzzzzzzzz1").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x07 = Value::from_str("68'hzzzzzzzzzzzzzzzzx").unwrap();
        let x10 = Value::from_str("68'h00000000000000000").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x13 = Value::from_str("68'hxxxxxxxxxxxxxxxx7").unwrap();
        let x14 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x15 = Value::from_str("68'hzzzzzzzzzzzzzzzz2").unwrap();
        let x16 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x17 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn binary_ne() {
        //x = 8'h00 != 8'h00; $display("%b", x); // 0000000000000000
        //x = 8'hf1 != 8'he2; $display("%b", x); // 0000000000000001
        //x = 8'hx0 != 8'hx0; $display("%b", x); // 000000000000000x
        //x = 8'hx3 != 8'hx7; $display("%b", x); // 0000000000000001
        //x = 8'hz0 != 8'hz0; $display("%b", x); // 000000000000000x
        //x = 8'hz1 != 8'hz2; $display("%b", x); // 0000000000000001
        //x = 8'hxz != 8'hxz; $display("%b", x); // 000000000000000x
        //x = 8'hzx != 8'hxz; $display("%b", x); // 000000000000000x

        let op = Op::Ne;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx0").unwrap();
        let x03 = Value::from_str("8'hx3").unwrap();
        let x04 = Value::from_str("8'hz0").unwrap();
        let x05 = Value::from_str("8'hz1").unwrap();
        let x06 = Value::from_str("8'hxz").unwrap();
        let x07 = Value::from_str("8'hzx").unwrap();
        let x10 = Value::from_str("8'h00").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'hx0").unwrap();
        let x13 = Value::from_str("8'hx7").unwrap();
        let x14 = Value::from_str("8'hz0").unwrap();
        let x15 = Value::from_str("8'hz2").unwrap();
        let x16 = Value::from_str("8'hxz").unwrap();
        let x17 = Value::from_str("8'hxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x03 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x04 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x05 = Value::from_str("68'hzzzzzzzzzzzzzzzz1").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x07 = Value::from_str("68'hzzzzzzzzzzzzzzzzx").unwrap();
        let x10 = Value::from_str("68'h00000000000000000").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x13 = Value::from_str("68'hxxxxxxxxxxxxxxxx7").unwrap();
        let x14 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x15 = Value::from_str("68'hzzzzzzzzzzzzzzzz2").unwrap();
        let x16 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x17 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn binary_eq_wildcard() {
        //x = 8'h00 ==? 8'h00; $display("%b", x); // 0000000000000001
        //x = 8'hf1 ==? 8'he2; $display("%b", x); // 0000000000000000
        //x = 8'hx0 ==? 8'h30; $display("%b", x); // 000000000000000x
        //x = 8'h43 ==? 8'h4x; $display("%b", x); // 0000000000000001
        //x = 8'hz0 ==? 8'h30; $display("%b", x); // 000000000000000x
        //x = 8'h11 ==? 8'h1z; $display("%b", x); // 0000000000000001
        //x = 8'hxz ==? 8'hxz; $display("%b", x); // 0000000000000001
        //x = 8'hzx ==? 8'hxz; $display("%b", x); // 0000000000000001

        let op = Op::EqWildcard;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx0").unwrap();
        let x03 = Value::from_str("8'h43").unwrap();
        let x04 = Value::from_str("8'hz0").unwrap();
        let x05 = Value::from_str("8'h11").unwrap();
        let x06 = Value::from_str("8'hxz").unwrap();
        let x07 = Value::from_str("8'hzx").unwrap();
        let x10 = Value::from_str("8'h00").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'h30").unwrap();
        let x13 = Value::from_str("8'h4x").unwrap();
        let x14 = Value::from_str("8'h30").unwrap();
        let x15 = Value::from_str("8'h1z").unwrap();
        let x16 = Value::from_str("8'hxz").unwrap();
        let x17 = Value::from_str("8'hxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x06), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x07), "16'b0000000000000001");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x03 = Value::from_str("68'h44444444444444443").unwrap();
        let x04 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x05 = Value::from_str("68'h11111111111111111").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x07 = Value::from_str("68'hzzzzzzzzzzzzzzzzx").unwrap();
        let x10 = Value::from_str("68'h00000000000000000").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'h33333333333333330").unwrap();
        let x13 = Value::from_str("68'h4444444444444444x").unwrap();
        let x14 = Value::from_str("68'h33333333333333330").unwrap();
        let x15 = Value::from_str("68'h1111111111111111z").unwrap();
        let x16 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x17 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x06), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x07), "68'h00000000000000001");
    }

    #[test]
    fn binary_ne_wildcard() {
        //x = 8'h00 !=? 8'h00; $display("%b", x); // 0000000000000000
        //x = 8'hf1 !=? 8'he2; $display("%b", x); // 0000000000000001
        //x = 8'hx0 !=? 8'h30; $display("%b", x); // 000000000000000x
        //x = 8'h43 !=? 8'h4x; $display("%b", x); // 0000000000000000
        //x = 8'hz0 !=? 8'h30; $display("%b", x); // 000000000000000x
        //x = 8'h11 !=? 8'h1z; $display("%b", x); // 0000000000000000
        //x = 8'hxz !=? 8'hxz; $display("%b", x); // 0000000000000000
        //x = 8'hzx !=? 8'hxz; $display("%b", x); // 0000000000000000

        let op = Op::NeWildcard;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h00").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx0").unwrap();
        let x03 = Value::from_str("8'h43").unwrap();
        let x04 = Value::from_str("8'hz0").unwrap();
        let x05 = Value::from_str("8'h11").unwrap();
        let x06 = Value::from_str("8'hxz").unwrap();
        let x07 = Value::from_str("8'hzx").unwrap();
        let x10 = Value::from_str("8'h00").unwrap();
        let x11 = Value::from_str("8'he2").unwrap();
        let x12 = Value::from_str("8'h30").unwrap();
        let x13 = Value::from_str("8'h4x").unwrap();
        let x14 = Value::from_str("8'h30").unwrap();
        let x15 = Value::from_str("8'h1z").unwrap();
        let x16 = Value::from_str("8'hxz").unwrap();
        let x17 = Value::from_str("8'hxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x06), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x07), "16'b0000000000000000");

        let x00 = Value::from_str("68'h00000000000000000").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x03 = Value::from_str("68'h44444444444444443").unwrap();
        let x04 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x05 = Value::from_str("68'h11111111111111111").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x07 = Value::from_str("68'hzzzzzzzzzzzzzzzzx").unwrap();
        let x10 = Value::from_str("68'h00000000000000000").unwrap();
        let x11 = Value::from_str("68'heeeeeeeeeeeeeeee2").unwrap();
        let x12 = Value::from_str("68'h33333333333333330").unwrap();
        let x13 = Value::from_str("68'h4444444444444444x").unwrap();
        let x14 = Value::from_str("68'h33333333333333330").unwrap();
        let x15 = Value::from_str("68'h1111111111111111z").unwrap();
        let x16 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x17 = Value::from_str("68'hxxxxxxxxxxxxxxxxz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x06), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x07), "68'h00000000000000000");
    }

    #[test]
    fn binary_greater() {
        //x = 8'h03  > 8'h01 ; $display("%b", x); // 0000000000000001
        //x = 8'hf1  > 8'h02 ; $display("%b", x); // 0000000000000001
        //x = 8'hx3  > 8'hx3 ; $display("%b", x); // 000000000000000x
        //x = 8'hz4  > 8'hz4 ; $display("%b", x); // 000000000000000x
        //x = 8'sh03 > 8'sh01; $display("%b", x); // 0000000000000001
        //x = 8'shf1 > 8'sh02; $display("%b", x); // 0000000000000000
        //x = 8'shx3 > 8'shx3; $display("%b", x); // 000000000000000x
        //x = 8'shz4 > 8'shz4; $display("%b", x); // 000000000000000x

        let op = Op::Greater;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shx3").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("68'h00000000000000001").unwrap();
        let x11 = Value::from_str("68'h00000000000000002").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x14 = Value::from_str("68'sh00000000000000001").unwrap();
        let x15 = Value::from_str("68'sh00000000000000002").unwrap();
        let x16 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x17 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn binary_greater_eq() {
        //x = 8'h03  >= 8'h01 ; $display("%b", x); // 0000000000000001
        //x = 8'hf1  >= 8'h02 ; $display("%b", x); // 0000000000000001
        //x = 8'hx3  >= 8'hx3 ; $display("%b", x); // 000000000000000x
        //x = 8'hz4  >= 8'hz4 ; $display("%b", x); // 000000000000000x
        //x = 8'sh03 >= 8'sh01; $display("%b", x); // 0000000000000001
        //x = 8'shf1 >= 8'sh02; $display("%b", x); // 0000000000000000
        //x = 8'shx3 >= 8'shx3; $display("%b", x); // 000000000000000x
        //x = 8'shz4 >= 8'shz4; $display("%b", x); // 000000000000000x

        let op = Op::GreaterEq;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shx3").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("68'h00000000000000001").unwrap();
        let x11 = Value::from_str("68'h00000000000000002").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x14 = Value::from_str("68'sh00000000000000001").unwrap();
        let x15 = Value::from_str("68'sh00000000000000002").unwrap();
        let x16 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x17 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn binary_less() {
        //x = 8'h03  < 8'h01 ; $display("%b", x); // 0000000000000000
        //x = 8'hf1  < 8'h02 ; $display("%b", x); // 0000000000000000
        //x = 8'hx3  < 8'hx3 ; $display("%b", x); // 000000000000000x
        //x = 8'hz4  < 8'hz4 ; $display("%b", x); // 000000000000000x
        //x = 8'sh03 < 8'sh01; $display("%b", x); // 0000000000000000
        //x = 8'shf1 < 8'sh02; $display("%b", x); // 0000000000000001
        //x = 8'shx3 < 8'shx3; $display("%b", x); // 000000000000000x
        //x = 8'shz4 < 8'shz4; $display("%b", x); // 000000000000000x

        let op = Op::Less;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shx3").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("68'h00000000000000001").unwrap();
        let x11 = Value::from_str("68'h00000000000000002").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x14 = Value::from_str("68'sh00000000000000001").unwrap();
        let x15 = Value::from_str("68'sh00000000000000002").unwrap();
        let x16 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x17 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn binary_less_eq() {
        //x = 8'h03  <= 8'h01 ; $display("%b", x); // 0000000000000000
        //x = 8'hf1  <= 8'h02 ; $display("%b", x); // 0000000000000000
        //x = 8'hx3  <= 8'hx3 ; $display("%b", x); // 000000000000000x
        //x = 8'hz4  <= 8'hz4 ; $display("%b", x); // 000000000000000x
        //x = 8'sh03 <= 8'sh01; $display("%b", x); // 0000000000000000
        //x = 8'shf1 <= 8'sh02; $display("%b", x); // 0000000000000001
        //x = 8'shx3 <= 8'shx3; $display("%b", x); // 000000000000000x
        //x = 8'shz4 <= 8'shz4; $display("%b", x); // 000000000000000x

        let op = Op::LessEq;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h02").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'sh01").unwrap();
        let x15 = Value::from_str("8'sh02").unwrap();
        let x16 = Value::from_str("8'shx3").unwrap();
        let x17 = Value::from_str("8'shz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("68'h00000000000000001").unwrap();
        let x11 = Value::from_str("68'h00000000000000002").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x14 = Value::from_str("68'sh00000000000000001").unwrap();
        let x15 = Value::from_str("68'sh00000000000000002").unwrap();
        let x16 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x17 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn binary_logic_and() {
        //x = 8'h03 && 8'h01; $display("%b", x); // 0000000000000001
        //x = 8'hf1 && 8'h00; $display("%b", x); // 0000000000000000
        //x = 8'hx3 && 8'hx3; $display("%b", x); // 0000000000000001
        //x = 8'hz4 && 8'hz4; $display("%b", x); // 0000000000000001
        //x = 8'h0x && 8'h01; $display("%b", x); // 000000000000000x
        //x = 8'hf1 && 8'h0z; $display("%b", x); // 000000000000000x
        //x = 8'hxx && 8'hx3; $display("%b", x); // 000000000000000x
        //x = 8'hz4 && 8'hzz; $display("%b", x); // 000000000000000x

        let op = Op::LogicAnd;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'h0x").unwrap();
        let x05 = Value::from_str("8'hf1").unwrap();
        let x06 = Value::from_str("8'hxx").unwrap();
        let x07 = Value::from_str("8'hz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h00").unwrap();
        let x12 = Value::from_str("8'hx3").unwrap();
        let x13 = Value::from_str("8'hz4").unwrap();
        let x14 = Value::from_str("8'h01").unwrap();
        let x15 = Value::from_str("8'h0z").unwrap();
        let x16 = Value::from_str("8'hx3").unwrap();
        let x17 = Value::from_str("8'hzz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x03), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x06), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x07), "16'b000000000000000x");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'h0000000000000000x").unwrap();
        let x05 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x07 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("68'h00000000000000001").unwrap();
        let x11 = Value::from_str("68'h00000000000000000").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x14 = Value::from_str("68'h00000000000000001").unwrap();
        let x15 = Value::from_str("68'h0000000000000000z").unwrap();
        let x16 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x17 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x03), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x06), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x07), "68'h0000000000000000X");
    }

    #[test]
    fn binary_logic_or() {
        //x = 8'h03 || 8'h01; $display("%b", x); // 0000000000000001
        //x = 8'h00 || 8'h00; $display("%b", x); // 0000000000000000
        //x = 8'hx0 || 8'hx0; $display("%b", x); // 000000000000000x
        //x = 8'hz0 || 8'hz0; $display("%b", x); // 000000000000000x
        //x = 8'h0x || 8'h0z; $display("%b", x); // 000000000000000x
        //x = 8'hf1 || 8'h0z; $display("%b", x); // 0000000000000001
        //x = 8'hxx || 8'hx3; $display("%b", x); // 0000000000000001
        //x = 8'hz4 || 8'hzz; $display("%b", x); // 0000000000000001

        let op = Op::LogicOr;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'h00").unwrap();
        let x02 = Value::from_str("8'hx0").unwrap();
        let x03 = Value::from_str("8'hz0").unwrap();
        let x04 = Value::from_str("8'h0x").unwrap();
        let x05 = Value::from_str("8'hf1").unwrap();
        let x06 = Value::from_str("8'hxx").unwrap();
        let x07 = Value::from_str("8'hz4").unwrap();
        let x10 = Value::from_str("8'h01").unwrap();
        let x11 = Value::from_str("8'h00").unwrap();
        let x12 = Value::from_str("8'hx0").unwrap();
        let x13 = Value::from_str("8'hz0").unwrap();
        let x14 = Value::from_str("8'h0z").unwrap();
        let x15 = Value::from_str("8'h0z").unwrap();
        let x16 = Value::from_str("8'hx3").unwrap();
        let x17 = Value::from_str("8'hzz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), false, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x01), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x02), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x03), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x04), "16'b000000000000000x");
        assert_eq!(format!("{:b}", x05), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x06), "16'b0000000000000001");
        assert_eq!(format!("{:b}", x07), "16'b0000000000000001");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'h00000000000000000").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x04 = Value::from_str("68'h0000000000000000x").unwrap();
        let x05 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'hxxxxxxxxxxxxxxxxx").unwrap();
        let x07 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("68'h00000000000000001").unwrap();
        let x11 = Value::from_str("68'h00000000000000000").unwrap();
        let x12 = Value::from_str("68'hxxxxxxxxxxxxxxxx0").unwrap();
        let x13 = Value::from_str("68'hzzzzzzzzzzzzzzzz0").unwrap();
        let x14 = Value::from_str("68'h0000000000000000z").unwrap();
        let x15 = Value::from_str("68'h0000000000000000z").unwrap();
        let x16 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x17 = Value::from_str("68'hzzzzzzzzzzzzzzzzz").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), false, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), false, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), false, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), false, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x01), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x02), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x03), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000X");
        assert_eq!(format!("{:x}", x05), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x06), "68'h00000000000000001");
        assert_eq!(format!("{:x}", x07), "68'h00000000000000001");
    }

    #[test]
    fn binary_logic_shift_r() {
        //x = 8'h03  >> 2; $display("%b", x); // 0000000000000000
        //x = 8'hf1  >> 2; $display("%b", x); // 0000000000111100
        //x = 8'hx3  >> 2; $display("%b", x); // 0000000000xxxx00
        //x = 8'hz4  >> 2; $display("%b", x); // 0000000000zzzz01
        //x = 8'sh03 >> 2; $display("%b", x); // 0000000000000000
        //x = 8'shf1 >> 2; $display("%b", x); // 0011111111111100
        //x = 8'shx3 >> 2; $display("%b", x); // 00xxxxxxxxxxxx00
        //x = 8'shz4 >> 2; $display("%b", x); // 00zzzzzzzzzzzz01

        let op = Op::LogicShiftR;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000111100");
        assert_eq!(format!("{:b}", x02), "16'b0000000000xxxx00");
        assert_eq!(format!("{:b}", x03), "16'b0000000000zzzz01");
        assert_eq!(format!("{:b}", x04), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x05), "16'b0011111111111100");
        assert_eq!(format!("{:b}", x06), "16'b00xxxxxxxxxxxx00");
        assert_eq!(format!("{:b}", x07), "16'b00zzzzzzzzzzzz01");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h3fffffffffffffffc");
        assert_eq!(format!("{:x}", x02), "68'hXxxxxxxxxxxxxxxxX");
        assert_eq!(format!("{:x}", x03), "68'hZzzzzzzzzzzzzzzzZ");
        assert_eq!(format!("{:x}", x04), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x05), "68'h3fffffffffffffffc");
        assert_eq!(format!("{:x}", x06), "68'hXxxxxxxxxxxxxxxxX");
        assert_eq!(format!("{:x}", x07), "68'hZzzzzzzzzzzzzzzzZ");
    }

    #[test]
    fn binary_logic_shift_l() {
        //x = 8'h03  << 2; $display("%b", x); // 0000000000001100
        //x = 8'hf1  << 2; $display("%b", x); // 0000001111000100
        //x = 8'hx3  << 2; $display("%b", x); // 000000xxxx001100
        //x = 8'hz4  << 2; $display("%b", x); // 000000zzzz010000
        //x = 8'sh03 << 2; $display("%b", x); // 0000000000001100
        //x = 8'shf1 << 2; $display("%b", x); // 1111111111000100
        //x = 8'shx3 << 2; $display("%b", x); // xxxxxxxxxx001100
        //x = 8'shz4 << 2; $display("%b", x); // zzzzzzzzzz010000

        let op = Op::LogicShiftL;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000001100");
        assert_eq!(format!("{:b}", x01), "16'b0000001111000100");
        assert_eq!(format!("{:b}", x02), "16'b000000xxxx001100");
        assert_eq!(format!("{:b}", x03), "16'b000000zzzz010000");
        assert_eq!(format!("{:b}", x04), "16'b0000000000001100");
        assert_eq!(format!("{:b}", x05), "16'b1111111111000100");
        assert_eq!(format!("{:b}", x06), "16'bxxxxxxxxxx001100");
        assert_eq!(format!("{:b}", x07), "16'bzzzzzzzzzz010000");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h0000000000000000c");
        assert_eq!(format!("{:x}", x01), "68'hfffffffffffffffc4");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxXc");
        assert_eq!(format!("{:x}", x03), "68'hzzzzzzzzzzzzzzzZ0");
        assert_eq!(format!("{:x}", x04), "68'h0000000000000000c");
        assert_eq!(format!("{:x}", x05), "68'hfffffffffffffffc4");
        assert_eq!(format!("{:x}", x06), "68'hxxxxxxxxxxxxxxxXc");
        assert_eq!(format!("{:x}", x07), "68'hzzzzzzzzzzzzzzzZ0");
    }

    #[test]
    fn binary_arith_shift_r() {
        //x = 8'h03  >>> 2; $display("%b", x); // 0000000000000000
        //x = 8'hf1  >>> 2; $display("%b", x); // 0000000000111100
        //x = 8'hx3  >>> 2; $display("%b", x); // 0000000000xxxx00
        //x = 8'hz4  >>> 2; $display("%b", x); // 0000000000zzzz01
        //x = 8'sh03 >>> 2; $display("%b", x); // 0000000000000000
        //x = 8'shf1 >>> 2; $display("%b", x); // 1111111111111100
        //x = 8'shx3 >>> 2; $display("%b", x); // xxxxxxxxxxxxxx00
        //x = 8'shz4 >>> 2; $display("%b", x); // zzzzzzzzzzzzzz01

        let op = Op::ArithShiftR;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000000000");
        assert_eq!(format!("{:b}", x01), "16'b0000000000111100");
        assert_eq!(format!("{:b}", x02), "16'b0000000000xxxx00");
        assert_eq!(format!("{:b}", x03), "16'b0000000000zzzz01");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000000000");
        assert_eq!(format!("{:b}", x05), "16'sb1111111111111100");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxxxxxx00");
        assert_eq!(format!("{:b}", x07), "16'sbzzzzzzzzzzzzzz01");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000000");
        assert_eq!(format!("{:x}", x01), "68'h3fffffffffffffffc");
        assert_eq!(format!("{:x}", x02), "68'hXxxxxxxxxxxxxxxxX");
        assert_eq!(format!("{:x}", x03), "68'hZzzzzzzzzzzzzzzzZ");
        assert_eq!(format!("{:x}", x04), "68'sh00000000000000000");
        assert_eq!(format!("{:x}", x05), "68'shffffffffffffffffc");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxxX");
        assert_eq!(format!("{:x}", x07), "68'shzzzzzzzzzzzzzzzzZ");
    }

    #[test]
    fn binary_arith_shift_l() {
        //x = 8'h03  <<< 2; $display("%b", x); // 0000000000001100
        //x = 8'hf1  <<< 2; $display("%b", x); // 0000001111000100
        //x = 8'hx3  <<< 2; $display("%b", x); // 000000xxxx001100
        //x = 8'hz4  <<< 2; $display("%b", x); // 000000zzzz010000
        //x = 8'sh03 <<< 2; $display("%b", x); // 0000000000001100
        //x = 8'shf1 <<< 2; $display("%b", x); // 1111111111000100
        //x = 8'shx3 <<< 2; $display("%b", x); // xxxxxxxxxx001100
        //x = 8'shz4 <<< 2; $display("%b", x); // zzzzzzzzzz010000

        let op = Op::ArithShiftL;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000001100");
        assert_eq!(format!("{:b}", x01), "16'b0000001111000100");
        assert_eq!(format!("{:b}", x02), "16'b000000xxxx001100");
        assert_eq!(format!("{:b}", x03), "16'b000000zzzz010000");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000001100");
        assert_eq!(format!("{:b}", x05), "16'sb1111111111000100");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxx001100");
        assert_eq!(format!("{:b}", x07), "16'sbzzzzzzzzzz010000");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h0000000000000000c");
        assert_eq!(format!("{:x}", x01), "68'hfffffffffffffffc4");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxXc");
        assert_eq!(format!("{:x}", x03), "68'hzzzzzzzzzzzzzzzZ0");
        assert_eq!(format!("{:x}", x04), "68'sh0000000000000000c");
        assert_eq!(format!("{:x}", x05), "68'shfffffffffffffffc4");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxXc");
        assert_eq!(format!("{:x}", x07), "68'shzzzzzzzzzzzzzzzZ0");
    }

    #[test]
    fn binary_pow() {
        //x = 8'h03  ** 2; $display("%b", x); // 0000000000001001
        //x = 8'hf1  ** 2; $display("%b", x); // 1110001011100001
        //x = 8'hx3  ** 2; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'hz4  ** 2; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'sh03 ** 2; $display("%b", x); // 0000000000001001
        //x = 8'shf1 ** 2; $display("%b", x); // 0000000011100001
        //x = 8'shx3 ** 2; $display("%b", x); // xxxxxxxxxxxxxxxx
        //x = 8'shz4 ** 2; $display("%b", x); // xxxxxxxxxxxxxxxx

        let op = Op::Pow;
        let mut cache = MaskCache::default();

        let x00 = Value::from_str("8'h03").unwrap();
        let x01 = Value::from_str("8'hf1").unwrap();
        let x02 = Value::from_str("8'hx3").unwrap();
        let x03 = Value::from_str("8'hz4").unwrap();
        let x04 = Value::from_str("8'sh03").unwrap();
        let x05 = Value::from_str("8'shf1").unwrap();
        let x06 = Value::from_str("8'shx3").unwrap();
        let x07 = Value::from_str("8'shz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(16), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(16), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(16), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(16), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(16), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(16), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(16), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(16), true, &mut cache);
        assert_eq!(format!("{:b}", x00), "16'b0000000000001001");
        assert_eq!(format!("{:b}", x01), "16'b1110001011100001");
        assert_eq!(format!("{:b}", x02), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x03), "16'bxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x04), "16'sb0000000000001001");
        assert_eq!(format!("{:b}", x05), "16'sb0000000011100001");
        assert_eq!(format!("{:b}", x06), "16'sbxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:b}", x07), "16'sbxxxxxxxxxxxxxxxx");

        let x00 = Value::from_str("68'h00000000000000003").unwrap();
        let x01 = Value::from_str("68'hffffffffffffffff1").unwrap();
        let x02 = Value::from_str("68'hxxxxxxxxxxxxxxxx3").unwrap();
        let x03 = Value::from_str("68'hzzzzzzzzzzzzzzzz4").unwrap();
        let x04 = Value::from_str("68'sh00000000000000003").unwrap();
        let x05 = Value::from_str("68'shffffffffffffffff1").unwrap();
        let x06 = Value::from_str("68'shxxxxxxxxxxxxxxxx3").unwrap();
        let x07 = Value::from_str("68'shzzzzzzzzzzzzzzzz4").unwrap();
        let x10 = Value::from_str("2").unwrap();
        let x11 = Value::from_str("2").unwrap();
        let x12 = Value::from_str("2").unwrap();
        let x13 = Value::from_str("2").unwrap();
        let x14 = Value::from_str("2").unwrap();
        let x15 = Value::from_str("2").unwrap();
        let x16 = Value::from_str("2").unwrap();
        let x17 = Value::from_str("2").unwrap();
        let x00 = op.eval_binary(&x00, &x10, Some(68), false, &mut cache);
        let x01 = op.eval_binary(&x01, &x11, Some(68), false, &mut cache);
        let x02 = op.eval_binary(&x02, &x12, Some(68), false, &mut cache);
        let x03 = op.eval_binary(&x03, &x13, Some(68), false, &mut cache);
        let x04 = op.eval_binary(&x04, &x14, Some(68), true, &mut cache);
        let x05 = op.eval_binary(&x05, &x15, Some(68), true, &mut cache);
        let x06 = op.eval_binary(&x06, &x16, Some(68), true, &mut cache);
        let x07 = op.eval_binary(&x07, &x17, Some(68), true, &mut cache);
        assert_eq!(format!("{:x}", x00), "68'h00000000000000009");
        assert_eq!(format!("{:x}", x01), "68'hf00000000000000e1");
        assert_eq!(format!("{:x}", x02), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x03), "68'hxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x04), "68'sh00000000000000009");
        assert_eq!(format!("{:x}", x05), "68'sh000000000000000e1");
        assert_eq!(format!("{:x}", x06), "68'shxxxxxxxxxxxxxxxxx");
        assert_eq!(format!("{:x}", x07), "68'shxxxxxxxxxxxxxxxxx");
    }
}
