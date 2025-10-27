use crate::analyzer_error::{AnalyzerError, InvalidPortDefaultValueKind};
use crate::conv::{Affiliation, Context};
use crate::ir::{self, Comptime, IrResult, VarKind};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

pub fn check_port_default_value(
    context: &mut Context,
    value: &PortDeclarationItem,
    comptime: &IrResult<(Comptime, ir::Expression)>,
    kind: VarKind,
    expr: &Expression,
) {
    let token: TokenRange = if let PortDeclarationItemGroup::PortTypeConcrete(x) =
        value.port_declaration_item_group.as_ref()
        && let Some(x) = &x.port_type_concrete.port_type_concrete_opt0
    {
        x.port_default_value.as_ref().into()
    } else {
        return;
    };

    if let Ok((comptime, _)) = comptime
        && !comptime.is_global
    {
        context.insert_error(AnalyzerError::invalid_port_default_value(
            InvalidPortDefaultValueKind::NotGlobal,
            &token,
        ));
    }
    // For now, port default value is allowed for module only.
    // https://github.com/veryl-lang/veryl/issues/1178#issuecomment-2568996379
    if context.is_affiliated(Affiliation::Function) {
        context.insert_error(AnalyzerError::invalid_port_default_value(
            InvalidPortDefaultValueKind::InFunction,
            &token,
        ));
    }
    match kind {
        VarKind::Input => (),
        VarKind::Output => {
            if !expr.is_anonymous_expression() {
                context.insert_error(AnalyzerError::invalid_port_default_value(
                    InvalidPortDefaultValueKind::NonAnonymousInOutput,
                    &token,
                ));
            }
        }
        _ => {
            context.insert_error(AnalyzerError::invalid_port_default_value(
                InvalidPortDefaultValueKind::InvalidDirection(kind.to_string()),
                &token,
            ));
        }
    }
}

pub fn check_port_direction(context: &mut Context, value: &PortDeclarationItem) {
    if let PortDeclarationItemGroup::PortTypeConcrete(x) =
        value.port_declaration_item_group.as_ref()
    {
        let x = x.port_type_concrete.as_ref();
        let direction = x.direction.as_ref();
        check_direction(context, direction);

        if let Direction::Inout(_) = direction {
            let r#type = &x.array_type;
            let is_tri = r#type
                .scalar_type
                .scalar_type_list
                .iter()
                .any(|x| matches!(x.type_modifier.as_ref(), TypeModifier::Tri(_)));

            if !is_tri {
                context.insert_error(AnalyzerError::missing_tri(&r#type.as_ref().into()));
            }
        }
    }
}

pub fn check_direction(context: &mut Context, value: &Direction) {
    match value {
        Direction::Modport(x) => {
            let valid = context.is_affiliated(Affiliation::Module)
                | context.is_affiliated(Affiliation::Function)
                | context.is_affiliated(Affiliation::ProtoModule);
            if !valid {
                context.insert_error(AnalyzerError::invalid_direction(
                    "modport",
                    &x.modport.modport_token.token.into(),
                ));
            }
        }
        Direction::Import(x) => {
            let valid = context.is_affiliated(Affiliation::Modport);
            if !valid {
                context.insert_error(AnalyzerError::invalid_direction(
                    "import",
                    &x.import.import_token.token.into(),
                ));
            }
        }
        _ => (),
    }
}
