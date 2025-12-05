use num_bigint::{BigInt, BigUint, Sign};

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
    let end = inv(end, width);
    beg & end
}

pub fn inv(value: BigUint, width: usize) -> BigUint {
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
        value.magnitude().clone()
    } else {
        let payload = value.magnitude().clone();
        let mask = gen_mask(width);
        (inv(payload, width) + BigUint::from(1u32)) & mask
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
        assert_eq!(format!("{:x}", inv(BigUint::from(1u32), 1)), "0");
        assert_eq!(format!("{:x}", inv(BigUint::from(1u32), 2)), "2");
        assert_eq!(format!("{:x}", inv(BigUint::from(1u32), 3)), "6");
        assert_eq!(format!("{:x}", inv(BigUint::from(1u32), 10)), "3fe");
        assert_eq!(
            format!("{:x}", inv(BigUint::from(1u32), 59)),
            "7fffffffffffffe"
        );
        assert_eq!(
            format!("{:x}", inv(BigUint::from(1u32), 90)),
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
