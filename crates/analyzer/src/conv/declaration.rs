use crate::analyzer_error::AnalyzerError;
use crate::conv::checker::r#enum::check_enum;
use crate::conv::checker::modport::{check_modport, check_modport_in_port};
use crate::conv::checker::port::{check_direction, check_port_default_value, check_port_direction};
use crate::conv::utils::{
    eval_array_literal, eval_expr, eval_range, expand_connect, expand_connect_const, get_component,
    get_overridden_params, get_port_connects, get_return_str, insert_port_connect,
};
use crate::conv::{Affiliation, Context, Conv};
use crate::ir::{
    self, Comptime, FuncPath, IrResult, Signature, TypeKind, ValueVariant, VarIndex, VarKind,
    VarPath, VarPathSelect, VarSelect, Variable,
};
use crate::namespace::DefineContext;
use crate::symbol::{Direction, GenericBoundKind, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use crate::value::Value;
use crate::{HashMap, ir_error};
use num_bigint::BigUint;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&GenerateItem> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateItem) -> IrResult<Self> {
        match value {
            GenerateItem::LetDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.let_declaration.as_ref(),
            )?)),
            GenerateItem::VarDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.var_declaration.as_ref(),
            )?)),
            GenerateItem::AlwaysFfDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.always_ff_declaration.as_ref(),
            )?)),
            GenerateItem::AlwaysCombDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.always_comb_declaration.as_ref(),
            )?)),
            GenerateItem::GenerateIfDeclaration(x) => {
                Conv::conv(context, x.generate_if_declaration.as_ref())
            }
            GenerateItem::GenerateForDeclaration(x) => {
                Conv::conv(context, x.generate_for_declaration.as_ref())
            }
            GenerateItem::ConstDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.const_declaration.as_ref(),
            )?)),
            GenerateItem::AssignDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.assign_declaration.as_ref(),
            )?)),
            GenerateItem::FunctionDeclaration(x) => {
                let _: () = Conv::conv(context, x.function_declaration.as_ref())?;
                Ok(ir::DeclarationBlock::default())
            }
            GenerateItem::EnumDeclaration(x) => {
                let _: () = Conv::conv(context, x.enum_declaration.as_ref())?;
                Ok(ir::DeclarationBlock::default())
            }
            GenerateItem::InitialDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.initial_declaration.as_ref(),
            )?)),
            GenerateItem::FinalDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.final_declaration.as_ref(),
            )?)),
            GenerateItem::InstDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.inst_declaration.as_ref(),
            )?)),
            GenerateItem::ConnectDeclaration(x) => {
                Conv::conv(context, x.connect_declaration.as_ref())
            }
            GenerateItem::UnsafeBlock(x) => Conv::conv(context, x.unsafe_block.as_ref()),
            GenerateItem::AliasDeclaration(_) => Ok(ir::DeclarationBlock::default()),
            _ => {
                let token: TokenRange = value.into();
                Err(ir_error!(token))
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
    fn conv(context: &mut Context, value: &GenerateIfDeclaration) -> IrResult<Self> {
        let label = value.generate_named_block.identifier.text();

        let (comptime, _) = eval_expr(context, None, value.expression.as_ref())?;
        let cond = comptime.get_value()?;

        if cond.to_usize() != 0 {
            context.push_hier(label);
            let block: IrResult<ir::DeclarationBlock> =
                Conv::conv(context, value.generate_named_block.as_ref());
            context.pop_hier();
            Ok(block?)
        } else {
            for x in &value.generate_if_declaration_list {
                let (comptime, _) = eval_expr(context, None, x.expression.as_ref())?;

                let cond = comptime.get_value()?;

                if cond.to_usize() != 0 {
                    let label = get_label(&x.generate_optional_named_block, label);

                    context.push_hier(label);
                    let block: IrResult<ir::DeclarationBlock> =
                        Conv::conv(context, x.generate_optional_named_block.as_ref());
                    context.pop_hier();
                    return block;
                }
            }

            if let Some(x) = &value.generate_if_declaration_opt {
                let label = get_label(&x.generate_optional_named_block, label);

                context.push_hier(label);
                let block: IrResult<ir::DeclarationBlock> =
                    Conv::conv(context, x.generate_optional_named_block.as_ref());
                context.pop_hier();
                block
            } else {
                Ok(ir::DeclarationBlock::default())
            }
        }
    }
}

impl Conv<&GenerateNamedBlock> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateNamedBlock) -> IrResult<Self> {
        let mut ret = vec![];
        for x in &value.generate_named_block_list {
            let items: Vec<_> = x.generate_group.as_ref().into();
            for item in items {
                let item: IrResult<ir::DeclarationBlock> = Conv::conv(context, item);
                context.insert_ir_error(&item);

                if let Ok(mut item) = item {
                    ret.append(&mut item.0);
                }
            }
        }
        Ok(ir::DeclarationBlock(ret))
    }
}

impl Conv<&GenerateOptionalNamedBlock> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateOptionalNamedBlock) -> IrResult<Self> {
        let mut ret = vec![];
        for x in &value.generate_optional_named_block_list {
            let items: Vec<_> = x.generate_group.as_ref().into();
            for item in items {
                let item: IrResult<ir::DeclarationBlock> = Conv::conv(context, item);
                context.insert_ir_error(&item);

                if let Ok(mut item) = item {
                    ret.append(&mut item.0);
                }
            }
        }
        Ok(ir::DeclarationBlock(ret))
    }
}

impl Conv<&GenerateForDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateForDeclaration) -> IrResult<Self> {
        let label = value.generate_named_block.identifier.text();
        let (beg, end) = eval_range(context, &value.range)?;

        let mut ret = ir::DeclarationBlock::default();

        for i in beg..end {
            let label = format!("{}[{}]", label, i);
            let label = resource_table::insert_str(&label);

            context.push_hier(label);

            let index = value.identifier.text();
            let token: TokenRange = (&value.identifier.identifier_token).into();
            let path = VarPath::new(index);
            let kind = VarKind::Const;
            let comptime = Comptime::create_value(BigUint::from(i), 32, token);

            let id = context.insert_var_path(path.clone(), comptime.clone());
            let variable = Variable::new(
                id,
                path,
                kind,
                comptime.r#type.clone(),
                vec![comptime.get_value().unwrap()],
                context.get_affiliation(),
                &token,
            );
            context.insert_variable(id, variable);

            let block: IrResult<ir::DeclarationBlock> =
                Conv::conv(context, value.generate_named_block.as_ref());
            context.insert_ir_error(&block);

            if let Ok(mut block) = block {
                ret.0.append(&mut block.0);
            }

            context.pop_hier();
        }

        Ok(ret)
    }
}

impl Conv<&WithGenericParameterItem> for () {
    fn conv(context: &mut Context, value: &WithGenericParameterItem) -> IrResult<Self> {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::GenericParameter(x) = symbol.found.kind
        {
            if let Some(y) = &value.with_generic_parameter_item_opt {
                let token: TokenRange = y.with_generic_argument_item.as_ref().into();

                match y.with_generic_argument_item.as_ref() {
                    WithGenericArgumentItem::Number(_)
                    | WithGenericArgumentItem::BooleanLiteral(_) => {
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
            Ok(())
        } else {
            let token: TokenRange = value.identifier.as_ref().into();
            Err(ir_error!(token))
        }
    }
}

impl Conv<&WithParameterItem> for () {
    fn conv(context: &mut Context, value: &WithParameterItem) -> IrResult<Self> {
        let define_context: DefineContext = (&value.colon.colon_token).into();
        if !define_context.is_default() {
            return Ok(());
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Parameter(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind: VarKind = (&x.kind).into();
            let token: TokenRange = value.into();
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let r#type = x.r#type.to_ir_type(context)?;

            let Some(expr) = &x.value else {
                context.insert_error(AnalyzerError::missing_default_argument(
                    &value.identifier.text().to_string(),
                    &value.identifier.as_ref().into(),
                ));
                return Err(ir_error!(token));
            };

            let (comptime, _) = eval_expr(context, Some(r#type.clone()), expr)?;

            // Get overridden parameter if it exists
            let value = context
                .get_override(&path)
                .cloned()
                .unwrap_or_else(|| comptime.value.clone());

            match value {
                ValueVariant::Numeric(value) => {
                    let id = context.insert_var_path(path.clone(), comptime);
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
                    return Err(ir_error!(token));
                }
                ValueVariant::Type(x) => {
                    let mut comptime = comptime.clone();
                    comptime.value = ValueVariant::Type(x);
                    context.insert_var_path(path, comptime);
                }
                _ => {
                    return Err(ir_error!(token));
                }
            }
            Ok(())
        } else {
            let token: TokenRange = value.identifier.as_ref().into();
            Err(ir_error!(token))
        }
    }
}

impl Conv<&PortDeclarationItem> for () {
    fn conv(context: &mut Context, value: &PortDeclarationItem) -> IrResult<Self> {
        let define_context: DefineContext = (&value.colon.colon_token).into();
        if !define_context.is_default() {
            return Ok(());
        }

        let token: TokenRange = value.into();

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Port(x) = symbol.found.kind
        {
            check_modport_in_port(context, value);
            check_port_direction(context, value);

            let path = VarPath::new(symbol.found.token.text);
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let r#type = x.r#type.to_ir_type(context)?;

            context.insert_port_type(path.clone(), r#type.clone());

            let kind = match x.direction {
                Direction::Input => VarKind::Input,
                Direction::Output => VarKind::Output,
                Direction::Inout => VarKind::Inout,
                Direction::Modport => {
                    if let ir::TypeKind::Modport(sig, name) = &r#type.kind {
                        let members = r#type.modport_members(&VarPath::default());
                        let component = get_component(context, sig, token)?;
                        let base = value.identifier.text();
                        let ir::Component::Interface(component) = component else {
                            return Err(ir_error!(token));
                        };
                        context.extract_interface_member(
                            base,
                            &r#type.array,
                            component,
                            Some(&members),
                            variable_token,
                        );

                        // insert path of modport instance
                        let path = VarPath::new(value.identifier.text());
                        let r#type = ir::Type::new(
                            TypeKind::Modport(sig.clone(), *name),
                            r#type.array,
                            vec![],
                            false,
                        );
                        let comptime = Comptime::from_type(r#type.clone(), variable_token);
                        context.insert_var_path(path.clone(), comptime);
                    }
                    // inserting modport is completed in this block
                    return Ok(());
                }
                _ => {
                    return Err(ir_error!(token));
                }
            };

            let default_value = if let Some(x) = &x.default_value {
                let default_value = eval_expr(context, Some(r#type.clone()), x);

                check_port_default_value(context, value, &default_value, kind, x);

                let (comptime, _) = default_value?;

                if x.is_anonymous_expression() {
                    None
                } else {
                    let value = comptime.get_value()?;
                    Some((value, comptime))
                }
            } else {
                None
            };

            if let Some((value, comptime)) = default_value {
                for x in r#type.expand(&path) {
                    let path = x.path;
                    let r#type = x.r#type;
                    let value = value.select(x.beg, x.end);

                    let mut comptime = comptime.clone();
                    comptime.value = ValueVariant::Numeric(value.clone());
                    comptime.r#type = r#type.clone();

                    // TODO for array
                    let id = context.insert_var_path(path.clone(), comptime);
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
                for x in r#type.expand(&path) {
                    let path = x.path;
                    let r#type = x.r#type;

                    let mut values = vec![];
                    for _ in 0..r#type.total_array() {
                        values.push(Value::new_x(r#type.total_width(), false));
                    }

                    let comptime = Comptime::from_type(r#type.clone(), variable_token);
                    let id = context.insert_var_path(path.clone(), comptime);
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
            Ok(())
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&AlwaysFfDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysFfDeclaration) -> IrResult<Self> {
        // TODO explicit clock / reset, now dummy VarId is used.
        let clock = context.get_default_clock().unwrap_or_default();
        let reset = context.get_default_reset().unwrap_or_default();

        context.affiliation.push(Affiliation::AlwaysFf);

        let statements: IrResult<ir::StatementBlock> =
            Conv::conv(context, value.statement_block.as_ref());

        context.affiliation.pop();

        let statements = statements?.0;
        Ok(ir::Declaration::new_ff(clock, reset, statements))
    }
}

impl Conv<&AlwaysCombDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysCombDeclaration) -> IrResult<Self> {
        context.affiliation.push(Affiliation::AlwaysComb);

        let statements: IrResult<ir::StatementBlock> =
            Conv::conv(context, value.statement_block.as_ref());

        context.affiliation.pop();

        let statements = statements?.0;
        Ok(ir::Declaration::new_comb(statements))
    }
}

impl Conv<&VarDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &VarDeclaration) -> IrResult<Self> {
        let define_context: DefineContext = (&value.var.var_token).into();
        if !define_context.is_default() {
            return Ok(ir::Declaration::Null);
        }

        let token: TokenRange = value.into();

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Variable(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind = VarKind::Variable;
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let r#type = x.r#type.to_ir_type(context)?;

            for x in r#type.expand(&path) {
                let path = x.path;
                let r#type = x.r#type;

                let mut values = vec![];
                for _ in 0..r#type.total_array() {
                    values.push(Value::new_x(r#type.total_width(), false));
                }

                let comptime = Comptime::from_type(r#type.clone(), variable_token);
                let id = context.insert_var_path(path.clone(), comptime);
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
            Ok(ir::Declaration::Null)
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&LetDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &LetDeclaration) -> IrResult<Self> {
        let define_context: DefineContext = (&value.r#let.let_token).into();
        if !define_context.is_default() {
            return Ok(ir::Declaration::Null);
        }

        let token: TokenRange = value.into();

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Variable(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind = VarKind::Variable;
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let r#type = x.r#type.to_ir_type(context)?;

            let mut dst = vec![];
            let mut width = 0;
            for x in r#type.expand(&path) {
                let path = x.path;
                let r#type = x.r#type;

                let mut values = vec![];
                for _ in 0..r#type.total_array() {
                    values.push(Value::new_x(r#type.total_width(), false));
                }

                let comptime = Comptime::from_type(r#type.clone(), variable_token);
                let id = context.insert_var_path(path.clone(), comptime);
                let variable = Variable::new(
                    id,
                    path.clone(),
                    kind,
                    r#type.clone(),
                    values,
                    context.get_affiliation(),
                    &variable_token,
                );
                context.insert_variable(id, variable);

                let ret = ir::AssignDestination {
                    id,
                    path,
                    index: VarIndex::default(),
                    select: VarSelect::default(),
                    r#type: r#type.clone(),
                    token: variable_token,
                };

                let x = ret.total_width(context).ok_or_else(|| ir_error!(token))?;
                width += x;

                dst.push(ret);
            }

            let (_, expr) = eval_expr(context, Some(r#type.clone()), &value.expression)?;
            let exprs = eval_array_literal(context, Some(&r#type.array), &expr)?;
            if let Some(exprs) = exprs {
                let mut statements = vec![];
                for (i, expr) in exprs.into_iter().enumerate() {
                    let index = VarIndex::from_index(i, &r#type.array);
                    let mut dst = dst.clone();
                    for d in &mut dst {
                        d.index = index.clone();
                    }
                    let statement = ir::Statement::Assign(ir::AssignStatement {
                        dst,
                        width,
                        expr,
                        token,
                    });
                    statements.push(statement);
                }
                Ok(ir::Declaration::new_comb(statements))
            } else {
                let statement = ir::Statement::Assign(ir::AssignStatement {
                    dst,
                    width,
                    expr,
                    token,
                });
                Ok(ir::Declaration::new_comb(vec![statement]))
            }
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&ConstDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &ConstDeclaration) -> IrResult<Self> {
        let define_context: DefineContext = (&value.r#const.const_token).into();
        if !define_context.is_default() {
            return Ok(ir::Declaration::Null);
        }

        let token: TokenRange = value.into();

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Parameter(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind: VarKind = (&x.kind).into();
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let r#type = x.r#type.to_ir_type(context)?;

            let Some(expr) = &x.value else {
                context.insert_error(AnalyzerError::missing_default_argument(
                    &value.identifier.text().to_string(),
                    &value.identifier.as_ref().into(),
                ));
                return Err(ir_error!(token));
            };

            let (comptime, _) = eval_expr(context, Some(r#type.clone()), expr)?;

            let value = comptime.get_value()?;

            // TODO for param array
            let id = context.insert_var_path(path.clone(), comptime);
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
            Ok(ir::Declaration::Null)
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&AssignDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AssignDeclaration) -> IrResult<Self> {
        let define_context: DefineContext = (&value.assign.assign_token).into();
        if !define_context.is_default() {
            return Ok(ir::Declaration::Null);
        }

        let token: TokenRange = value.into();

        match value.assign_destination.as_ref() {
            AssignDestination::HierarchicalIdentifier(x) => {
                let ident = x.hierarchical_identifier.as_ref();
                let dst: VarPathSelect = Conv::conv(context, ident)?;

                if let Some(dst) = dst.to_assign_destination(context)
                    && let Some(width) = dst.total_width(context)
                {
                    let (_, expr) =
                        eval_expr(context, Some(dst.r#type.clone()), &value.expression)?;
                    let statement = ir::Statement::Assign(ir::AssignStatement {
                        dst: vec![dst],
                        width,
                        expr,
                        token,
                    });
                    Ok(ir::Declaration::Comb(ir::CombDeclaration {
                        statements: vec![statement],
                    }))
                } else {
                    Err(ir_error!(token))
                }
            }
            AssignDestination::LBraceAssignConcatenationListRBrace(x) => {
                let items: Vec<_> = x.assign_concatenation_list.as_ref().into();

                let mut dst = vec![];
                for item in items {
                    let ident = item.hierarchical_identifier.as_ref();
                    let x: VarPathSelect = Conv::conv(context, ident)?;
                    if let Some(x) = x.to_assign_destination(context) {
                        dst.push(x);
                    } else {
                        return Err(ir_error!(token));
                    }
                }

                let mut width = 0;
                for x in &dst {
                    let x = x.total_width(context).ok_or_else(|| ir_error!(token))?;
                    width += x;
                }

                let r#type = ir::Type::new(TypeKind::Logic, vec![], vec![width], false);

                let (_, expr) = eval_expr(context, Some(r#type), &value.expression)?;
                let statement = ir::Statement::Assign(ir::AssignStatement {
                    dst,
                    width,
                    expr,
                    token,
                });
                Ok(ir::Declaration::Comb(ir::CombDeclaration {
                    statements: vec![statement],
                }))
            }
        }
    }
}

impl Conv<&FunctionDeclaration> for () {
    fn conv(context: &mut Context, value: &FunctionDeclaration) -> IrResult<Self> {
        let name = value.identifier.text();

        let token: TokenRange = (&value.identifier.identifier_token).into();

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Function(x) = symbol.found.kind
        {
            let ret_type = if let Some(x) = &x.ret {
                let r#type = x.to_ir_type(context)?;
                let comptime = Comptime::from_type(r#type, token);
                Some(comptime)
            } else {
                None
            };

            // insert VarPath for function before statement_block conv
            // because it may be refered by recursive function
            let path = FuncPath::new(symbol.found.id);
            let id = context.insert_func_path(path.clone(), ret_type.clone());

            context.affiliation.push(Affiliation::Function);
            context.push_hier(name);

            // insert return value as variable
            let ret_id = if let Some(ret_type) = ret_type.clone() {
                let path = VarPath::new(get_return_str());
                let kind = VarKind::Variable;
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
                    let ret: IrResult<()> = Conv::conv(context, item);
                    context.insert_ir_error(&ret);

                    let name = item.identifier.text();
                    let path = VarPath::new(name);
                    if let Some((id, comptime)) = context.var_paths.get(&path) {
                        let members = comptime.r#type.modport_members(&path);
                        for (path, _) in members.into_values() {
                            if let Some((id, _)) = context.var_paths.get(&path) {
                                ports.insert(path, *id);
                            }
                        }
                        ports.insert(path, *id);
                    }
                }
            }

            let statements: IrResult<ir::StatementBlock> =
                Conv::conv(context, value.statement_block.as_ref());

            context.pop_hier();
            context.affiliation.pop();

            let statements = statements?;

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
                array: vec![],
                functions: vec![body],
            };

            // function should be inserted outside the function scope
            context.insert_function(id, function);
            Ok(())
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&EnumDeclaration> for () {
    fn conv(context: &mut Context, value: &EnumDeclaration) -> IrResult<Self> {
        check_enum(context, value);
        Ok(())
    }
}

impl Conv<&InitialDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &InitialDeclaration) -> IrResult<Self> {
        let statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref())?;
        Ok(ir::Declaration::Initial(ir::InitialDeclaration {
            statements: statements.0,
        }))
    }
}

impl Conv<&FinalDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &FinalDeclaration) -> IrResult<Self> {
        let statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref())?;
        Ok(ir::Declaration::Final(ir::FinalDeclaration {
            statements: statements.0,
        }))
    }
}

impl Conv<&InstDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &InstDeclaration) -> IrResult<Self> {
        let define_context: DefineContext = (&value.inst.inst_token).into();
        if !define_context.is_default() {
            return Ok(ir::Declaration::Null);
        }

        let value = value.component_instantiation.as_ref();
        let token: TokenRange = value.identifier.as_ref().into();
        let generic_path: GenericSymbolPath = value.scoped_identifier.as_ref().into();

        let mut sig =
            Signature::from_path(context, generic_path).ok_or_else(|| ir_error!(token))?;
        let symbol = symbol_table::get(sig.symbol).unwrap();

        let parameters = symbol.kind.get_parameters();
        let overridden_params = get_overridden_params(context, value)?;
        for x in parameters {
            let path = VarPath::new(x.name);
            if let Some(value) = overridden_params.get(&path) {
                sig.add_parameter(x.name, value.clone());
            }
        }

        context.overrides.push(overridden_params);
        let component = get_component(context, &sig, token);
        context.overrides.pop();

        let component = component?;

        match component {
            ir::Component::Module(component) => {
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
                        if let Some(dst_type) = component.port_types.get(&path) {
                            let connects =
                                get_port_connects(context, port, &path, dst_type, token)?;

                            for (path, dst, expr) in connects {
                                if let Some(id) = component.ports.get(&path)
                                    && let Some(variable) = component.variables.get(id)
                                {
                                    insert_port_connect(
                                        context,
                                        variable,
                                        dst,
                                        expr,
                                        &mut inputs,
                                        &mut outputs,
                                    );
                                }
                            }
                        }
                    }
                }

                // TOOD modport parameter override

                let name = value.identifier.text();
                let component = ir::Component::Module(component);
                Ok(ir::Declaration::Inst(ir::InstDeclaration {
                    name,
                    inputs,
                    outputs,
                    component,
                }))
            }
            ir::Component::Interface(component) => {
                let mut array = vec![];
                if let Some(x) = &value.component_instantiation_opt0 {
                    let exprs: Vec<_> = x.array.as_ref().into();
                    for expr in exprs {
                        let (comptime, _) = eval_expr(context, None, expr)?;
                        let x = comptime.get_value()?;
                        array.push(x.to_usize())
                    }
                }

                let base = value.identifier.text();
                context.extract_interface_member(base, &array, component, None, token);

                // insert path of interface instance
                let path = VarPath::new(value.identifier.text());
                let r#type = ir::Type::new(TypeKind::Interface(sig), array, vec![], false);
                let comptime = Comptime::from_type(r#type.clone(), token);
                context.insert_var_path(path.clone(), comptime);

                Ok(ir::Declaration::Null)
            }
            ir::Component::SystemVerilog(mut component) => {
                if let Some(x) = &value.component_instantiation_opt2
                    && let Some(x) = &x.inst_port.inst_port_opt
                {
                    let ports: Vec<_> = x.inst_port_list.as_ref().into();
                    let mut dst_paths = vec![];
                    for port in ports {
                        let dst_path = if let Some(x) = &port.inst_port_item_opt {
                            let dst: Vec<VarPathSelect> =
                                Conv::conv(context, x.expression.as_ref())?;
                            let x = dst.first().ok_or_else(|| ir_error!(token))?;
                            x.0.clone()
                        } else {
                            let name = port.identifier.text();
                            VarPath::new(name)
                        };
                        dst_paths.push(dst_path);
                    }
                    component.connects.append(&mut dst_paths);
                }
                let name = value.identifier.text();
                let component = ir::Component::SystemVerilog(component);
                Ok(ir::Declaration::Inst(ir::InstDeclaration {
                    name,
                    inputs: HashMap::default(),
                    outputs: HashMap::default(),
                    component,
                }))
            }
        }
    }
}

impl Conv<&ConnectDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &ConnectDeclaration) -> IrResult<Self> {
        let token: TokenRange = value.into();

        // TODO enable after removing checker_connect_operation
        //check_connect(
        //    context,
        //    value.hierarchical_identifier.as_ref(),
        //    value.expression.as_ref(),
        //);

        let lhs: VarPathSelect = Conv::conv(context, value.hierarchical_identifier.as_ref())?;
        let rhs: Vec<VarPathSelect> = Conv::conv(context, value.expression.as_ref())?;

        let (comptime, _) = eval_expr(context, None, value.expression.as_ref())?;

        let statements = if comptime.is_const {
            expand_connect_const(context, lhs, comptime, token)
        } else {
            if rhs.len() != 1 {
                // TODO error
                return Err(ir_error!(token));
            }

            let rhs = rhs[0].clone();

            expand_connect(context, lhs, rhs, token)
        };

        let ret = ir::CombDeclaration { statements };
        let ret = ir::Declaration::Comb(ret);
        Ok(ir::DeclarationBlock(vec![ret]))
    }
}

impl Conv<&UnsafeBlock> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &UnsafeBlock) -> IrResult<Self> {
        let mut ret = vec![];
        for x in &value.unsafe_block_list {
            let items: Vec<_> = x.generate_group.as_ref().into();
            for item in items {
                let item: IrResult<ir::DeclarationBlock> = Conv::conv(context, item);
                context.insert_ir_error(&item);

                if let Ok(mut item) = item {
                    ret.append(&mut item.0);
                }
            }
        }
        Ok(ir::DeclarationBlock(ret))
    }
}

impl Conv<&ModportDeclaration> for () {
    fn conv(context: &mut Context, value: &ModportDeclaration) -> IrResult<Self> {
        context.affiliation.push(Affiliation::Modport);

        if let Some(x) = &value.modport_declaration_opt {
            let items: Vec<_> = x.modport_list.as_ref().into();
            for item in &items {
                check_modport(context, item);
                check_direction(context, &item.direction);
            }
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Modport(x) = symbol.found.kind
        {
            let sig = if let Some(x) = context.instance_history.get_current_signature() {
                x.clone()
            } else {
                Signature::new(x.interface)
            };

            let name = value.identifier.text();
            let path = VarPath::new(name);
            let r#type = ir::Type {
                kind: ir::TypeKind::Modport(sig, symbol.found.token.text),
                signed: false,
                width: vec![],
                array: vec![],
            };
            let token: TokenRange = value.identifier.as_ref().into();
            let comptime = Comptime::from_type(r#type, token);
            context.insert_var_path(path, comptime);
        }

        context.affiliation.pop();

        Ok(())
    }
}
