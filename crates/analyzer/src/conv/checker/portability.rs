use crate::analyzer_error::AnalyzerError;
use crate::attribute::{AllowItem, Attribute};
use crate::attribute_table;
use crate::conv::Context;
use crate::ir::{Expression, FfReset, TypeKind, VarId, VarPath, VarPathSelect};
use crate::namespace::DefineContext;
use crate::symbol::{Direction, Symbol, SymbolKind};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::{AlwaysFfDeclaration, StatementBlockItem};

pub fn check_statement_after_if_reset(
    context: &mut Context,
    value: &AlwaysFfDeclaration,
    reset: &FfReset,
) {
    // Only explicitly synchronous resets are safe: a library's generic reset
    // may become asynchronous under the consuming project's build settings.
    if !matches!(
        reset.comptime.r#type.kind,
        TypeKind::Reset | TypeKind::ResetAsyncLow | TypeKind::ResetAsyncHigh
    ) || context.in_test_module
    {
        return;
    }

    // eval_reset only supplies a reset when the first item is if_reset.
    // Inspect the source before constant folding so the warning also covers
    // constructs that remain in the emitted SystemVerilog.
    let items: Vec<_> = value.statement_block.as_ref().into();
    for item in items.into_iter().skip(1) {
        if matches!(
            item,
            StatementBlockItem::VarDeclaration(_)
                | StatementBlockItem::ConstDeclaration(_)
                | StatementBlockItem::GenDeclaration(_)
        ) {
            continue;
        }
        let token: TokenRange = item.into();
        if DefineContext::from(token.beg).is_active(&context.config.defines) {
            context.insert_error(AnalyzerError::statement_after_if_reset(&token));
        }
    }
}

/// The opt-in relies on the target device initializing the variable at
/// configuration time, which ASIC synthesizers ignore. Testbench modules are
/// exempt: their `initial` blocks are not part of the design.
pub fn check_initial_assign(context: &mut Context, path: &VarPath, token: &TokenRange) {
    let Some(local_base) = context.in_initial else {
        return;
    };
    if context.in_test_module {
        return;
    }

    let Some((id, _)) = context.find_path(path) else {
        return;
    };
    // Declared inside the block: a procedural local, not state the device
    // brings up.
    if id >= local_base {
        return;
    }
    if !context.variables.contains_key(&id) || allow_initial_assign(context, id) {
        return;
    }

    context.insert_error(AnalyzerError::invalid_initial_assign(
        &path.to_string(),
        token,
    ));
}

/// Only a few system functions are modelled by `SystemFunctionCall`; for the
/// rest the arguments never become assignment destinations, so the declared
/// port directions are what covers them (`$readmemb` among others).
pub fn check_initial_assign_system_function_args(
    context: &mut Context,
    symbol: &Symbol,
    args: &[(Expression, Vec<VarPathSelect>, TokenRange)],
) {
    if context.in_initial.is_none() || context.in_test_module {
        return;
    }

    let SymbolKind::SystemFunction(property) = &symbol.kind else {
        return;
    };

    for (port, arg) in property.ports.iter().zip(args.iter()) {
        if !matches!(
            port.property().direction,
            Direction::Output | Direction::Inout
        ) {
            continue;
        }
        for dst in &arg.1 {
            check_initial_assign(context, &dst.0, &dst.2);
        }
    }
}

pub fn allow_initial_assign(context: &Context, id: VarId) -> bool {
    context.variables.get(&id).is_some_and(|variable| {
        attribute_table::contains(
            &variable.token.beg,
            Attribute::Allow(AllowItem::InitialAssign),
        )
    })
}

pub fn allow_multiple_assign(context: &Context, id: VarId) -> bool {
    context.variables.get(&id).is_some_and(|variable| {
        attribute_table::contains(
            &variable.token.beg,
            Attribute::Allow(AllowItem::MultipleAssign),
        )
    })
}
