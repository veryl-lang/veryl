use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::conv::utils::TypePosition;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;

fn calc_width(value: usize) -> usize {
    (usize::BITS - value.leading_zeros()) as usize
}

pub fn check_enum(context: &mut Context, arg: &EnumDeclaration) {
    let enum_symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();

    if let SymbolKind::Enum(r#enum) = enum_symbol.found.kind
        && let Some(r#type) = r#enum.r#type
        && let Ok(r#type) = r#type.to_ir_type(context, TypePosition::Enum)
        && let Some(width) = r#type.total_width()
    {
        let variants = r#enum.members.len();
        if calc_width(variants - 1) > width {
            let name = arg.identifier.identifier_token.to_string();
            context.insert_error(AnalyzerError::too_much_enum_variant(
                &name,
                variants,
                width,
                &arg.identifier.as_ref().into(),
            ));
        }

        for id in r#enum.members {
            let member_symbol = symbol_table::get(id).unwrap();
            if let SymbolKind::EnumMember(member) = member_symbol.kind {
                let member_value = member.value.value().unwrap_or(0);
                if calc_width(member_value) > width {
                    context.insert_error(AnalyzerError::too_large_enum_variant(
                        &member_symbol.token.to_string(),
                        member_value as isize,
                        width,
                        &member_symbol.token.into(),
                    ));
                }
            }
        }
    }
}
