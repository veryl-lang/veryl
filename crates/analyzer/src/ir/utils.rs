use crate::attribute::{AllowItem, Attribute, CondTypeItem};
use crate::attribute_table;
use crate::ir::comptime::TypeKind;
use crate::value::Value;
use veryl_parser::token_range::TokenRange;

pub fn has_cond_type(token: &TokenRange) -> bool {
    let mut attrs = attribute_table::get(&token.beg);
    attrs.reverse();
    for attr in attrs {
        match attr {
            Attribute::CondType(CondTypeItem::None) => return false,
            Attribute::CondType(_) => return true,
            _ => (),
        }
    }
    false
}

pub fn allow_missing_reset_statement(token: &TokenRange) -> bool {
    attribute_table::contains(
        &token.beg,
        Attribute::Allow(AllowItem::MissingResetStatement),
    )
}

/// Sign-extend a width-masked `to_u64()` payload (i8 -2 is 0xFE) to i64, which
/// a bare `as i64` would instead read as the positive 254.
fn sign_extend_to_i64(bits: u64, width: usize) -> i64 {
    if (1..64).contains(&width) {
        let shift = 64 - width;
        ((bits << shift) as i64) >> shift
    } else {
        bits as i64
    }
}

/// Float values are stored as IEEE 754 bit patterns via `f64::to_bits()`,
/// so float<->int casts require actual numeric conversion, not just bit reinterpretation.
pub fn convert_cast(
    val: Value,
    src_kind: &TypeKind,
    dst_kind: &TypeKind,
    dst_width: usize,
) -> Value {
    let src_float = src_kind.is_float();
    let dst_float = dst_kind.is_float();

    if src_float && !dst_float {
        if let Some(bits) = val.to_u64() {
            match src_kind {
                TypeKind::F64 => {
                    let f = f64::from_bits(bits);
                    Value::new(f as i64 as u64, dst_width, false)
                }
                TypeKind::F32 => {
                    let f = f32::from_bits(bits as u32);
                    Value::new(f as i64 as u64, dst_width, false)
                }
                _ => val,
            }
        } else {
            val
        }
    } else if !src_float && dst_float {
        match dst_kind {
            TypeKind::F64 => {
                if let Some(bits) = val.to_u64() {
                    let f: f64 = if val.signed() {
                        sign_extend_to_i64(bits, val.width()) as f64
                    } else {
                        bits as f64
                    };
                    Value::new(f.to_bits(), 64, false)
                } else {
                    val
                }
            }
            TypeKind::F32 => {
                if let Some(bits) = val.to_u64() {
                    let f: f32 = if val.signed() {
                        sign_extend_to_i64(bits, val.width()) as f32
                    } else {
                        bits as f32
                    };
                    Value::new(f.to_bits() as u64, 32, false)
                } else {
                    val
                }
            }
            _ => val,
        }
    } else if match dst_kind {
        TypeKind::Bit => true,
        TypeKind::Struct(x) => x.is_2state(),
        TypeKind::Union(x) => x.is_2state(),
        TypeKind::Enum(x) => x.is_2state(),
        _ => false,
    } {
        // IEEE 1800-2023 6.11.2 maps X/Z to zero for 2-state targets.
        let mut val = val;
        match &mut val {
            Value::U64(x) => {
                x.payload &= !x.mask_xz;
                x.mask_xz = 0;
            }
            Value::BigUint(x) => {
                *x.payload ^= &*x.payload & &*x.mask_xz;
                *x.mask_xz = 0u32.into();
            }
        }
        val
    } else {
        val
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integral_casts_to_two_state_clear_unknown_payload_and_mask() {
        for width in [1, 4, 64, 65, 128, 129] {
            let input = Value::new_z(width, true);
            let output = convert_cast(input.clone(), &TypeKind::Logic, &TypeKind::Bit, width);
            assert!(output.is_zero());
            assert!(!output.is_xz());
            assert_eq!((output.width(), output.signed()), (width, true));
            assert_eq!(
                convert_cast(input.clone(), &TypeKind::Logic, &TypeKind::Logic, width),
                input
            );
        }
    }
}
