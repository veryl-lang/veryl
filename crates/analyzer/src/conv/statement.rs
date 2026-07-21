use crate::analyzer_error::{ComponentInterfaceMismatchKind, MismatchTypeKind};
use crate::conv::utils::{
    TbMethodCallPosition, TypePosition, argument_list, build_for_range, build_for_statement,
    case_patterns, check_assign_clock_domain, eval_array_range_assign, eval_assign_statement,
    eval_expr, eval_variable, expand_connect, expand_connect_const, function_call, get_return_str,
    hoist_component_method_call, single_function_call_factor, switch_condition, tb_method_call,
    try_infer_decl_type, try_infer_var_assign,
};
use crate::conv::{Context, Conv};
use crate::ir::{
    self, Comptime, IrResult, Shape, TypeKind, VarIndex, VarKind, VarPath, VarPathSelect, VarSelect,
};
use crate::namespace::DefineContext;
use crate::symbol::{Affiliation, SymbolKind};
use crate::symbol_table;
use crate::{AnalyzerError, ir_error};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&StatementBlock> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &StatementBlock) -> IrResult<Self> {
        let statements: Vec<_> = value.into();
        let mut ret = vec![];
        for s in statements {
            let x: IrResult<ir::StatementBlock> = Conv::conv(context, s);
            match x {
                Ok(x) => {
                    ret.append(&mut x.0.into_iter().filter(|x| !x.is_null()).collect());
                }
                Err(e) => {
                    if !context.in_generic {
                        ret.push(ir::Statement::Unsupported(e.token));
                    }
                }
            }
        }
        Ok(ir::StatementBlock(ret))
    }
}

/// Runs `f` with a fresh testbench hoist sink and splices the component
/// method calls hoisted out of the converted construct in front of its
/// statements. Giving each statement-level construct its own sink keeps
/// hoists inside loop/branch bodies attached to the statement they came
/// from (re-executed per iteration), not the enclosing block. No-op
/// outside initial/final blocks (no sink installed).
fn with_tb_hoist_sink(
    context: &mut Context,
    f: impl FnOnce(&mut Context) -> IrResult<ir::StatementBlock>,
) -> IrResult<ir::StatementBlock> {
    let saved = match context.tb_hoist.take() {
        Some(outer) => {
            context.tb_hoist = Some(Vec::new());
            Some(outer)
        }
        None => None,
    };

    let result = f(context);

    let Some(outer) = saved else {
        return result;
    };
    let hoisted = std::mem::replace(context.tb_hoist.as_mut().unwrap(), outer);
    let mut block = result?;
    if !hoisted.is_empty() {
        block.0.splice(0..0, hoisted);
    }
    Ok(block)
}

impl Conv<&StatementBlockItem> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &StatementBlockItem) -> IrResult<Self> {
        // The sink covers the whole item, so `let`/`var` initializers and
        // concatenation assignments hoist like plain statements.
        with_tb_hoist_sink(context, |context| match value {
            StatementBlockItem::VarDeclaration(x) => {
                let _: ir::Declaration = Conv::conv(context, x.var_declaration.as_ref())?;
                Ok(ir::StatementBlock::default())
            }
            StatementBlockItem::LetStatement(x) => Conv::conv(context, x.let_statement.as_ref()),
            StatementBlockItem::ConstDeclaration(x) => {
                let _: ir::Declaration = Conv::conv(context, x.const_declaration.as_ref())?;
                Ok(ir::StatementBlock::default())
            }
            StatementBlockItem::GenDeclaration(x) => {
                let _: ir::Declaration = Conv::conv(context, x.gen_declaration.as_ref())?;
                Ok(ir::StatementBlock::default())
            }
            StatementBlockItem::Statement(x) => Conv::conv(context, x.statement.as_ref()),
            StatementBlockItem::ConcatenationAssignment(x) => {
                Conv::conv(context, x.concatenation_assignment.as_ref())
            }
        })
    }
}

impl Conv<&LetStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &LetStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.r#let.let_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::StatementBlock::default());
        }

        let token: TokenRange = value.into();

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Variable(x) = &symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind = VarKind::Let;
            let variable_token: TokenRange = (&value.identifier.identifier_token).into();

            let inferred = try_infer_decl_type(
                context,
                &x.r#type,
                &value.expression,
                value.identifier.identifier_token.token.id,
                token,
            )?;
            let r#type = if let Some((ref comptime, _)) = inferred {
                comptime.r#type.clone()
            } else {
                x.r#type.to_ir_type(context, TypePosition::Variable)?
            };
            let clock_domain = x.clock_domain;

            eval_variable(context, &path, kind, &r#type, clock_domain, variable_token);

            let (id, comptime) = context.find_path(&path).ok_or_else(|| ir_error!(token))?;

            let mut dst = ir::AssignDestination {
                id,
                path,
                index: VarIndex::default(),
                select: VarSelect::default(),
                comptime,
                token: variable_token,
            };

            let mut expr = if let Some(inferred) = inferred {
                inferred
            } else {
                eval_expr(context, Some(r#type.clone()), &value.expression, false)?
            };

            let statements = eval_assign_statement(context, &mut dst, &mut expr, token)?;
            Ok(ir::StatementBlock(statements))
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&ConcatenationAssignment> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &ConcatenationAssignment) -> IrResult<Self> {
        let token: TokenRange = value.into();

        let items: Vec<_> = value.assign_concatenation_list.as_ref().into();

        let mut dst = vec![];
        for item in items {
            let ident = item.hierarchical_identifier.as_ref();
            let x: VarPathSelect = Conv::conv(context, ident)?;
            if let Some(x) = x.to_assign_destination(context, false) {
                dst.push(x);
            } else {
                if let Ok(symbol) = symbol_table::resolve(item.hierarchical_identifier.as_ref())
                    && let SymbolKind::Variable(x) = &symbol.found.kind
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

        let r#type = {
            let mut t = ir::Type::new(TypeKind::Logic);
            t.set_concrete_width(Shape::new(vec![width]));
            t
        };

        let (comptime, expr) = eval_expr(context, Some(r#type), &value.expression, false)?;
        for d in &mut dst {
            check_assign_clock_domain(context, d, &comptime, &token);
        }
        let statement = ir::Statement::Assign(ir::AssignStatement {
            dst,
            width,
            expr,
            token,
        });

        Ok(ir::StatementBlock(vec![statement]))
    }
}

impl Conv<&Statement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &Statement) -> IrResult<Self> {
        match value {
            Statement::IdentifierStatement(x) => {
                Conv::conv(context, x.identifier_statement.as_ref())
            }
            Statement::IfStatement(x) => Conv::conv(context, x.if_statement.as_ref()),
            Statement::IfResetStatement(x) => Conv::conv(context, x.if_reset_statement.as_ref()),
            Statement::ReturnStatement(x) => Ok(ir::StatementBlock(vec![Conv::conv(
                context,
                x.return_statement.as_ref(),
            )?])),
            Statement::BreakStatement(_) => Ok(ir::StatementBlock(vec![ir::Statement::Break])),
            Statement::ForStatement(x) => Conv::conv(context, x.for_statement.as_ref()),
            Statement::CaseStatement(x) => Conv::conv(context, x.case_statement.as_ref()),
            Statement::SwitchStatement(x) => Conv::conv(context, x.switch_statement.as_ref()),
        }
    }
}

/// Handles `dst = inst.method(...)`, where the RHS is a component method
/// call returning a value across the host boundary. `Ok(Some(_))` means the
/// assignment was such a call and is fully converted; `Ok(None)` means it is
/// an ordinary assignment the caller should convert normally.
fn conv_tb_method_call_assignment(
    context: &mut Context,
    dst_expr: &ExpressionIdentifier,
    value_expr: &Expression,
    token: TokenRange,
) -> IrResult<Option<ir::StatementBlock>> {
    let Some((receiver, call)) = single_function_call_factor(value_expr) else {
        return Ok(None);
    };
    let Some(tb_stmt) =
        tb_method_call(context, receiver, call, token, TbMethodCallPosition::Value)?
    else {
        return Ok(None);
    };
    let ir::Statement::TbMethodCall(mut method_call) = tb_stmt else {
        unreachable!()
    };
    if !matches!(method_call.method, ir::TbMethod::Component { .. }) {
        // Builtin $tb methods return nothing; report the assignment as an
        // unsupported use.
        context.insert_error(AnalyzerError::invalid_factor(
            None,
            "testbench method without return value",
            &token,
            &[],
        ));
        return Err(ir_error!(token));
    }

    let dst: VarPathSelect = Conv::conv(context, dst_expr)?;
    let Some(dst) = dst.to_assign_destination(context, false) else {
        return Err(ir_error!(token));
    };
    if let Some(w) = method_call.ret_width
        && let Some(dst_width) = dst.total_width(context)
        && dst_width != w as usize
    {
        context.insert_error(AnalyzerError::component_interface_mismatch(
            ComponentInterfaceMismatchKind::MethodReturnWidth {
                returned: w as usize,
                destination: dst_width,
            },
            None,
            &token,
        ));
    }
    // A plain destination receives the return value directly, carrying its
    // full declared width. Indexed/selected destinations go through the
    // expression-position hoist (a temporary plus an ordinary assignment,
    // which handles selects).
    if dst.index.0.is_empty() && dst.select.0.is_empty() && dst.select.1.is_none() {
        method_call.ret = Some(Box::new(dst));
        return Ok(Some(ir::StatementBlock(vec![ir::Statement::TbMethodCall(
            method_call,
        )])));
    }
    let temp = hoist_component_method_call(context, method_call, token)?;
    let width = dst
        .total_width(context)
        .and_then(|x| context.check_size(x, token));
    Ok(Some(ir::StatementBlock(vec![ir::Statement::Assign(
        ir::AssignStatement {
            dst: vec![dst],
            width,
            expr: temp,
            token,
        },
    )])))
}

impl Conv<&IdentifierStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &IdentifierStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.semicolon.semicolon_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::StatementBlock::default());
        }

        let token: TokenRange = value.into();
        let expr = value.expression_identifier.as_ref();

        match value.identifier_statement_group.as_ref() {
            IdentifierStatementGroup::Assignment(x) => {
                match x.assignment.assignment_group.as_ref() {
                    AssignmentGroup::Equ(_) => {
                        // `x = inst.method(...)`: a component method call in
                        // assignment position returns a value through the
                        // host boundary; intercept before expression eval.
                        if let Some(block) = conv_tb_method_call_assignment(
                            context,
                            expr,
                            &x.assignment.expression,
                            token,
                        )? {
                            return Ok(block);
                        }

                        let inferred = if let Ok(symbol) = symbol_table::resolve(expr) {
                            try_infer_var_assign(
                                context,
                                &symbol.found,
                                &x.assignment.expression,
                                token,
                            )?
                        } else {
                            None
                        };

                        let dst: VarPathSelect = Conv::conv(context, expr)?;

                        if let Some(statements) =
                            eval_array_range_assign(context, &dst, &x.assignment.expression, token)?
                        {
                            return Ok(ir::StatementBlock(statements));
                        }

                        if let Some(mut dst) = dst.to_assign_destination(context, false) {
                            let mut expr = if let Some(inferred) = inferred {
                                inferred
                            } else {
                                eval_expr(
                                    context,
                                    Some(dst.comptime.r#type.clone()),
                                    &x.assignment.expression,
                                    false,
                                )?
                            };

                            let statements =
                                eval_assign_statement(context, &mut dst, &mut expr, token)?;
                            Ok(ir::StatementBlock(statements))
                        } else {
                            // check expression even if dst can't be determined
                            let _ = eval_expr(context, None, &x.assignment.expression, false)?;

                            Err(ir_error!(token))
                        }
                    }
                    AssignmentGroup::AssignmentOperator(op) => {
                        let op: ir::Op = Conv::conv(context, op.assignment_operator.as_ref())?;

                        let dst: VarPathSelect = Conv::conv(context, expr)?;
                        let src: VarPathSelect = Conv::conv(context, expr)?;

                        // An op-assign to an array range slice has no valid lowering and
                        // emits illegal SystemVerilog; reject it, don't silently drop it.
                        if dst.is_array_range(context) {
                            let _ = eval_expr(context, None, &x.assignment.expression, false);
                            context.insert_error(AnalyzerError::invalid_range_assign(&token));
                            return Ok(ir::StatementBlock::default());
                        }

                        if let Some(dst) = dst.to_assign_destination(context, false)
                            && let Some(src) = src.to_expression(context)
                        {
                            let (_, expr) = eval_expr(
                                context,
                                Some(dst.comptime.r#type.clone()),
                                &x.assignment.expression,
                                false,
                            )?;

                            let width = dst.total_width(context);
                            let comptime = Box::new(Comptime::create_unknown(token));
                            let mut expr =
                                ir::Expression::Binary(Box::new(src), op, Box::new(expr), comptime);
                            let _ = expr.eval_comptime(context, width);

                            let statement = ir::AssignStatement {
                                dst: vec![dst],
                                width,
                                expr,
                                token,
                            };
                            let statement = ir::Statement::Assign(statement);
                            Ok(ir::StatementBlock(vec![statement]))
                        } else {
                            // check expression even if dst can't be determined
                            let _ = eval_expr(context, None, &x.assignment.expression, false)?;

                            Err(ir_error!(token))
                        }
                    }
                    AssignmentGroup::DiamondOperator(_) => {
                        let lhs: VarPathSelect = Conv::conv(context, expr)?;
                        let rhs: Vec<VarPathSelect> =
                            Conv::conv(context, x.assignment.expression.as_ref())?;

                        let (comptime, _) =
                            eval_expr(context, None, x.assignment.expression.as_ref(), false)?;

                        let statements = if comptime.is_const {
                            expand_connect_const(context, lhs, comptime, token)?
                        } else {
                            if rhs.len() != 1 {
                                context.insert_error(AnalyzerError::mismatch_type(
                                    MismatchTypeKind::ConnectMultipleExpression,
                                    &token,
                                ));
                                return Err(ir_error!(token));
                            }

                            let rhs = rhs[0].clone();

                            expand_connect(context, lhs, rhs, token)?
                        };

                        Ok(ir::StatementBlock(statements))
                    }
                }
            }
            IdentifierStatementGroup::FunctionCall(x) => {
                // Intercept $tb component method calls before argument evaluation
                if let Some(tb_stmt) = tb_method_call(
                    context,
                    value.expression_identifier.as_ref(),
                    &x.function_call,
                    token,
                    TbMethodCallPosition::Statement,
                )? {
                    return Ok(ir::StatementBlock(vec![tb_stmt]));
                }

                let args = if let Some(x) = &x.function_call.function_call_opt {
                    argument_list(context, x.argument_list.as_ref())?
                } else {
                    ir::Arguments::Null
                };

                let resolved_path =
                    context.resolve_path(value.expression_identifier.as_ref().into());
                let symbol = symbol_table::resolve(&resolved_path).map_err(|_| ir_error!(token))?;

                match &symbol.found.kind {
                    SymbolKind::SystemFunction(_) => {
                        let name = symbol.found.token.text;
                        let args = args.to_system_function_args(context, &symbol.found);
                        let ret = ir::SystemFunctionCall::new(context, name, args, token)?;
                        Ok(ir::StatementBlock(vec![ir::Statement::SystemFunctionCall(
                            Box::new(ret),
                        )]))
                    }
                    SymbolKind::Function(x) if !x.is_proto => {
                        let ret = function_call(
                            context,
                            value.expression_identifier.as_ref(),
                            args,
                            token,
                        )?;

                        if x.ret.is_some() {
                            context.insert_error(AnalyzerError::unused_return(
                                &symbol.found.token.text.to_string(),
                                &token,
                            ));
                        }

                        Ok(ir::StatementBlock(vec![ir::Statement::FunctionCall(
                            Box::new(ret),
                        )]))
                    }
                    SymbolKind::ModportFunctionMember(x) => {
                        let symbol = symbol_table::get(x.function).unwrap();
                        if let SymbolKind::Function(x) = &symbol.kind {
                            let ret = function_call(
                                context,
                                value.expression_identifier.as_ref(),
                                args,
                                token,
                            )?;

                            if x.ret.is_some() {
                                context.insert_error(AnalyzerError::unused_return(
                                    &symbol.token.text.to_string(),
                                    &token,
                                ));
                            }

                            Ok(ir::StatementBlock(vec![ir::Statement::FunctionCall(
                                Box::new(ret),
                            )]))
                        } else {
                            unreachable!();
                        }
                    }
                    SymbolKind::Function(x) if x.is_proto => Err(ir_error!(token)),
                    SymbolKind::SystemVerilog => Err(ir_error!(token)),
                    _ => {
                        let name = symbol.found.token.text.to_string();
                        let kind = symbol.found.kind.to_kind_name();
                        context
                            .insert_error(AnalyzerError::call_non_function(&name, &kind, &token));
                        Err(ir_error!(token))
                    }
                }
            }
        }
    }
}

fn check_true_false(comptime: &Comptime) -> (bool, bool) {
    if comptime.is_const
        && let Ok(value) = comptime.get_value()
    {
        if value.to_usize().unwrap_or(0) != 0 {
            (true, false)
        } else {
            (false, true)
        }
    } else {
        (false, false)
    }
}

/// Short-circuiting `check_true_false`: also folds a branch made dead by a
/// constant-decisive `||`/`&&` operand when the whole condition is not constant
/// (e.g. the `i >= 4` arms of an unrolled `if i >= 4 || sig`), so a dead arm's
/// out-of-range select never reaches the range check or the simulator.
/// Returns `(true_side_only, false_side_only)`.
fn eval_cond_true_false(context: &mut Context, cond: &ir::Expression) -> (bool, bool) {
    if let ir::Expression::Binary(x, op, y, _) = cond {
        match op {
            ir::Op::LogicOr => {
                let (xt, xf) = eval_cond_true_false(context, x);
                let (yt, yf) = eval_cond_true_false(context, y);
                return (xt || yt, xf && yf);
            }
            ir::Op::LogicAnd => {
                let (xt, xf) = eval_cond_true_false(context, x);
                let (yt, yf) = eval_cond_true_false(context, y);
                return (xt && yt, xf || yf);
            }
            _ => {}
        }
    }
    // Leaf: mirror `check_true_false` (no `is_xz` guard) so `if`, `if_reset`,
    // and fully-constant conditions fold x/z the same way.
    if cond.comptime().is_const
        && let Some(value) = cond.eval_value(context)
    {
        if value.to_usize().unwrap_or(0) != 0 {
            (true, false)
        } else {
            (false, true)
        }
    } else {
        (false, false)
    }
}

impl Conv<&IfStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &IfStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.r#if.if_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::StatementBlock::default());
        }

        let (comptime, cond) = eval_expr(context, None, &value.expression, false)?;

        if !comptime.r#type.is_binary() {
            let token: TokenRange = value.expression.as_ref().into();
            context.insert_error(AnalyzerError::invalid_logical_operand(false, &token));
        }

        let (true_side_only, false_side_only) = eval_cond_true_false(context, &cond);

        let true_side = if false_side_only {
            vec![]
        } else {
            let true_side: IrResult<ir::StatementBlock> = context
                .with_condition_domain(comptime.clone(), |c| {
                    Conv::conv(c, value.statement_block.as_ref())
                });
            true_side?.0
        };

        if true_side_only {
            return Ok(ir::StatementBlock(true_side));
        }

        let mut false_side = vec![];
        let mut else_if_break = false;

        for x in &value.if_statement_list {
            let (comptime, cond) = eval_expr(context, None, &x.expression, false)?;

            if !comptime.r#type.is_binary() {
                let token: TokenRange = x.expression.as_ref().into();
                context.insert_error(AnalyzerError::invalid_logical_operand(false, &token));
            }

            let (true_side_only, false_side_only) = eval_cond_true_false(context, &cond);

            // If this `else if` is false_side_only, this iteration should be skipped.
            if false_side_only {
                continue;
            }

            let true_side: IrResult<ir::StatementBlock> = context
                .with_condition_domain(comptime.clone(), |c| {
                    Conv::conv(c, x.statement_block.as_ref())
                });
            let true_side = true_side?.0;

            let statement = ir::Statement::If(ir::IfStatement {
                cond,
                true_side,
                false_side: vec![],
                token: x.into(),
            });

            if let Some(x) = false_side.last_mut() {
                if let ir::Statement::If(x) = x {
                    x.insert_leaf_false(vec![statement]);
                }
            } else {
                false_side.push(statement);
            }

            // If this `else if` is true_side_only, the remaining else should be skipped.
            if true_side_only {
                else_if_break = true;
                break;
            }
        }
        if let Some(x) = &value.if_statement_opt
            && !else_if_break
        {
            let block: IrResult<ir::StatementBlock> = context
                .with_condition_domain(comptime.clone(), |c| {
                    Conv::conv(c, x.statement_block.as_ref())
                });
            let mut block = block?.0;

            if let Some(x) = false_side.last_mut() {
                if let ir::Statement::If(x) = x {
                    x.insert_leaf_false(block);
                }
            } else {
                false_side.append(&mut block);
            }
        }

        if false_side_only {
            Ok(ir::StatementBlock(false_side))
        } else {
            let statement = ir::Statement::If(ir::IfStatement {
                cond,
                true_side,
                false_side,
                token: value.into(),
            });
            Ok(ir::StatementBlock(vec![statement]))
        }
    }
}

impl Conv<&IfResetStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &IfResetStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.if_reset.if_reset_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::StatementBlock::default());
        }

        context.in_if_reset = true;

        let true_side: ir::IrResult<ir::StatementBlock> =
            context.block(|c| Conv::conv(c, value.statement_block.as_ref()));

        context.in_if_reset = false;

        let true_side = true_side?.0;

        let mut false_side = vec![];
        let mut else_if_break = false;

        for x in &value.if_reset_statement_list {
            let (comptime, cond) = eval_expr(context, None, &x.expression, false)?;

            if !comptime.r#type.is_binary() {
                let token: TokenRange = x.expression.as_ref().into();
                context.insert_error(AnalyzerError::invalid_logical_operand(false, &token));
            }

            let (true_side_only, false_side_only) = check_true_false(&comptime);

            // If this `else if` is false_side_only, this iteration should be skipped.
            if false_side_only {
                continue;
            }

            let true_side: ir::StatementBlock = Conv::conv(context, x.statement_block.as_ref())?;
            let true_side = true_side.0;

            let statement = ir::Statement::If(ir::IfStatement {
                cond,
                true_side,
                false_side: vec![],
                token: x.into(),
            });

            if let Some(x) = false_side.last_mut() {
                if let ir::Statement::If(x) = x {
                    x.insert_leaf_false(vec![statement]);
                }
            } else {
                false_side.push(statement);
            }

            // If this `else if` is true_side_only, the remaining else should be skipped.
            if true_side_only {
                else_if_break = true;
                break;
            }
        }
        if let Some(x) = &value.if_reset_statement_opt
            && !else_if_break
        {
            let block: ir::StatementBlock = Conv::conv(context, x.statement_block.as_ref())?;
            let mut block = block.0;

            if let Some(x) = false_side.last_mut() {
                if let ir::Statement::If(x) = x {
                    x.insert_leaf_false(block);
                }
            } else {
                false_side.append(&mut block);
            }
        }

        let statement = ir::Statement::IfReset(ir::IfResetStatement {
            true_side,
            false_side,
            token: value.into(),
        });
        Ok(ir::StatementBlock(vec![statement]))
    }
}

impl Conv<&ReturnStatement> for ir::Statement {
    fn conv(context: &mut Context, value: &ReturnStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.semicolon.semicolon_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::Statement::Null);
        }

        let token: TokenRange = value.into();

        let dst = VarPath::new(get_return_str());
        let dst = VarPathSelect(dst, ir::VarSelect::default(), token);

        if let Some(dst) = dst.to_assign_destination(context, false) {
            let width = dst.total_width(context);
            let (_, expr) = eval_expr(
                context,
                Some(dst.comptime.r#type.clone()),
                &value.expression,
                false,
            )?;
            Ok(ir::Statement::Assign(ir::AssignStatement {
                dst: vec![dst],
                width,
                expr,
                token,
            }))
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&ForStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &ForStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.r#for.for_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::StatementBlock::default());
        }

        let token: TokenRange = (&value.identifier.identifier_token).into();

        let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref()) else {
            return Err(ir_error!(token));
        };

        let SymbolKind::Variable(ref x) = symbol.found.kind else {
            unreachable!();
        };
        let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
        let clock_domain = x.clock_domain;

        let rev = value.for_statement_opt.is_some();

        let step = value
            .for_statement_opt0
            .as_ref()
            .map(|x| (x.assignment_operator.as_ref(), x.expression.as_ref()));

        let for_range = build_for_range(context, &value.range, rev, step)?;

        // Testbench for-loops may iterate many times at runtime; do not unroll.
        // A body with `break` also stays a runtime For: flattening would strand a
        // runtime-conditional Break with no loop to leave, running every iteration.
        if !context.in_test_module
            && !statement_block_has_break(value.statement_block.as_ref())
            && let Some(range) = for_range.eval_iter(context)
        {
            return unroll_for(context, value, &r#type, clock_domain, &range, token);
        }

        build_for_statement(context, value, &r#type, clock_domain, for_range, token)
    }
}

/// True when the block contains a `break` that would target THIS loop:
/// nested for-loops consume their own breaks, so the scan stops there.
fn statement_block_has_break(block: &StatementBlock) -> bool {
    block
        .statement_block_list
        .iter()
        .any(|x| statement_block_group_has_break(&x.statement_block_group))
}

fn statement_block_group_has_break(group: &StatementBlockGroup) -> bool {
    match &*group.statement_block_group_group {
        StatementBlockGroupGroup::BlockLBraceStatementBlockGroupGroupListRBrace(x) => x
            .statement_block_group_group_list
            .iter()
            .any(|x| statement_block_group_has_break(&x.statement_block_group)),
        StatementBlockGroupGroup::StatementBlockItem(x) => match x.statement_block_item.as_ref() {
            StatementBlockItem::Statement(x) => statement_has_break(&x.statement),
            _ => false,
        },
    }
}

fn statement_has_break(stmt: &Statement) -> bool {
    match stmt {
        Statement::BreakStatement(_) => true,
        Statement::IfStatement(x) => {
            let x = &x.if_statement;
            statement_block_has_break(&x.statement_block)
                || x.if_statement_list
                    .iter()
                    .any(|x| statement_block_has_break(&x.statement_block))
                || x.if_statement_opt
                    .as_ref()
                    .is_some_and(|x| statement_block_has_break(&x.statement_block))
        }
        Statement::IfResetStatement(x) => {
            let x = &x.if_reset_statement;
            statement_block_has_break(&x.statement_block)
                || x.if_reset_statement_list
                    .iter()
                    .any(|x| statement_block_has_break(&x.statement_block))
                || x.if_reset_statement_opt
                    .as_ref()
                    .is_some_and(|x| statement_block_has_break(&x.statement_block))
        }
        Statement::CaseStatement(x) => x.case_statement.case_statement_list.iter().any(|x| match x
            .case_item
            .case_item_group0
            .as_ref()
        {
            CaseItemGroup0::Statement(x) => statement_has_break(&x.statement),
            CaseItemGroup0::StatementBlock(x) => statement_block_has_break(&x.statement_block),
        }),
        Statement::SwitchStatement(x) => x.switch_statement.switch_statement_list.iter().any(|x| {
            match x.switch_item.switch_item_group0.as_ref() {
                SwitchItemGroup0::Statement(x) => statement_has_break(&x.statement),
                SwitchItemGroup0::StatementBlock(x) => {
                    statement_block_has_break(&x.statement_block)
                }
            }
        }),
        // A break inside a nested for belongs to that loop.
        Statement::ForStatement(_) => false,
        Statement::IdentifierStatement(_) | Statement::ReturnStatement(_) => false,
    }
}

fn unroll_for(
    context: &mut Context,
    value: &ForStatement,
    r#type: &ir::Type,
    clock_domain: crate::symbol::ClockDomain,
    range: &[usize],
    token: TokenRange,
) -> ir::IrResult<ir::StatementBlock> {
    use veryl_parser::resource_table;

    let mut ret = ir::StatementBlock::default();
    'outer: for &i in range {
        let label = format!("[{}]", i);
        let label = resource_table::insert_str(&label);

        context.push_hierarchy(label);

        let block = context.block(|c| {
            let index = value.identifier.text();
            let path = ir::VarPath::new(index);
            let kind = ir::VarKind::Const;
            let mut comptime = ir::Comptime::from_type(r#type.clone(), clock_domain, token);
            comptime.is_const = true;
            if let Some(total_width) = r#type.total_width() {
                comptime.value = ir::ValueVariant::Numeric(crate::value::Value::new(
                    i as u64,
                    total_width,
                    r#type.signed,
                ));
            }

            let id = c.insert_var_path(path.clone(), comptime.clone());
            let array_limit = c.config.evaluate_array_limit;
            let variable = ir::Variable::new(
                id,
                path,
                kind,
                comptime.r#type.clone(),
                vec![comptime.get_value().unwrap().clone()],
                c.get_affiliation(),
                &token,
                array_limit,
            );
            c.insert_variable(id, variable);

            let block: ir::IrResult<ir::StatementBlock> =
                Conv::conv(c, value.statement_block.as_ref());
            block
        });

        context.pop_hierarchy();

        if let Ok(mut block) = block {
            for stmt in block.0.drain(..) {
                if matches!(stmt, ir::Statement::Break) {
                    break 'outer;
                }
                ret.0.push(stmt);
            }
        }
    }
    Ok(ret)
}

impl Conv<&CaseStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &CaseStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.case.case_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::StatementBlock::default());
        }

        let mut tgt: ir::Expression = Conv::conv(context, value.expression.as_ref())?;
        let tgt_comptime = tgt.eval_comptime(context, None).clone();

        let mut arms: Vec<ir::CaseArm> = Vec::new();
        let mut default: Vec<ir::Statement> = Vec::new();

        for item in &value.case_statement_list {
            let body: IrResult<ir::StatementBlock> =
                context.with_condition_domain(tgt_comptime.clone(), |c| {
                    match item.case_item.case_item_group0.as_ref() {
                        // A bare statement arm has no StatementBlockItem wrapper, so
                        // it needs its own hoist sink here.
                        CaseItemGroup0::Statement(x) => {
                            with_tb_hoist_sink(c, |c| Conv::conv(c, x.statement.as_ref()))
                        }
                        CaseItemGroup0::StatementBlock(x) => {
                            Conv::conv(c, x.statement_block.as_ref())
                        }
                    }
                });
            let body = body?;
            match item.case_item.case_item_group.as_ref() {
                CaseItemGroup::CaseCondition(x) => {
                    let patterns = case_patterns(context, x.case_condition.as_ref())?;
                    arms.push(ir::CaseArm {
                        patterns,
                        body: body.0,
                        token: item.case_item.as_ref().into(),
                    });
                }
                CaseItemGroup::Defaul(_) => {
                    // Parser enforces at most one default; guard against
                    // a malformed input by keeping the first occurrence.
                    if default.is_empty() {
                        default = body.0;
                    }
                }
            }
        }

        if arms.is_empty() && default.is_empty() {
            return Ok(ir::StatementBlock::default());
        }

        Ok(ir::StatementBlock(vec![ir::Statement::Case(
            ir::CaseStatement {
                arms,
                default,
                case_target: Box::new(tgt),
                token: value.case.case_token.token.into(),
            },
        )]))
    }
}

// `switch` arms carry arbitrary boolean conditions with no shared
// selector, so they stay as a nested if-else chain — only `case` is
// lifted to `Statement::Case`.
impl Conv<&SwitchStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &SwitchStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.switch.switch_token).into();
        if !define_context.is_active(&context.config.defines) {
            return Ok(ir::StatementBlock::default());
        }

        // The emitted SV is `case (1'b1)`, whose `default` is a fallback
        // regardless of where it is listed, so `default` must be the final
        // `else` here too. Lowering positionally instead drops every arm after a
        // non-last `default` (and collapses the switch to the default body when
        // it is listed first).
        let mut arms: Vec<(ir::Expression, Vec<ir::Statement>, TokenRange)> = Vec::new();
        let mut default: Vec<ir::Statement> = Vec::new();

        for item in &value.switch_statement_list {
            let cond = match item.switch_item.switch_item_group.as_ref() {
                SwitchItemGroup::SwitchCondition(x) => {
                    let mut cond = switch_condition(context, x.switch_condition.as_ref())?;
                    cond.eval_comptime(context, None);
                    Some(cond)
                }
                SwitchItemGroup::Defaul(_) => None,
            };

            let convert = |c: &mut Context| -> IrResult<ir::StatementBlock> {
                match item.switch_item.switch_item_group0.as_ref() {
                    // A bare statement arm has no StatementBlockItem wrapper, so
                    // it needs its own hoist sink here.
                    SwitchItemGroup0::Statement(x) => {
                        with_tb_hoist_sink(c, |c| Conv::conv(c, x.statement.as_ref()))
                    }
                    SwitchItemGroup0::StatementBlock(x) => {
                        Conv::conv(c, x.statement_block.as_ref())
                    }
                }
            };
            let true_side = if let Some(cond) = &cond {
                context.with_condition_domain(cond.comptime().clone(), convert)?
            } else {
                convert(context)?
            };

            match cond {
                Some(cond) => {
                    arms.push((cond, true_side.0, item.switch_item.as_ref().into()));
                }
                None => {
                    // Parser enforces at most one default.
                    if default.is_empty() {
                        default = true_side.0;
                    }
                }
            }
        }

        let mut tail: Vec<ir::Statement> = default;
        for (cond, body, token) in arms.into_iter().rev() {
            tail = vec![ir::Statement::If(ir::IfStatement {
                cond,
                true_side: body,
                false_side: std::mem::take(&mut tail),
                token,
            })];
        }

        Ok(ir::StatementBlock(tail))
    }
}
