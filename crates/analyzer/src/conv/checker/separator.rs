use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::symbol::{SymbolId, SymbolKind};
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;

fn check_path(
    context: &mut Context,
    full_path: &mut Vec<SymbolId>,
    check_dot_separator: bool,
    separator_token: &Token,
) {
    let preceed_symbol = if let Some(symbol_id) = full_path.pop() {
        symbol_table::get(symbol_id).unwrap()
    } else {
        return;
    };
    let this_symbol = if let Some(symbol_id) = full_path.last() {
        symbol_table::get(*symbol_id).unwrap()
    } else {
        // length of `full_path` may be shorter than length of actual path
        // if the type of the preceed symbol is defined in the SV namespace.
        return;
    };
    let expect_dot_separator = if let SymbolKind::Function(_) = this_symbol.kind {
        matches!(
            preceed_symbol.kind,
            SymbolKind::Instance(_) // member function of interface
        )
    } else {
        matches!(
            this_symbol.kind,
            SymbolKind::Variable(_) // member variable of instance
            | SymbolKind::StructMember(_)
            | SymbolKind::UnionMember(_)
            | SymbolKind::Modport(_)
            | SymbolKind::ModportVariableMember(_)
            | SymbolKind::ModportFunctionMember(_)
        )
    };

    if expect_dot_separator != check_dot_separator {
        context.insert_error(AnalyzerError::wrong_seperator(
            &separator_token.to_string(),
            &separator_token.into(),
        ));
    }
}

pub fn check_separator(context: &mut Context, value: &ExpressionIdentifier) {
    if let Ok(symbol) = symbol_table::resolve(value) {
        let mut full_path: Vec<_> = symbol.full_path.into_iter().rev().collect();

        for x in &value.scoped_identifier.scoped_identifier_list {
            check_path(
                context,
                &mut full_path,
                false,
                &x.colon_colon.colon_colon_token.token,
            );
        }

        for x in &value.expression_identifier_list0 {
            check_path(context, &mut full_path, true, &x.dot.dot_token.token);
        }
    }
}
