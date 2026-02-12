use crate::analyzer_error::{AnalyzerError, UnevaluableValueKind};
use crate::conv::checker::alias::{AliasType, check_alias_target};
use crate::conv::checker::bind::check_bind_target;
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::conv::checker::generic::check_generic_args;
use crate::conv::checker::generic::check_generic_bound;
use crate::conv::checker::import::check_import;
use crate::conv::checker::inst::check_inst;
use crate::conv::checker::modport::{check_modport, check_modport_default, check_modport_in_port};
use crate::conv::checker::port::{check_direction, check_port_default_value, check_port_direction};
use crate::conv::utils::{
    TypePosition, eval_assign_statement, eval_clock, eval_const_assign, eval_expr, eval_for_range,
    eval_reset, eval_size, eval_type, eval_variable, expand_connect, expand_connect_const,
    get_component, get_overridden_params, get_port_connects, get_return_str, insert_port_connect,
    var_path_to_assign_destination,
};
use crate::conv::{Affiliation, Context, Conv};
use crate::ir::{
    self, Comptime, FuncArg, FuncPath, IrResult, Shape, Signature, TypeKind, ValueVariant, VarId,
    VarIndex, VarKind, VarPath, VarPathSelect, VarSelect, Variable,
};
use crate::namespace::DefineContext;
use crate::symbol::{ClockDomain, Direction, GenericBoundKind, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use crate::value::Value;
use crate::{HashMap, ir_error};
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
            GenerateItem::GenerateBlockDeclaration(x) => Conv::conv(
                context,
                x.generate_block_declaration.generate_named_block.as_ref(),
            ),
            GenerateItem::ConstDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.const_declaration.as_ref(),
            )?)),
            GenerateItem::AssignDeclaration(x) => Ok(ir::DeclarationBlock::new(Conv::conv(
                context,
                x.assign_declaration.as_ref(),
            )?)),
            GenerateItem::FunctionDeclaration(x) => {
                // ignore IrError of generic function
                let use_ir = context.config.use_ir;
                if x.function_declaration.function_declaration_opt.is_some() {
                    context.config.use_ir = false;
                    context.ignore_var_func = true;
                }

                let ret = context.block(|c| {
                    let ret: () = Conv::conv(c, x.function_declaration.as_ref())?;
                    Ok(ret)
                });

                if x.function_declaration.function_declaration_opt.is_some() {
                    context.config.use_ir = use_ir;
                    context.ignore_var_func = false;
                } else {
                    // check IrError for non-generic function
                    ret?;
                }

                Ok(ir::DeclarationBlock::default())
            }
            GenerateItem::StructUnionDeclaration(x) => {
                let _: () = Conv::conv(context, x.struct_union_declaration.as_ref())?;
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
            GenerateItem::ImportDeclaration(x) => {
                Conv::conv(context, x.import_declaration.as_ref())
            }
            GenerateItem::BindDeclaration(x) => {
                let _: () = Conv::conv(context, x.bind_declaration.as_ref())?;
                Ok(ir::DeclarationBlock::default())
            }
            GenerateItem::AliasDeclaration(x) => {
                let _: () = Conv::conv(context, x.alias_declaration.as_ref())?;
                Ok(ir::DeclarationBlock::default())
            }
            GenerateItem::TypeDefDeclaration(x) => {
                let _: () = Conv::conv(context, x.type_def_declaration.as_ref())?;
                Ok(ir::DeclarationBlock::default())
            }
            GenerateItem::EmbedDeclaration(x) => {
                let _: () = Conv::conv(context, x.embed_declaration.as_ref())?;
                Ok(ir::DeclarationBlock::default())
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

        let (comptime, _) = eval_expr(context, None, value.expression.as_ref(), false)?;
        let cond = comptime.get_value()?;

        if cond.to_usize().unwrap_or(0) != 0 {
            context.push_hierarchy(label);

            let block = context.block(|c| {
                let block: ir::DeclarationBlock =
                    Conv::conv(c, value.generate_named_block.as_ref())?;
                Ok(block)
            });

            context.pop_hierarchy();
            block
        } else {
            for x in &value.generate_if_declaration_list {
                let (comptime, _) = eval_expr(context, None, x.expression.as_ref(), false)?;

                let cond = comptime.get_value()?;

                if cond.to_usize().unwrap_or(0) != 0 {
                    let label = get_label(&x.generate_optional_named_block, label);

                    context.push_hierarchy(label);

                    let block = context.block(|c| {
                        let block: ir::DeclarationBlock =
                            Conv::conv(c, x.generate_optional_named_block.as_ref())?;
                        Ok(block)
                    });

                    context.pop_hierarchy();
                    return block;
                }
            }

            if let Some(x) = &value.generate_if_declaration_opt {
                let label = get_label(&x.generate_optional_named_block, label);

                context.push_hierarchy(label);

                let block = context.block(|c| {
                    let block: ir::DeclarationBlock =
                        Conv::conv(c, x.generate_optional_named_block.as_ref())?;
                    Ok(block)
                });

                context.pop_hierarchy();
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
        let token: TokenRange = (&value.identifier.identifier_token).into();
        let label = value.generate_named_block.identifier.text();

        let rev = value.generate_for_declaration_opt.is_some();
        let step = value
            .generate_for_declaration_opt0
            .as_ref()
            .map(|x| (x.assignment_operator.as_ref(), x.expression.as_ref()));

        let range = eval_for_range(context, &value.range, rev, step, token)?;

        let mut ret = ir::DeclarationBlock::default();

        for i in range {
            let label = format!("{}[{}]", label, i);
            let label = resource_table::insert_str(&label);

            let index = value.identifier.text();
            let path = VarPath::new(index);
            let kind = VarKind::Const;
            let comptime = Comptime::create_value(Value::new(i as u64, 32, false), token);

            context.push_hierarchy(label);

            let block = context.block(|c| {
                let id = c.insert_var_path(path.clone(), comptime.clone());
                let variable = Variable::new(
                    id,
                    path,
                    kind,
                    comptime.r#type.clone(),
                    vec![comptime.get_value().unwrap().clone()],
                    c.get_affiliation(),
                    &token,
                );
                c.insert_variable(id, variable);

                let block: IrResult<ir::DeclarationBlock> =
                    Conv::conv(c, value.generate_named_block.as_ref());
                c.insert_ir_error(&block);
                block
            });

            context.pop_hierarchy();

            if let Ok(mut block) = block {
                ret.0.append(&mut block.0);
            }
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

            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;

            // parameter default value
            let expr = if context.is_affiliated(Affiliation::ProtoModule) {
                let comptime = Comptime::create_unknown(ClockDomain::None, token);
                let expr = ir::Expression::Term(Box::new(ir::Factor::Unknown(token)));
                (comptime, expr)
            } else {
                let Some(expr) = &x.value else {
                    context.insert_error(AnalyzerError::missing_default_argument(
                        &value.identifier.text().to_string(),
                        &value.identifier.as_ref().into(),
                    ));
                    return Err(ir_error!(token));
                };

                eval_expr(context, Some(r#type.clone()), expr, false)?
            };

            // Get overridden parameter if it exists
            let mut expr = context.get_override(&path).cloned().unwrap_or(expr);

            let dst = ir::AssignDestination {
                id: VarId::default(),
                path: path.clone(),
                index: VarIndex::default(),
                select: VarSelect::default(),
                comptime: Comptime::from_type(r#type, ClockDomain::None, TokenRange::default()),
                token: variable_token,
            };

            eval_const_assign(context, kind, &dst, &mut expr)?;

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
            && let SymbolKind::Port(x) = &symbol.found.kind
        {
            check_modport_in_port(context, value);
            check_port_direction(context, value);

            let path = VarPath::new(symbol.found.token.text);
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let pos = match x.direction {
                Direction::Modport => TypePosition::Modport,
                _ => TypePosition::Variable,
            };

            let r#type = x.r#type.to_ir_type(context, pos)?;
            let clock_domain = x.clock_domain;

            context.insert_port_type(path.clone(), r#type.clone(), clock_domain);

            let kind = match x.direction {
                Direction::Input => VarKind::Input,
                Direction::Output => VarKind::Output,
                Direction::Inout => VarKind::Inout,
                Direction::Modport => {
                    match &r#type.kind {
                        ir::TypeKind::Modport(sig, name) => {
                            let component = get_component(context, sig, token)?;
                            let base = value.identifier.text();
                            let ir::Component::Interface(component) = component else {
                                return Err(ir_error!(token));
                            };
                            context.extract_interface_member(
                                base,
                                &r#type.array,
                                component,
                                Some(*name),
                                clock_domain,
                                variable_token,
                            );

                            // insert path of modport instance
                            let path = VarPath::new(value.identifier.text());
                            let r#type = ir::Type {
                                kind: TypeKind::Modport(sig.clone(), *name),
                                array: r#type.array,
                                ..Default::default()
                            };

                            let comptime =
                                Comptime::from_type(r#type.clone(), clock_domain, variable_token);
                            context.insert_var_path(path.clone(), comptime);
                        }
                        ir::TypeKind::SystemVerilog => (),
                        _ => {
                            context.insert_error(AnalyzerError::mismatch_type(
                                &symbol.found.token.to_string(),
                                "modport",
                                &symbol.found.kind.to_kind_name(),
                                &variable_token,
                            ));
                        }
                    }
                    // inserting modport is completed in this block
                    return Ok(());
                }
                Direction::Interface => {
                    if let ir::TypeKind::AbstractInterface(_) = &r#type.kind {
                        let path = VarPath::new(value.identifier.text());
                        let comptime =
                            Comptime::from_type(r#type.clone(), clock_domain, variable_token);
                        context.insert_var_path(path.clone(), comptime);
                    }
                    return Ok(());
                }
                _ => {
                    return Err(ir_error!(token));
                }
            };

            let default_value = if let Some(x) = &x.default_value {
                let allow_anonymous = kind == VarKind::Output;
                let default_value = eval_expr(context, Some(r#type.clone()), x, allow_anonymous);

                check_port_default_value(context, value, &default_value, kind, x);

                let (comptime, _) = default_value?;

                if x.is_anonymous_expression() {
                    None
                } else {
                    let value = comptime.get_value()?.clone();
                    Some((value, comptime))
                }
            } else {
                None
            };

            if let Some((value, comptime)) = default_value {
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
                    vec![value.clone()],
                    context.get_affiliation(),
                    &variable_token,
                );
                context.insert_variable(id, variable);
            } else {
                eval_variable(context, &path, kind, &r#type, clock_domain, variable_token);
            }
            Ok(())
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&AlwaysFfDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysFfDeclaration) -> IrResult<Self> {
        let clock = eval_clock(context, value)?;
        let reset = eval_reset(context, value)?;

        if let Some(reset) = &reset {
            check_clock_domain(
                context,
                &clock.comptime,
                &reset.comptime,
                &value.always_ff.always_ff_token.token,
            );
        }

        context.current_clock = Some(clock.comptime.clone());

        context.push_affiliation(Affiliation::AlwaysFf);

        let statements: IrResult<ir::StatementBlock> =
            context.block(|c| Conv::conv(c, value.statement_block.as_ref()));

        context.pop_affiliation();

        Ok(ir::Declaration::new_ff(clock, reset, statements?.0))
    }
}

impl Conv<&AlwaysCombDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysCombDeclaration) -> IrResult<Self> {
        context.push_affiliation(Affiliation::AlwaysComb);

        let statements: IrResult<ir::StatementBlock> =
            context.block(|c| Conv::conv(c, value.statement_block.as_ref()));

        context.pop_affiliation();

        Ok(ir::Declaration::new_comb(statements?.0))
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

            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
            let clock_domain = x.clock_domain;

            eval_variable(context, &path, kind, &r#type, clock_domain, variable_token);
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
            let kind = VarKind::Let;
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
            let clock_domain = x.clock_domain;

            eval_variable(context, &path, kind, &r#type, clock_domain, variable_token);

            let (id, comptime) = context.find_path(&path).ok_or_else(|| ir_error!(token))?;

            let dst = ir::AssignDestination {
                id,
                path,
                index: VarIndex::default(),
                select: VarSelect::default(),
                comptime,
                token: variable_token,
            };

            let mut expr = eval_expr(context, Some(r#type.clone()), &value.expression, false)?;

            let statements = eval_assign_statement(context, &dst, &mut expr, token)?;
            Ok(ir::Declaration::new_comb(statements))
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

            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;

            let Some(expr) = &x.value else {
                context.insert_error(AnalyzerError::missing_default_argument(
                    &value.identifier.text().to_string(),
                    &value.identifier.as_ref().into(),
                ));
                return Err(ir_error!(token));
            };

            let (comptime, expr) = eval_expr(context, Some(r#type.clone()), expr, false)?;
            if !comptime.is_const {
                context.insert_error(AnalyzerError::unevaluable_value(
                    UnevaluableValueKind::ConstValue,
                    &expr.token_range(),
                ));
            }

            let dst = ir::AssignDestination {
                id: VarId::default(),
                path: path.clone(),
                index: VarIndex::default(),
                select: VarSelect::default(),
                comptime: Comptime::from_type(r#type, ClockDomain::None, TokenRange::default()),
                token: variable_token,
            };

            eval_const_assign(context, kind, &dst, &mut (comptime, expr))?;

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

                if let Some(dst) = dst.to_assign_destination(context, false) {
                    let mut expr = eval_expr(
                        context,
                        Some(dst.comptime.r#type.clone()),
                        &value.expression,
                        false,
                    )?;

                    let statements = eval_assign_statement(context, &dst, &mut expr, token)?;

                    Ok(ir::Declaration::new_comb(statements))
                } else {
                    if let Ok(symbol) = symbol_table::resolve(x.hierarchical_identifier.as_ref())
                        && let SymbolKind::Variable(x) = symbol.found.kind
                        && x.affiliation == Affiliation::Module
                    {
                        let ident_token = ident.identifier.identifier_token.token;
                        context.insert_error(AnalyzerError::referring_before_definition(
                            &ident_token.text.to_string(),
                            &ident_token.into(),
                        ));
                    }
                    Err(ir_error!(token))
                }
            }
            AssignDestination::LBraceAssignConcatenationListRBrace(x) => {
                let items: Vec<_> = x.assign_concatenation_list.as_ref().into();

                let mut dst = vec![];
                for item in items {
                    let ident = item.hierarchical_identifier.as_ref();
                    let x: VarPathSelect = Conv::conv(context, ident)?;
                    if let Some(x) = x.to_assign_destination(context, false) {
                        dst.push(x);
                    } else {
                        if let Ok(symbol) =
                            symbol_table::resolve(item.hierarchical_identifier.as_ref())
                            && let SymbolKind::Variable(x) = symbol.found.kind
                            && x.affiliation == Affiliation::Module
                        {
                            let ident_token = ident.identifier.identifier_token.token;
                            context.insert_error(AnalyzerError::referring_before_definition(
                                &ident_token.text.to_string(),
                                &ident_token.into(),
                            ));
                        }
                        return Err(ir_error!(token));
                    }
                }

                let mut width = Some(0);
                for x in &dst {
                    if let Some(x) = x.total_width(context)
                        && let Some(width) = &mut width
                    {
                        *width += x;
                    } else {
                        width = None;
                    }
                }
                if let Some(x) = width {
                    width = context.check_size(x, token);
                }

                let r#type = ir::Type {
                    kind: TypeKind::Logic,
                    width: Shape::new(vec![width]),
                    ..Default::default()
                };

                let (_, expr) = eval_expr(context, Some(r#type), &value.expression, false)?;
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

        if let Some(x) = &value.function_declaration_opt {
            check_generic_bound(context, &x.with_generic_parameter);
        }

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Function(x) = symbol.found.kind
        {
            let ret_type = if let Some(x) = &x.ret {
                let mut r#type = x.to_ir_type(context, TypePosition::Variable)?;
                if r#type.is_struct() {
                    r#type.width = Shape::new(vec![r#type.total_width()]);
                    r#type.kind = if r#type.is_2state() {
                        TypeKind::Bit
                    } else {
                        TypeKind::Logic
                    };
                }
                let comptime = Comptime::from_type(r#type, ClockDomain::None, token);
                Some(comptime)
            } else {
                None
            };

            let arg_items: Vec<_> = if let Some(x) = &value.function_declaration_opt0
                && let Some(x) = &x.port_declaration.port_declaration_opt
            {
                x.port_declaration_list.as_ref().into()
            } else {
                vec![]
            };

            // insert VarPath for function before statement_block conv
            // because it may be refered by recursive function
            let path = FuncPath::new(symbol.found.id);
            let arity = arg_items.len();
            let args = vec![]; // args are collected later
            let token: TokenRange = symbol.found.token.into();
            let id =
                context.insert_func_path(name, path.clone(), ret_type.clone(), arity, args, token);

            context.push_affiliation(Affiliation::Function);
            context.push_hierarchy(name);

            let body = context.block(|c| {
                // insert return value as variable
                let ret_id = if let Some(ret_type) = ret_type.clone() {
                    let path = VarPath::new(get_return_str());
                    let kind = VarKind::Variable;
                    let r#type = ret_type.r#type.clone();

                    if let Some(total_array) = r#type.total_array()
                        && let Some(total_width) = r#type.total_width()
                    {
                        // type.expand is not necessary
                        // because member access is not allowed for return value
                        let mut values = vec![];
                        for _ in 0..total_array {
                            values.push(Value::new_x(total_width, false));
                        }

                        let ret_id = c.insert_var_path(path.clone(), ret_type);
                        let variable = Variable::new(
                            ret_id,
                            path,
                            kind,
                            r#type,
                            values,
                            c.get_affiliation(),
                            &token,
                        );
                        c.insert_variable(ret_id, variable);
                        Some(ret_id)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let mut arg_map = HashMap::default();
                let mut args = vec![];

                for item in arg_items {
                    let ret: IrResult<()> = Conv::conv(c, item);
                    c.insert_ir_error(&ret);

                    let name = item.identifier.text();
                    let path = VarPath::new(name);
                    if let Some((_, comptime)) = c.var_paths.get(&path).cloned() {
                        let modport_members = comptime.r#type.expand_modport(c, &path, token)?;
                        if modport_members.is_empty() {
                            let mut members = vec![];
                            if let Some((id, comptime)) = c.var_paths.get(&path).cloned() {
                                let variable =
                                    c.variables.get(&id).ok_or_else(|| ir_error!(token))?;
                                let direction = match variable.kind {
                                    VarKind::Input => Direction::Input,
                                    VarKind::Output => Direction::Output,
                                    _ => unreachable!(),
                                };

                                arg_map.insert(path.clone(), id);
                                members.push((path.clone(), comptime, direction));
                            }
                            let arg = FuncArg {
                                name: path.first(),
                                comptime,
                                members,
                            };
                            args.push(arg);
                        } else {
                            let mut members = vec![];
                            for (path, direction) in modport_members {
                                if let Some((id, comptime)) = c.var_paths.get(&path) {
                                    arg_map.insert(path.clone(), *id);
                                    members.push((path, comptime.clone(), direction));
                                }
                            }
                            let arg = FuncArg {
                                name: path.first(),
                                comptime,
                                members,
                            };
                            args.push(arg);
                        }
                    }
                }

                c.insert_func_args(&path, args);

                let statements: ir::StatementBlock = Conv::conv(c, value.statement_block.as_ref())?;

                Ok(ir::FunctionBody {
                    ret: ret_id,
                    arg_map,
                    statements: statements.0,
                })
            });

            context.pop_affiliation();
            context.pop_hierarchy();

            let r#type = ret_type.as_ref().map(|x| x.r#type.clone());
            let function = ir::Function {
                id,
                path,
                r#type,
                array: Shape::default(),
                functions: vec![body?],
            };

            // function should be inserted outside the function scope
            context.insert_function(id, function);
            Ok(())
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&StructUnionDeclaration> for () {
    fn conv(context: &mut Context, value: &StructUnionDeclaration) -> IrResult<Self> {
        eval_type(
            context,
            &value.identifier.as_ref().into(),
            TypePosition::TypeDef,
        )?;

        if let Some(x) = &value.struct_union_declaration_opt {
            check_generic_bound(context, &x.with_generic_parameter);
        }

        if context.is_affiliated(Affiliation::Interface) {
            let kind = match value.struct_union.as_ref() {
                StructUnion::Struct(_) => "struct",
                StructUnion::Union(_) => "union",
            };
            context.insert_error(AnalyzerError::invalid_type_declaration(kind, &value.into()));
        }

        Ok(())
    }
}

impl Conv<&EnumDeclaration> for () {
    fn conv(context: &mut Context, value: &EnumDeclaration) -> IrResult<Self> {
        eval_type(
            context,
            &value.identifier.as_ref().into(),
            TypePosition::TypeDef,
        )?;

        if context.is_affiliated(Affiliation::Interface) {
            context.insert_error(AnalyzerError::invalid_type_declaration(
                "enum",
                &value.into(),
            ));
        }

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

        let in_module = context.is_affiliated(Affiliation::Module);
        let token = value.inst.inst_token.token;
        check_inst(context, in_module, &token, &value.component_instantiation);

        let clock_domain = if let Ok(symbol) =
            symbol_table::resolve(value.component_instantiation.identifier.as_ref())
            && let SymbolKind::Instance(x) = symbol.found.kind
        {
            x.clock_domain
        } else {
            ClockDomain::None
        };

        let value = value.component_instantiation.as_ref();
        let token: TokenRange = value.identifier.as_ref().into();
        let generic_path: GenericSymbolPath = value.scoped_identifier.as_ref().into();

        check_generic_args(context, &generic_path);

        let mut sig =
            Signature::from_path(context, generic_path).ok_or_else(|| ir_error!(token))?;
        let symbol = symbol_table::get(sig.symbol).unwrap();

        let parameters = symbol.kind.get_parameters();
        let overridden_params = get_overridden_params(context, value)?;
        for x in parameters {
            let path = VarPath::new(x.name);
            if let Some(value) = overridden_params.get(&path) {
                sig.add_parameter(x.name, value.0.value.clone());
            }
        }

        context.push_override(overridden_params);

        let component = context.block(|c| get_component(c, &sig, token));

        context.pop_override();

        match component? {
            ir::Component::Module(component) => {
                let mut inputs = vec![];
                let mut outputs = vec![];
                if let Some(x) = &value.component_instantiation_opt2
                    && let Some(x) = &x.inst_port.inst_port_opt
                {
                    let ports: Vec<_> = x.inst_port_list.as_ref().into();
                    let mut clock_domain_table = HashMap::default();

                    for port in ports {
                        let name = port.identifier.text();
                        let path = VarPath::new(name);
                        let token: TokenRange = port.identifier.as_ref().into();
                        if let Some((dst_type, clock_domain)) = component.port_types.get(&path) {
                            let connects = get_port_connects(
                                context, &component, port, &path, dst_type, token,
                            );
                            let Ok(connects) = connects else {
                                context.insert_ir_error(&connects);
                                continue;
                            };

                            let dst_comptime =
                                Comptime::from_type(dst_type.clone(), *clock_domain, token);

                            for (path, dst, mut expr) in connects {
                                let mut variables = vec![];
                                if let Some(id) = component.ports.get(&path)
                                    && let Some(variable) = component.variables.get(id)
                                {
                                    variables.push(variable);
                                }

                                if !variables.is_empty() {
                                    let expr_comptime = expr.eval_comptime(context, None);

                                    if let Some(x) =
                                        clock_domain_table.get(&dst_comptime.clock_domain)
                                    {
                                        check_clock_domain(context, x, &expr_comptime, &token.beg);
                                    } else {
                                        clock_domain_table
                                            .insert(dst_comptime.clock_domain, expr_comptime);
                                    }

                                    insert_port_connect(
                                        context,
                                        &variables,
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
                Ok(ir::Declaration::Inst(Box::new(ir::InstDeclaration {
                    name,
                    inputs,
                    outputs,
                    component,
                })))
            }
            ir::Component::Interface(component) => {
                let mut array = Shape::default();
                if let Some(x) = &value.component_instantiation_opt0 {
                    let exprs: Vec<_> = x.array.as_ref().into();
                    for expr in exprs {
                        let (_, value) = eval_size(context, expr, false)?;
                        array.push(value)
                    }
                }

                let base = value.identifier.text();
                context.extract_interface_member(
                    base,
                    &array,
                    component,
                    None,
                    clock_domain,
                    token,
                );

                // insert path of interface instance
                let path = VarPath::new(value.identifier.text());
                let r#type = ir::Type {
                    kind: TypeKind::Interface(sig),
                    array,
                    ..Default::default()
                };

                let comptime = Comptime::from_type(r#type.clone(), clock_domain, token);
                context.insert_var_path(path.clone(), comptime);

                Ok(ir::Declaration::Null)
            }
            ir::Component::SystemVerilog(mut component) => {
                if let Some(x) = &value.component_instantiation_opt2
                    && let Some(x) = &x.inst_port.inst_port_opt
                {
                    let ports: Vec<_> = x.inst_port_list.as_ref().into();
                    let mut prev_port = None;
                    for port in ports {
                        let dst_paths = if let Some(x) = &port.inst_port_item_opt {
                            let dst: Vec<VarPathSelect> =
                                Conv::conv(context, x.expression.as_ref())?;
                            dst
                        } else {
                            let name = port.identifier.text();
                            let path = VarPath::new(name);
                            let path = VarPathSelect(
                                path,
                                VarSelect::default(),
                                port.identifier.identifier_token.token.into(),
                            );
                            vec![path]
                        };

                        let mut expanded_paths = vec![];
                        for dst_path in dst_paths {
                            if let Some((_, comptime)) = context.find_path(&dst_path.0) {
                                if comptime.r#type.is_interface() {
                                    let paths = comptime.r#type.expand_interface(
                                        context,
                                        &dst_path.0,
                                        comptime.token,
                                    )?;
                                    for x in paths {
                                        let path =
                                            VarPathSelect(x.0, dst_path.1.clone(), dst_path.2);
                                        expanded_paths.push(path);
                                    }
                                } else {
                                    expanded_paths.push(dst_path);
                                }
                            }
                        }

                        let mut dst_paths =
                            var_path_to_assign_destination(context, expanded_paths, true);

                        for dst_path in &dst_paths {
                            if let Some((_, comptime)) = context.find_path(&dst_path.path) {
                                // All port of SV instance should have the same clock domain
                                if let Some(prev) = &prev_port {
                                    check_clock_domain(context, &comptime, prev, &token.beg);
                                }

                                // Check implicit reset to SV instance
                                if comptime.r#type.is_reset()
                                    && !comptime.r#type.is_explicit_reset()
                                {
                                    context.insert_error(AnalyzerError::sv_with_implicit_reset(
                                        &token,
                                    ));
                                }

                                prev_port = Some(comptime);
                            }
                        }

                        component.connects.append(&mut dst_paths);
                    }
                }
                let name = value.identifier.text();
                let component = ir::Component::SystemVerilog(component);
                Ok(ir::Declaration::Inst(Box::new(ir::InstDeclaration {
                    name,
                    inputs: vec![],
                    outputs: vec![],
                    component,
                })))
            }
        }
    }
}

impl Conv<&ConnectDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &ConnectDeclaration) -> IrResult<Self> {
        let token: TokenRange = value.into();

        let lhs: VarPathSelect = Conv::conv(context, value.hierarchical_identifier.as_ref())?;
        let rhs: Vec<VarPathSelect> = Conv::conv(context, value.expression.as_ref())?;

        let (comptime, _) = eval_expr(context, None, value.expression.as_ref(), false)?;

        let statements = if comptime.is_const {
            expand_connect_const(context, lhs, comptime, token)?
        } else {
            if rhs.len() != 1 {
                // TODO error
                return Err(ir_error!(token));
            }

            let rhs = rhs[0].clone();

            expand_connect(context, lhs, rhs, token)?
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

impl Conv<&ImportDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &ImportDeclaration) -> IrResult<Self> {
        check_import(context, value);
        Ok(ir::DeclarationBlock::default())
    }
}

impl Conv<&BindDeclaration> for () {
    fn conv(context: &mut Context, value: &BindDeclaration) -> IrResult<Self> {
        if let Ok(symbol) = symbol_table::resolve(value.scoped_identifier.as_ref()) {
            if !check_bind_target(context, &value.scoped_identifier, &symbol.found) {
                return Ok(());
            }

            let in_module = symbol.found.is_module(true);
            let token = value.bind.bind_token.token;
            check_inst(context, in_module, &token, &value.component_instantiation);
        }

        Ok(())
    }
}

impl Conv<&EmbedDeclaration> for () {
    fn conv(context: &mut Context, value: &EmbedDeclaration) -> IrResult<Self> {
        for x in &value.embed_content.embed_content_list {
            if let EmbedItem::EmbedScopedIdentifier(x) = x.embed_item.as_ref() {
                let path = x.embed_scoped_identifier.scoped_identifier.as_ref();

                let token: TokenRange = path.identifier().into();
                let generic_path: GenericSymbolPath = path.into();

                // Call get_component for identifier in embed declaration to check generic instances
                if let Ok(symbol) = symbol_table::resolve(path)
                    && matches!(
                        symbol.found.kind,
                        SymbolKind::Module(_) | SymbolKind::Interface(_)
                    )
                {
                    let sig = Signature::from_path(context, generic_path)
                        .ok_or_else(|| ir_error!(token))?;
                    let _component = context.block(|c| get_component(c, &sig, token));
                }
            }
        }

        Ok(())
    }
}

impl Conv<&AliasDeclaration> for () {
    fn conv(context: &mut Context, value: &AliasDeclaration) -> IrResult<Self> {
        let r#type = match value.alias_declaration_group.as_ref() {
            AliasDeclarationGroup::Module(_) => AliasType::Module,
            AliasDeclarationGroup::Interface(_) => AliasType::Interface,
            AliasDeclarationGroup::Package(_) => AliasType::Package,
        };

        check_alias_target(context, &value.scoped_identifier, r#type);

        Ok(())
    }
}

impl Conv<&TypeDefDeclaration> for () {
    fn conv(context: &mut Context, value: &TypeDefDeclaration) -> IrResult<Self> {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::TypeDef(x) = &symbol.found.kind
        {
            x.r#type.to_ir_type(context, TypePosition::TypeDef)?;
        }

        Ok(())
    }
}

impl Conv<&ModportDeclaration> for () {
    fn conv(context: &mut Context, value: &ModportDeclaration) -> IrResult<Self> {
        context.push_affiliation(Affiliation::Modport);

        let ret = context.block(|c| {
            if let Some(x) = &value.modport_declaration_opt {
                let items: Vec<_> = x.modport_list.as_ref().into();
                for item in &items {
                    check_modport(c, item);
                    check_direction(c, &item.direction);
                }
            }

            if let Some(x) = &value.modport_declaration_opt0 {
                check_modport_default(c, x.modport_default.as_ref(), value.identifier.text());
            }

            if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
                && let SymbolKind::Modport(x) = symbol.found.kind
            {
                let sig = if let Some(x) = c.get_current_signature() {
                    x.clone()
                } else {
                    Signature::new(x.interface)
                };

                let name = value.identifier.text();
                let path = VarPath::new(name);
                let r#type = ir::Type {
                    kind: ir::TypeKind::Modport(sig, symbol.found.token.text),
                    ..Default::default()
                };
                let token: TokenRange = value.identifier.as_ref().into();

                let comptime = Comptime::from_type(r#type, ClockDomain::None, token);
                c.insert_var_path(path, comptime);

                let mut members = vec![];
                for x in x.members {
                    let symbol = symbol_table::get(x).unwrap();
                    match &symbol.kind {
                        SymbolKind::ModportVariableMember(x) => {
                            members.push((symbol.token.text, x.direction));
                        }
                        SymbolKind::ModportFunctionMember(_) => {
                            members.push((symbol.token.text, Direction::Import));
                        }
                        _ => (),
                    }
                }
                c.insert_modport(name, members);
            }
            Ok(())
        });

        context.pop_affiliation();
        ret
    }
}
