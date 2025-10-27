use crate::analyzer_error::AnalyzerError;
use crate::conv::checker::r#enum::check_enum;
use crate::conv::checker::modport::{check_modport, check_modport_in_port};
use crate::conv::checker::port::{check_direction, check_port_default_value, check_port_direction};
use crate::conv::utils::{
    eval_array_literal, eval_expr, eval_range, expand_connect, get_component, get_return_str,
};
use crate::conv::{Affiliation, Context, Conv};
use crate::ir::{
    self, TypeKind, TypedValue, Value, ValueVariant, VarIndex, VarKind, VarPath, VarPathSelect,
    VarSelect, Variable,
};
use crate::namespace::DefineContext;
use crate::symbol::{Direction, GenericBoundKind, SymbolKind};
use crate::symbol_table;
use crate::{HashMap, HashSet};
use num_bigint::BigUint;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&GenerateItem> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateItem) -> Self {
        match value {
            GenerateItem::LetDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.let_declaration.as_ref()))
            }
            GenerateItem::VarDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.var_declaration.as_ref()))
            }
            GenerateItem::AlwaysFfDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.always_ff_declaration.as_ref()))
            }
            GenerateItem::AlwaysCombDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.always_comb_declaration.as_ref()))
            }
            GenerateItem::GenerateIfDeclaration(x) => {
                Conv::conv(context, x.generate_if_declaration.as_ref())
            }
            GenerateItem::GenerateForDeclaration(x) => {
                Conv::conv(context, x.generate_for_declaration.as_ref())
            }
            GenerateItem::ConstDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.const_declaration.as_ref()))
            }
            GenerateItem::AssignDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.assign_declaration.as_ref()))
            }
            GenerateItem::FunctionDeclaration(x) => {
                let _: () = Conv::conv(context, x.function_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::EnumDeclaration(x) => {
                let _: () = Conv::conv(context, x.enum_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::InitialDeclaration(x) => {
                let _: () = Conv::conv(context, x.initial_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::FinalDeclaration(x) => {
                let _: () = Conv::conv(context, x.final_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::InstDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.inst_declaration.as_ref()))
            }
            GenerateItem::ConnectDeclaration(x) => {
                Conv::conv(context, x.connect_declaration.as_ref())
            }
            _ => {
                let token: TokenRange = value.into();
                context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                ir::DeclarationBlock::default()
            }
        }
    }
}

fn get_label(block: &GenerateOptionalNamedBlock, default: StrId) -> StrId {
    if let Some(x) = &block.generate_optional_named_block_opt {
        x.identifier.text()
    } else {
        default
    }
}

impl Conv<&GenerateIfDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateIfDeclaration) -> Self {
        let label = value.generate_named_block.identifier.text();
        let token: TokenRange = value.into();

        let (typed_value, _) = eval_expr(context, None, value.expression.as_ref());

        let Some(cond) = typed_value.get_value() else {
            context.insert_error(AnalyzerError::unsupported_by_ir(&token));
            return ir::DeclarationBlock::default();
        };

        if cond.to_usize() != 0 {
            context.push_hier(label);
            let block: ir::DeclarationBlock =
                Conv::conv(context, value.generate_named_block.as_ref());
            context.pop_hier();
            block
        } else {
            for x in &value.generate_if_declaration_list {
                let (typed_value, _) = eval_expr(context, None, x.expression.as_ref());

                let Some(cond) = typed_value.get_value() else {
                    context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                    return ir::DeclarationBlock::default();
                };

                if cond.to_usize() != 0 {
                    let label = get_label(&x.generate_optional_named_block, label);

                    context.push_hier(label);
                    let block: ir::DeclarationBlock =
                        Conv::conv(context, x.generate_optional_named_block.as_ref());
                    context.pop_hier();
                    return block;
                }
            }

            if let Some(x) = &value.generate_if_declaration_opt {
                let label = get_label(&x.generate_optional_named_block, label);

                context.push_hier(label);
                let block: ir::DeclarationBlock =
                    Conv::conv(context, x.generate_optional_named_block.as_ref());
                context.pop_hier();
                block
            } else {
                ir::DeclarationBlock::default()
            }
        }
    }
}

impl Conv<&GenerateNamedBlock> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateNamedBlock) -> Self {
        let mut ret = vec![];
        for x in &value.generate_named_block_list {
            let items: Vec<_> = x.generate_group.as_ref().into();
            for item in items {
                let mut item: ir::DeclarationBlock = Conv::conv(context, item);
                ret.append(&mut item.0);
            }
        }
        ir::DeclarationBlock(ret)
    }
}

impl Conv<&GenerateOptionalNamedBlock> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateOptionalNamedBlock) -> Self {
        let mut ret = vec![];
        for x in &value.generate_optional_named_block_list {
            let items: Vec<_> = x.generate_group.as_ref().into();
            for item in items {
                let mut item: ir::DeclarationBlock = Conv::conv(context, item);
                ret.append(&mut item.0);
            }
        }
        ir::DeclarationBlock(ret)
    }
}

impl Conv<&GenerateForDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateForDeclaration) -> Self {
        let label = value.generate_named_block.identifier.text();
        let range = eval_range(context, &value.range);

        let mut ret = ir::DeclarationBlock::default();

        if let Some((beg, end)) = range {
            for i in beg..end {
                let label = format!("{}[{}]", label, i);
                let label = resource_table::insert_str(&label);

                context.push_hier(label);

                let index = value.identifier.text();
                let token: TokenRange = (&value.identifier.identifier_token).into();
                let path = VarPath::new(index);
                let kind = VarKind::Const;
                let typed_value = TypedValue::create_value(BigUint::from(i), 32);

                let id = context.insert_var_path(path.clone(), typed_value.clone());
                let variable = Variable::new(
                    id,
                    path,
                    kind,
                    typed_value.r#type.clone(),
                    vec![typed_value.get_value().unwrap()],
                    context.get_affiliation(),
                    &token,
                );
                context.insert_variable(id, variable);

                let mut block: ir::DeclarationBlock =
                    Conv::conv(context, value.generate_named_block.as_ref());
                ret.0.append(&mut block.0);

                context.pop_hier();
            }
        } else {
            let token: TokenRange = value.into();
            context.insert_error(AnalyzerError::unsupported_by_ir(&token));
        }

        ret
    }
}

impl Conv<&WithGenericParameterItem> for () {
    fn conv(context: &mut Context, value: &WithGenericParameterItem) -> Self {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::GenericParameter(x) = symbol.found.kind
            && let Some(y) = &value.with_generic_parameter_item_opt
        {
            let token: TokenRange = y.with_generic_argument_item.as_ref().into();

            match y.with_generic_argument_item.as_ref() {
                WithGenericArgumentItem::Number(_) | WithGenericArgumentItem::BooleanLiteral(_) => {
                    if !matches!(x.bound, GenericBoundKind::Proto(_)) {
                        context.insert_error(AnalyzerError::mismatch_assignment(
                            "number",
                            &x.bound.to_string(),
                            &token,
                            &[],
                        ));
                    }
                }
                WithGenericArgumentItem::FixedType(_) => {
                    if !matches!(x.bound, GenericBoundKind::Type) {
                        context.insert_error(AnalyzerError::mismatch_assignment(
                            "type",
                            &x.bound.to_string(),
                            &token,
                            &[],
                        ));
                    }
                }
                WithGenericArgumentItem::GenericArgIdentifier(_) => {
                    // TODO
                }
            }
        }
    }
}

impl Conv<&WithParameterItem> for () {
    fn conv(context: &mut Context, value: &WithParameterItem) -> Self {
        let define_context: DefineContext = (&value.colon.colon_token).into();
        if !define_context.is_default() {
            return;
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Parameter(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind: VarKind = (&x.kind).into();
            let token: TokenRange = value.into();
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let Some(r#type) = x.r#type.to_ir_type(context) else {
                context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                return;
            };

            let Some(expr) = &x.value else {
                context.insert_error(AnalyzerError::missing_default_argument(
                    &value.identifier.text().to_string(),
                    &value.identifier.as_ref().into(),
                ));
                return;
            };

            let (typed_value, _) = eval_expr(context, Some(r#type.clone()), expr);

            // Get overridden parameter if it exists
            let value = context
                .get_override(&path)
                .cloned()
                .unwrap_or_else(|| typed_value.value.clone());

            match value {
                ValueVariant::Numeric(value) => {
                    let id = context.insert_var_path(path.clone(), typed_value);
                    let variable = Variable::new(
                        id,
                        path,
                        kind,
                        r#type,
                        vec![value],
                        context.get_affiliation(),
                        &variable_token,
                    );
                    context.insert_variable(id, variable);
                }
                ValueVariant::NumericArray(_) => {
                    // TODO for param array
                }
                ValueVariant::Type(x) => {
                    let mut typed_value = typed_value.clone();
                    typed_value.value = ValueVariant::Type(x);
                    context.insert_var_path(path, typed_value);
                }
                _ => {
                    context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                }
            }
        }
    }
}

impl Conv<&PortDeclarationItem> for () {
    fn conv(context: &mut Context, value: &PortDeclarationItem) -> Self {
        let define_context: DefineContext = (&value.colon.colon_token).into();
        if !define_context.is_default() {
            return;
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Port(x) = symbol.found.kind
        {
            check_modport_in_port(context, value);
            check_port_direction(context, value);

            let path = VarPath::new(symbol.found.token.text);
            let kind = match x.direction {
                Direction::Input => VarKind::Input,
                Direction::Output => VarKind::Output,
                Direction::Inout => VarKind::Inout,
                _ => {
                    // TODO modport
                    return;
                }
            };
            let token: TokenRange = value.into();
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let Some(r#type) = x.r#type.to_ir_type(context) else {
                context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                return;
            };

            if let Some(x) = &x.default_value {
                let (typed_value, _) = eval_expr(context, Some(r#type.clone()), x);

                check_port_default_value(context, value, &typed_value, kind, x);

                let Some(value) = typed_value.get_value() else {
                    context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                    return;
                };

                let types = r#type.expand(context, &path);

                for x in types {
                    let path = x.path;
                    let r#type = x.r#type;
                    let value = value.select(x.beg, x.end);

                    let mut typed_value = typed_value.clone();
                    typed_value.value = ValueVariant::Numeric(value.clone());
                    typed_value.r#type = r#type.clone();

                    // TODO for array
                    let id = context.insert_var_path(path.clone(), typed_value);
                    let variable = Variable::new(
                        id,
                        path,
                        kind,
                        r#type,
                        vec![value],
                        context.get_affiliation(),
                        &variable_token,
                    );
                    context.insert_variable(id, variable);
                }
            } else {
                for x in r#type.expand(context, &path) {
                    let path = x.path;
                    let r#type = x.r#type;

                    let mut values = vec![];
                    for _ in 0..r#type.total_array() {
                        values.push(Value::new_x(r#type.total_width(), false));
                    }

                    let typed_value = TypedValue::from_type(r#type.clone());
                    let id = context.insert_var_path(path.clone(), typed_value);
                    let variable = Variable::new(
                        id,
                        path,
                        kind,
                        r#type,
                        values,
                        context.get_affiliation(),
                        &variable_token,
                    );
                    context.insert_variable(id, variable);
                }
            }
        }
    }
}

impl Conv<&AlwaysFfDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysFfDeclaration) -> Self {
        // TODO explicit clock / reset, now dummy VarId is used.
        let clock = context.get_default_clock().unwrap_or_default();
        let reset = context.get_default_reset().unwrap_or_default();

        context.affiliation.push(Affiliation::AlwaysFf);

        let statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
        let statements = statements.0;

        context.affiliation.pop();

        ir::Declaration::new_ff(clock, reset, statements)
    }
}

impl Conv<&AlwaysCombDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysCombDeclaration) -> Self {
        context.affiliation.push(Affiliation::AlwaysComb);

        let statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
        let statements = statements.0;

        context.affiliation.pop();

        ir::Declaration::new_comb(statements)
    }
}

impl Conv<&VarDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &VarDeclaration) -> Self {
        let define_context: DefineContext = (&value.var.var_token).into();
        if !define_context.is_default() {
            return ir::Declaration::Null;
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Variable(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind = VarKind::Variable;
            let token: TokenRange = value.into();
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let Some(r#type) = x.r#type.to_ir_type(context) else {
                context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                return ir::Declaration::Null;
            };

            for x in r#type.expand(context, &path) {
                let path = x.path;
                let r#type = x.r#type;

                let mut values = vec![];
                for _ in 0..r#type.total_array() {
                    values.push(Value::new_x(r#type.total_width(), false));
                }

                let typed_value = TypedValue::from_type(r#type.clone());
                let id = context.insert_var_path(path.clone(), typed_value);
                let variable = Variable::new(
                    id,
                    path,
                    kind,
                    r#type,
                    values,
                    context.get_affiliation(),
                    &variable_token,
                );
                context.insert_variable(id, variable);
            }
        }
        ir::Declaration::Null
    }
}

impl Conv<&LetDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &LetDeclaration) -> Self {
        let define_context: DefineContext = (&value.r#let.let_token).into();
        if !define_context.is_default() {
            return ir::Declaration::Null;
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Variable(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind = VarKind::Variable;
            let token: TokenRange = value.into();
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let Some(r#type) = x.r#type.to_ir_type(context) else {
                context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                return ir::Declaration::Null;
            };

            let mut dst = vec![];
            for x in r#type.expand(context, &path) {
                let path = x.path;
                let r#type = x.r#type;

                let mut values = vec![];
                for _ in 0..r#type.total_array() {
                    values.push(Value::new_x(r#type.total_width(), false));
                }

                let typed_value = TypedValue::from_type(r#type.clone());
                let id = context.insert_var_path(path.clone(), typed_value);
                let variable = Variable::new(
                    id,
                    path,
                    kind,
                    r#type.clone(),
                    values,
                    context.get_affiliation(),
                    &variable_token,
                );
                context.insert_variable(id, variable);

                dst.push(ir::AssignDestination {
                    id,
                    index: VarIndex::default(),
                    select: VarSelect::default(),
                    r#type: r#type.clone(),
                    token: variable_token,
                });
            }

            let (_, expr) = eval_expr(context, Some(r#type.clone()), &value.expression);
            let exprs = eval_array_literal(context, Some(&r#type.array), &expr);
            if let Some(exprs) = exprs {
                let mut statements = vec![];
                for (i, expr) in exprs.into_iter().enumerate() {
                    let index = VarIndex::from_index(i, &r#type.array);
                    let mut dst = dst.clone();
                    for d in &mut dst {
                        d.index = index.clone();
                    }
                    let statement = ir::Statement::Assign(ir::AssignStatement { dst, expr, token });
                    statements.push(statement);
                }
                ir::Declaration::new_comb(statements)
            } else {
                let statement = ir::Statement::Assign(ir::AssignStatement { dst, expr, token });
                ir::Declaration::new_comb(vec![statement])
            }
        } else {
            ir::Declaration::Null
        }
    }
}

impl Conv<&ConstDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &ConstDeclaration) -> Self {
        let define_context: DefineContext = (&value.r#const.const_token).into();
        if !define_context.is_default() {
            return ir::Declaration::Null;
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Parameter(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind: VarKind = (&x.kind).into();
            let token: TokenRange = value.into();
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let Some(r#type) = x.r#type.to_ir_type(context) else {
                context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                return ir::Declaration::Null;
            };

            let Some(expr) = &x.value else {
                context.insert_error(AnalyzerError::missing_default_argument(
                    &value.identifier.text().to_string(),
                    &value.identifier.as_ref().into(),
                ));
                return ir::Declaration::Null;
            };

            let (typed_value, _) = eval_expr(context, Some(r#type.clone()), expr);

            let Some(value) = typed_value.get_value() else {
                context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                return ir::Declaration::Null;
            };

            // TODO for param array
            let id = context.insert_var_path(path.clone(), typed_value);
            let variable = Variable::new(
                id,
                path,
                kind,
                r#type,
                vec![value],
                context.get_affiliation(),
                &variable_token,
            );
            context.insert_variable(id, variable);
        }
        ir::Declaration::Null
    }
}

impl Conv<&AssignDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AssignDeclaration) -> Self {
        let define_context: DefineContext = (&value.assign.assign_token).into();
        if !define_context.is_default() {
            return ir::Declaration::Null;
        }

        let token: TokenRange = value.into();

        match value.assign_destination.as_ref() {
            AssignDestination::HierarchicalIdentifier(x) => {
                let ident = x.hierarchical_identifier.as_ref();
                let dst: VarPathSelect = Conv::conv(context, ident);

                if let Some(dst) = dst.to_assign_destination(context) {
                    let (_, expr) = eval_expr(context, Some(dst.r#type.clone()), &value.expression);
                    let statement = ir::Statement::Assign(ir::AssignStatement {
                        dst: vec![dst],
                        expr,
                        token,
                    });
                    ir::Declaration::Comb(ir::CombDeclaration {
                        statements: vec![statement],
                    })
                } else {
                    context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                    ir::Declaration::Null
                }
            }
            AssignDestination::LBraceAssignConcatenationListRBrace(x) => {
                let items: Vec<_> = x.assign_concatenation_list.as_ref().into();

                let mut dst = vec![];
                for item in items {
                    let ident = item.hierarchical_identifier.as_ref();
                    let x: VarPathSelect = Conv::conv(context, ident);
                    if let Some(x) = x.to_assign_destination(context) {
                        dst.push(x);
                    } else {
                        context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                        return ir::Declaration::Null;
                    }
                }

                let mut total_width = 0;
                for x in &dst {
                    total_width += x.r#type.total_width();
                }

                let r#type = ir::Type::new(TypeKind::Logic, vec![total_width], false);

                let (_, expr) = eval_expr(context, Some(r#type), &value.expression);
                let statement = ir::Statement::Assign(ir::AssignStatement { dst, expr, token });
                ir::Declaration::Comb(ir::CombDeclaration {
                    statements: vec![statement],
                })
            }
        }
    }
}

impl Conv<&FunctionDeclaration> for () {
    fn conv(context: &mut Context, value: &FunctionDeclaration) -> Self {
        let name = value.identifier.text();

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Function(x) = symbol.found.kind
        {
            let ret_type = if let Some(x) = &x.ret {
                let Some(r#type) = x.to_ir_type(context) else {
                    let token: TokenRange = value.into();
                    context.insert_error(AnalyzerError::unsupported_by_ir(&token));
                    return;
                };

                let typed_value = TypedValue::from_type(r#type);
                Some(typed_value)
            } else {
                None
            };

            // insert VarPath for function before statement_block conv
            // because it may be refered by recursive function
            let path = VarPath::new(name);
            let id = context.insert_func_path(path.clone(), ret_type.clone());

            context.affiliation.push(Affiliation::Function);
            context.push_hier(name);

            // insert return value as variable
            let ret_id = if let Some(ret_type) = ret_type.clone() {
                let path = VarPath::new(get_return_str());
                let kind = VarKind::Variable;
                let token: TokenRange = (&value.identifier.identifier_token).into();
                let r#type = ret_type.r#type.clone();

                // type.expand is not necessary
                // because member access is not allowed for return value
                let mut values = vec![];
                for _ in 0..r#type.total_array() {
                    values.push(Value::new_x(r#type.total_width(), false));
                }

                let ret_id = context.insert_var_path(path.clone(), ret_type);
                let variable = Variable::new(
                    ret_id,
                    path,
                    kind,
                    r#type,
                    values,
                    context.get_affiliation(),
                    &token,
                );
                context.insert_variable(ret_id, variable);

                Some(ret_id)
            } else {
                None
            };

            let mut ports = HashMap::default();

            if let Some(x) = &value.function_declaration_opt0
                && let Some(x) = &x.port_declaration.port_declaration_opt
            {
                let items: Vec<_> = x.port_declaration_list.as_ref().into();
                for item in items {
                    let _: () = Conv::conv(context, item);

                    let name = item.identifier.text();
                    let path = VarPath::new(name);
                    if let Some((id, _)) = context.var_paths.get(&path) {
                        ports.insert(name, *id);
                    }
                }
            }

            let statements: ir::StatementBlock =
                Conv::conv(context, value.statement_block.as_ref());
            let body = ir::FunctionBody {
                ret: ret_id,
                ports,
                statements: statements.0,
            };
            let r#type = ret_type.as_ref().map(|x| x.r#type.clone());
            let function = ir::Function {
                id,
                path,
                r#type,
                functions: vec![body],
            };

            context.pop_hier();
            context.affiliation.pop();

            // function should be inserted outside the function scope
            context.insert_function(id, function);
        }
    }
}

impl Conv<&EnumDeclaration> for () {
    fn conv(context: &mut Context, value: &EnumDeclaration) -> Self {
        check_enum(context, value);
    }
}

impl Conv<&InitialDeclaration> for () {
    fn conv(context: &mut Context, value: &InitialDeclaration) -> Self {
        let _statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
    }
}

impl Conv<&FinalDeclaration> for () {
    fn conv(context: &mut Context, value: &FinalDeclaration) -> Self {
        let _statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
    }
}

impl Conv<&InstDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &InstDeclaration) -> Self {
        let define_context: DefineContext = (&value.inst.inst_token).into();
        if !define_context.is_default() {
            return ir::Declaration::Null;
        }

        let value = value.component_instantiation.as_ref();
        let Some((component, id)) = get_component(context, value) else {
            return ir::Declaration::Null;
        };
        let symbol = symbol_table::get(id).unwrap();

        match component {
            ir::Component::Module(component) => {
                let mut port_types = HashMap::default();
                if let SymbolKind::Module(x) = &symbol.kind {
                    for port in &x.ports {
                        let name = port.name();
                        let property = port.property();
                        if let Some(dst_type) = property.r#type.to_ir_type(context) {
                            port_types.insert(name, dst_type);
                        }
                    }
                }

                let mut inputs = HashMap::default();
                let mut outputs = HashMap::default();
                if let Some(x) = &value.component_instantiation_opt2
                    && let Some(x) = &x.inst_port.inst_port_opt
                {
                    let ports: Vec<_> = x.inst_port_list.as_ref().into();
                    for port in ports {
                        let name = port.identifier.text();
                        let path = VarPath::new(name);
                        let token: TokenRange = port.identifier.as_ref().into();
                        if let Some(dst_type) = port_types.get(&name) {
                            let (dst, expr) = if let Some(x) = &port.inst_port_item_opt {
                                // Check type compatibility for all kind
                                let (_, expr) =
                                    eval_expr(context, Some(dst_type.clone()), &x.expression);
                                let dst: Vec<VarPathSelect> =
                                    Conv::conv(context, x.expression.as_ref());
                                (dst, expr)
                            } else {
                                let expr =
                                    if let Some((var_id, typed_value)) = context.find_path(&path) {
                                        ir::Expression::Term(Box::new(ir::Factor::Variable(
                                            var_id,
                                            VarIndex::default(),
                                            VarSelect::default(),
                                            typed_value,
                                            token,
                                        )))
                                    } else {
                                        ir::Expression::Term(Box::new(ir::Factor::Unknown(token)))
                                    };
                                let dst =
                                    vec![VarPathSelect(path.clone(), VarSelect::default(), token)];
                                (dst, expr)
                            };

                            if let Some(id) = component.ports.get(&path)
                                && let Some(variable) = component.variables.get(id)
                            {
                                match variable.kind {
                                    VarKind::Input => {
                                        inputs.insert(*id, expr);
                                    }
                                    VarKind::Output => {
                                        let dst: Vec<_> = dst
                                            .into_iter()
                                            .filter_map(|x| x.to_assign_destination(context))
                                            .collect();
                                        outputs.insert(*id, dst);
                                    }
                                    _ => (),
                                }
                            }
                        }
                    }
                }

                let name = symbol.token.text;
                let component = ir::Component::Module(component);
                ir::Declaration::Inst(ir::InstDeclaration {
                    name,
                    inputs,
                    outputs,
                    component,
                })
            }
            ir::Component::Interface(component) => {
                let base = value.identifier.text();
                let mut rename_table = HashMap::default();

                let mut inserted = HashSet::default();

                for mut variable in component.variables.into_values() {
                    inserted.insert(variable.path.clone());
                    let typed_value = TypedValue::from_type(variable.r#type.clone());
                    variable.path.add_prelude(&[base]);
                    let id = context.insert_var_path(variable.path.clone(), typed_value);
                    rename_table.insert(variable.id, id);
                    variable.id = id;
                    context.insert_variable(id, variable);
                }

                for (mut path, (id, typed_value)) in component.var_paths {
                    if !inserted.contains(&path) {
                        path.add_prelude(&[base]);
                        let new_id = context.insert_var_path(path, typed_value);
                        rename_table.insert(id, new_id);
                    }
                }

                let mut functions = Vec::new();
                for mut function in component.functions.into_values() {
                    let typed_value = function
                        .r#type
                        .as_ref()
                        .map(|x| TypedValue::from_type(x.clone()));
                    function.path.add_prelude(&[base]);
                    let id = context.insert_func_path(function.path.clone(), typed_value);
                    rename_table.insert(function.id, id);
                    function.id = id;
                    functions.push((id, function));
                }

                for (id, mut function) in functions {
                    function.rename(&rename_table);
                    context.insert_function(id, function);
                }

                // insert path of interface instance
                let path = VarPath::new(value.identifier.text());
                let r#type = ir::Type::new(TypeKind::UserDefined(symbol.id), vec![], false);
                let typed_value = TypedValue::from_type(r#type.clone());
                context.insert_var_path(path.clone(), typed_value);

                ir::Declaration::Null
            }
        }
    }
}

impl Conv<&ConnectDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &ConnectDeclaration) -> Self {
        let token: TokenRange = value.into();

        // TODO enable after removing checker_connect_operation
        //check_connect(
        //    context,
        //    value.hierarchical_identifier.as_ref(),
        //    value.expression.as_ref(),
        //);

        let lhs: VarPathSelect = Conv::conv(context, value.hierarchical_identifier.as_ref());
        let rhs: Vec<VarPathSelect> = Conv::conv(context, value.expression.as_ref());

        if rhs.len() != 1 {
            // TODO error
            return ir::DeclarationBlock::default();
        }

        let rhs = rhs[0].clone();

        let statements = expand_connect(context, lhs, rhs, token);
        let ret = ir::CombDeclaration { statements };
        let ret = ir::Declaration::Comb(ret);
        ir::DeclarationBlock(vec![ret])
    }
}

impl Conv<&ModportDeclaration> for () {
    fn conv(context: &mut Context, value: &ModportDeclaration) -> Self {
        context.affiliation.push(Affiliation::Modport);

        if let Some(x) = &value.modport_declaration_opt {
            let items: Vec<_> = x.modport_list.as_ref().into();
            for item in &items {
                check_modport(context, item);
                check_direction(context, &item.direction);
            }
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref()) {
            let name = value.identifier.text();
            let path = VarPath::new(name);
            let r#type = ir::Type {
                kind: ir::TypeKind::UserDefined(symbol.found.id),
                signed: false,
                width: vec![],
                array: vec![],
            };
            let typed_value = TypedValue::from_type(r#type);
            context.insert_var_path(path, typed_value);
        }

        context.affiliation.pop();
    }
}
