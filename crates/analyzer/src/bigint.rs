use num_bigint as bigint_lib;
use num_traits::{FromPrimitive, Num, One, ToPrimitive, Zero};
use std::ops::{
    Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div, DivAssign,
    Mul, MulAssign, Rem, RemAssign, Shl, ShlAssign, Shr, ShrAssign, Sub, SubAssign,
};

macro_rules! impl_from {
    ($x:ident, $y:ty) => {
        impl From<$y> for $x {
            #[inline(always)]
            fn from(value: $y) -> Self {
                $x(value.into())
            }
        }
    };
}

macro_rules! impl_binary_op {
    ($x:ident, $y:ident, $z:ident) => {
        impl $y for $x {
            type Output = $x;

            #[inline(always)]
            fn $z(self, rhs: Self) -> Self::Output {
                $x(self.0.$z(rhs.0))
            }
        }

        impl $y for &$x {
            type Output = $x;

            #[inline(always)]
            fn $z(self, rhs: Self) -> Self::Output {
                $x((&self.0).$z(&rhs.0))
            }
        }

        impl $y<$x> for &$x {
            type Output = $x;

            #[inline(always)]
            fn $z(self, rhs: $x) -> Self::Output {
                $x((&self.0).$z(&rhs.0))
            }
        }

        impl<'a> $y<&'a $x> for $x {
            type Output = $x;

            #[inline(always)]
            fn $z(self, rhs: &'a $x) -> Self::Output {
                $x((&self.0).$z(&rhs.0))
            }
        }
    };

    ($x:ident, $y:ident, $z:ident, $v:ident) => {
        impl $y<$v> for $x {
            type Output = $x;

            #[inline(always)]
            fn $z(self, rhs: $v) -> Self::Output {
                $x(self.0.$z(rhs))
            }
        }

        impl $y<$v> for &$x {
            type Output = $x;

            #[inline(always)]
            fn $z(self, rhs: $v) -> Self::Output {
                $x((&self.0).$z(rhs))
            }
        }
    };
}

macro_rules! impl_assign_op {
    ($x:ident, $y:ident, $z:ident) => {
        impl $y for $x {
            #[inline(always)]
            fn $z(&mut self, rhs: Self) {
                self.0.$z(rhs.0);
            }
        }

        impl $y for &mut $x {
            #[inline(always)]
            fn $z(&mut self, rhs: Self) {
                self.0.$z(&rhs.0);
            }
        }

        impl<'a> $y<&'a $x> for $x {
            #[inline(always)]
            fn $z(&mut self, rhs: &'a Self) {
                self.0.$z(&rhs.0);
            }
        }
    };
    ($x:ident, $y:ident, $z:ident, $v:ident) => {
        impl $y<$v> for $x {
            #[inline(always)]
            fn $z(&mut self, rhs: $v) {
                self.0.$z(rhs);
            }
        }

        impl $y<$v> for &mut $x {
            #[inline(always)]
            fn $z(&mut self, rhs: $v) {
                self.0.$z(&rhs);
            }
        }
    };
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BigUint(bigint_lib::BigUint);

impl BigUint {
    #[inline(always)]
    pub fn pow(&self, x: u32) -> BigUint {
        Self(self.0.pow(x))
    }

    #[inline(always)]
    pub fn bit(&self, bit: u64) -> bool {
        self.0.bit(bit)
    }

    #[inline(always)]
    pub fn bits(&self) -> u64 {
        self.0.bits()
    }

    #[inline(always)]
    pub fn set_bit(&mut self, bit: u64, value: bool) {
        self.0.set_bit(bit, value)
    }

    #[inline(always)]
    pub fn from_slice(slice: &[u32]) -> Self {
        BigUint(bigint_lib::BigUint::from_slice(slice))
    }

    #[inline(always)]
    pub fn to_u32_digits(&self) -> Vec<u32> {
        self.0.to_u32_digits()
    }

    #[inline(always)]
    pub fn count_ones(&self) -> u64 {
        self.0.count_ones()
    }
}

impl_from!(BigUint, bool);
impl_from!(BigUint, usize);
impl_from!(BigUint, u8);
impl_from!(BigUint, u16);
impl_from!(BigUint, u32);
impl_from!(BigUint, u64);
impl_from!(BigUint, u128);

impl_binary_op!(BigUint, Add, add);
impl_binary_op!(BigUint, Sub, sub);
impl_binary_op!(BigUint, Mul, mul);
impl_binary_op!(BigUint, Div, div);
impl_binary_op!(BigUint, Rem, rem);
impl_binary_op!(BigUint, BitAnd, bitand);
impl_binary_op!(BigUint, BitOr, bitor);
impl_binary_op!(BigUint, BitXor, bitxor);
impl_binary_op!(BigUint, Shl, shl, u32);
impl_binary_op!(BigUint, Shl, shl, usize);
impl_binary_op!(BigUint, Shr, shr, u32);
impl_binary_op!(BigUint, Shr, shr, usize);

impl_assign_op!(BigUint, AddAssign, add_assign);
impl_assign_op!(BigUint, SubAssign, sub_assign);
impl_assign_op!(BigUint, MulAssign, mul_assign);
impl_assign_op!(BigUint, DivAssign, div_assign);
impl_assign_op!(BigUint, RemAssign, rem_assign);
impl_assign_op!(BigUint, BitAndAssign, bitand_assign);
impl_assign_op!(BigUint, BitOrAssign, bitor_assign);
impl_assign_op!(BigUint, BitXorAssign, bitxor_assign);
impl_assign_op!(BigUint, ShlAssign, shl_assign, u32);
impl_assign_op!(BigUint, ShlAssign, shl_assign, usize);
impl_assign_op!(BigUint, ShlAssign, shl_assign, i32);
impl_assign_op!(BigUint, ShlAssign, shl_assign, isize);
impl_assign_op!(BigUint, ShrAssign, shr_assign, u32);
impl_assign_op!(BigUint, ShrAssign, shr_assign, usize);
impl_assign_op!(BigUint, ShrAssign, shr_assign, i32);
impl_assign_op!(BigUint, ShrAssign, shr_assign, isize);

impl Zero for BigUint {
    #[inline(always)]
    fn zero() -> Self {
        Self(bigint_lib::BigUint::zero())
    }

    #[inline(always)]
    fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    #[inline(always)]
    fn set_zero(&mut self) {
        self.0.set_zero()
    }
}

impl One for BigUint {
    #[inline(always)]
    fn one() -> Self {
        Self(bigint_lib::BigUint::one())
    }

    #[inline(always)]
    fn is_one(&self) -> bool {
        self.0.is_one()
    }

    #[inline(always)]
    fn set_one(&mut self) {
        self.0.set_one()
    }
}

impl Num for BigUint {
    type FromStrRadixErr = ();

    #[inline(always)]
    fn from_str_radix(str: &str, radix: u32) -> Result<Self, Self::FromStrRadixErr> {
        match bigint_lib::BigUint::from_str_radix(str, radix) {
            Ok(x) => Ok(Self(x)),
            Err(_) => Err(()),
        }
    }
}

impl FromPrimitive for BigUint {
    #[inline(always)]
    fn from_i64(n: i64) -> Option<Self> {
        bigint_lib::BigUint::from_i64(n).map(BigUint)
    }

    #[inline(always)]
    fn from_u64(n: u64) -> Option<Self> {
        bigint_lib::BigUint::from_u64(n).map(BigUint)
    }
}

impl ToPrimitive for BigUint {
    #[inline(always)]
    fn to_i64(&self) -> Option<i64> {
        self.0.to_i64()
    }

    #[inline(always)]
    fn to_u64(&self) -> Option<u64> {
        self.0.to_u64()
    }
}

impl std::fmt::LowerHex for BigUint {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for BigUint {
    type Err = ();

    #[inline(always)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match bigint_lib::BigUint::from_str(s) {
            Ok(x) => Ok(Self(x)),
            Err(_) => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct BigInt(bigint_lib::BigInt);

impl BigInt {
    #[inline(always)]
    pub fn pow(&self, x: u32) -> BigInt {
        Self(self.0.pow(x))
    }

    #[inline(always)]
    pub fn checked_div(&self, value: &BigInt) -> Option<BigInt> {
        self.0.checked_div(&value.0).map(Self)
    }

    #[inline(always)]
    pub fn from_biguint(sign: Sign, value: BigUint) -> Self {
        let sign = match sign {
            Sign::Plus => bigint_lib::Sign::Plus,
            Sign::Minus => bigint_lib::Sign::Minus,
            Sign::NoSign => bigint_lib::Sign::NoSign,
        };
        BigInt(bigint_lib::BigInt::from_biguint(sign, value.0))
    }

    #[inline(always)]
    pub fn sign(&self) -> Sign {
        match self.0.sign() {
            bigint_lib::Sign::Plus => Sign::Plus,
            bigint_lib::Sign::Minus => Sign::Minus,
            bigint_lib::Sign::NoSign => Sign::NoSign,
        }
    }

    #[inline(always)]
    pub fn magnitude(&self) -> BigUint {
        BigUint(self.0.magnitude().clone())
    }
}

impl_from!(BigInt, usize);
impl_from!(BigInt, u8);
impl_from!(BigInt, u16);
impl_from!(BigInt, u32);
impl_from!(BigInt, u64);
impl_from!(BigInt, u128);
impl_from!(BigInt, isize);
impl_from!(BigInt, i8);
impl_from!(BigInt, i16);
impl_from!(BigInt, i32);
impl_from!(BigInt, i64);
impl_from!(BigInt, i128);

impl_binary_op!(BigInt, Add, add);
impl_binary_op!(BigInt, Sub, sub);
impl_binary_op!(BigInt, Mul, mul);
impl_binary_op!(BigInt, Div, div);
impl_binary_op!(BigInt, Rem, rem);
impl_binary_op!(BigInt, BitAnd, bitand);
impl_binary_op!(BigInt, BitOr, bitor);
impl_binary_op!(BigInt, BitXor, bitxor);
impl_binary_op!(BigInt, Shl, shl, u32);
impl_binary_op!(BigInt, Shl, shl, usize);
impl_binary_op!(BigInt, Shr, shr, u32);
impl_binary_op!(BigInt, Shr, shr, usize);

impl_assign_op!(BigInt, AddAssign, add_assign);
impl_assign_op!(BigInt, SubAssign, sub_assign);
impl_assign_op!(BigInt, MulAssign, mul_assign);
impl_assign_op!(BigInt, DivAssign, div_assign);
impl_assign_op!(BigInt, RemAssign, rem_assign);
impl_assign_op!(BigInt, BitAndAssign, bitand_assign);
impl_assign_op!(BigInt, BitOrAssign, bitor_assign);
impl_assign_op!(BigInt, BitXorAssign, bitxor_assign);
impl_assign_op!(BigInt, ShlAssign, shl_assign, u32);
impl_assign_op!(BigInt, ShlAssign, shl_assign, usize);
impl_assign_op!(BigInt, ShlAssign, shl_assign, i32);
impl_assign_op!(BigInt, ShlAssign, shl_assign, isize);
impl_assign_op!(BigInt, ShrAssign, shr_assign, u32);
impl_assign_op!(BigInt, ShrAssign, shr_assign, usize);
impl_assign_op!(BigInt, ShrAssign, shr_assign, i32);
impl_assign_op!(BigInt, ShrAssign, shr_assign, isize);

impl FromPrimitive for BigInt {
    #[inline(always)]
    fn from_i64(n: i64) -> Option<Self> {
        bigint_lib::BigInt::from_i64(n).map(BigInt)
    }

    #[inline(always)]
    fn from_u64(n: u64) -> Option<Self> {
        bigint_lib::BigInt::from_u64(n).map(BigInt)
    }
}

impl ToPrimitive for BigInt {
    #[inline(always)]
    fn to_i64(&self) -> Option<i64> {
        self.0.to_i64()
    }

    #[inline(always)]
    fn to_u64(&self) -> Option<u64> {
        self.0.to_u64()
    }
}

impl std::fmt::LowerHex for BigInt {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sign {
    Minus,
    NoSign,
    Plus,
}
