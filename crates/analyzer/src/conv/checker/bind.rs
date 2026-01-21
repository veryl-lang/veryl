use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::symbol::Symbol;
use veryl_parser::Stringifier;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::VerylWalker;

pub fn check_bind_target(
    context: &mut Context,
    identifier: &ScopedIdentifier,
    target: &Symbol,
) -> bool {
    if !(target.is_module(false) || target.is_interface(false)) {
        let mut stringifier = Stringifier::new();
        stringifier.scoped_identifier(identifier);
        let name = stringifier.as_str();

        context.insert_error(AnalyzerError::mismatch_type(
            name,
            "module or interface",
            &target.kind.to_kind_name(),
            &identifier.into(),
        ));
        return false;
    }
    true
}
