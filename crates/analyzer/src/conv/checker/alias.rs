use crate::conv::Context;
use crate::conv::checker::generic::check_generic_args;
use crate::{AnalyzerError, symbol_table};
use veryl_parser::veryl_grammar_trait::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AliasType {
    Module,
    ProtoModule,
    Interface,
    ProtoInterface,
    Package,
    ProtoPackage,
}

pub fn check_alias_target(context: &mut Context, value: &ScopedIdentifier, r#type: AliasType) {
    if let Ok(symbol) = symbol_table::resolve(value) {
        let symbol = symbol.found;

        let expected = match r#type {
            AliasType::Module if !symbol.is_module(false) => Some("module"),
            AliasType::ProtoModule if !symbol.is_proto_module(true) => Some("proto module"),
            AliasType::Interface if !symbol.is_interface(false) => Some("interface"),
            AliasType::ProtoInterface if !symbol.is_proto_interface(true, false) => {
                Some("proto interface")
            }
            AliasType::Package if !symbol.is_package(false) => Some("package"),
            AliasType::ProtoPackage if !symbol.is_proto_package(true) => Some("proto package"),
            _ => None,
        };

        if let Some(expected) = expected {
            context.insert_error(AnalyzerError::mismatch_type(
                &symbol.token.to_string(),
                expected,
                &symbol.kind.to_kind_name(),
                &value.identifier().token.into(),
            ));
        }
    }

    check_generic_args(context, &value.into());
}
