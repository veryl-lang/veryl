use crate::analyzer_error::{AnalyzerError, UnevaluableValueKind};
use crate::attribute::EnumEncodingItem;
use crate::conv::utils::TypePosition;
use crate::conv::{self, Context, Conv};
use crate::ir::{self, IrResult};
use crate::symbol::{EnumMemberValue, EnumProperty, Symbol, SymbolKind};
use crate::symbol_table;

pub fn resolve_enum(list: &[Symbol]) -> Vec<AnalyzerError> {
    let mut errors = Vec::new();
    let mut context = conv::Context::default();

    for symbol in list {
        let SymbolKind::Enum(r#enum) = &symbol.kind else {
            unreachable!();
        };
        let enum_width = eval_enum_width(&mut context, symbol, r#enum, &mut errors);

        let mut pre_value = None;
        let mut member_width = 0;
        for id in &r#enum.members {
            let mut symbol = symbol_table::get(*id).unwrap();

            let value = eval_enum_member_value(
                &mut context,
                &symbol,
                r#enum,
                pre_value.as_ref(),
                &mut errors,
            );
            if matches!(value, EnumMemberValue::UnevaluableValue) {
                errors.push(AnalyzerError::unevaluable_value(
                    UnevaluableValueKind::EnumVariant,
                    &symbol.token.into(),
                ));
            }

            if let Some(value) = value.value() {
                let width = calc_width(value);
                if enum_width > 0 && width > enum_width {
                    errors.push(AnalyzerError::too_large_enum_variant(
                        &symbol.token.to_string(),
                        value as isize,
                        enum_width,
                        &symbol.token.into(),
                    ));
                }
                member_width = member_width.max(width);
            }

            if let SymbolKind::EnumMember(mut member) = symbol.kind {
                member.value = value.clone();
                symbol.kind = SymbolKind::EnumMember(member);
                symbol_table::update(symbol);
            }

            pre_value = Some(value);
        }

        let mut symbol = symbol.clone();
        if let SymbolKind::Enum(mut r#enum) = symbol.kind {
            r#enum.width = 1.max(enum_width.max(member_width));
            symbol.kind = SymbolKind::Enum(r#enum);
            symbol_table::update(symbol);
        }
    }

    errors
}

fn eval_enum_width(
    context: &mut Context,
    symbol: &Symbol,
    r#enum: &EnumProperty,
    errors: &mut Vec<AnalyzerError>,
) -> usize {
    let Some(r#type) = &r#enum.r#type else {
        return 0;
    };

    let width = if let Ok(x) = r#type.to_ir_type(context, TypePosition::Enum) {
        x.total_width().unwrap_or(0)
    } else {
        0
    };

    if width > 0 && calc_width(r#enum.members.len() - 1) > width {
        errors.push(AnalyzerError::too_much_enum_variant(
            &symbol.token.to_string(),
            r#enum.members.len(),
            width,
            &symbol.token.into(),
        ));
    }

    width
}

fn eval_enum_member_value(
    context: &mut Context,
    symbol: &Symbol,
    r#enum: &EnumProperty,
    pre_value: Option<&EnumMemberValue>,
    errors: &mut Vec<AnalyzerError>,
) -> EnumMemberValue {
    let SymbolKind::EnumMember(enum_member) = &symbol.kind else {
        unreachable!();
    };

    if let EnumMemberValue::ExplicitValue(ref expression, _) = enum_member.value {
        let Ok(mut expr): IrResult<ir::Expression> = Conv::conv(context, expression) else {
            return EnumMemberValue::UnevaluableValue;
        };

        let comptime = expr.eval_comptime(context, None);
        let Ok(value) = comptime.get_value() else {
            return EnumMemberValue::UnevaluableValue;
        };

        if value.is_xz() {
            if matches!(
                &r#enum.encoding,
                EnumEncodingItem::OneHot | EnumEncodingItem::Gray
            ) {
                EnumMemberValue::UnevaluableValue
            } else {
                if let Some(r#type) = &r#enum.r#type
                    && r#type.kind.is_2state()
                {
                    let src = comptime.r#type.to_string();
                    let dst = r#type.to_string();
                    errors.push(AnalyzerError::mismatch_assignment(
                        &src,
                        &dst,
                        &expression.into(),
                        &[],
                    ));
                }
                enum_member.value.clone()
            }
        } else {
            let value = value.to_usize().unwrap_or(0);
            let is_valid = match r#enum.encoding {
                EnumEncodingItem::OneHot => value.count_ones() == 1,
                EnumEncodingItem::Gray => get_enum_member_next_value(r#enum.encoding, pre_value)
                    .map(|x| value == x)
                    .unwrap_or(true),
                _ => true,
            };
            if is_valid {
                EnumMemberValue::ExplicitValue(expression.clone(), Some(value))
            } else {
                errors.push(AnalyzerError::invalid_enum_variant(
                    &symbol.token.to_string(),
                    &r#enum.encoding.to_string(),
                    &symbol.token.into(),
                ));
                enum_member.value.clone()
            }
        }
    } else if let Some(value) = get_enum_member_next_value(r#enum.encoding, pre_value) {
        EnumMemberValue::ImplicitValue(value)
    } else {
        EnumMemberValue::UnevaluableValue
    }
}

fn calc_width(value: usize) -> usize {
    (usize::BITS - value.leading_zeros()) as usize
}

fn get_enum_member_next_value(
    encoding: EnumEncodingItem,
    pre_value: Option<&EnumMemberValue>,
) -> Option<usize> {
    if let Some(value) = pre_value
        && let Some(value) = value.value()
    {
        match encoding {
            EnumEncodingItem::Sequential => Some(value + 1),
            EnumEncodingItem::OneHot => Some(value << 1),
            EnumEncodingItem::Gray => Some(((value + 1) >> 1) ^ (value + 1)),
        }
    } else if pre_value.is_none() {
        if matches!(encoding, EnumEncodingItem::OneHot) {
            Some(1)
        } else {
            Some(0)
        }
    } else {
        None
    }
}
