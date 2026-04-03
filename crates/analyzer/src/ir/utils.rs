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
                        bits as i64 as f64
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
                        bits as i64 as f32
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
    } else {
        val
    }
}
