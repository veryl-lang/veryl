use crate::analyzer_error::AnalyzerError;
use crate::conv::utils::{
    TypePosition, argument_list, case_condition, eval_assign_statement, eval_expr, eval_for_range,
    eval_variable, expand_connect, expand_connect_const, function_call, get_return_str,
    switch_condition,
};
use crate::conv::{Context, Conv};
use crate::ir::{
    self, Comptime, IrResult, Shape, TypeKind, VarIndex, VarKind, VarPath, VarPathSelect,
    VarSelect, Variable,
};
use crate::ir_error;
use crate::namespace::DefineContext;
use crate::symbol::{Affiliation, SymbolKind};
use crate::symbol_table;
use crate::value::Value;
use num_bigint::BigUint;
use veryl_parser::resource_table;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&StatementBlock> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &StatementBlock) -> IrResult<Self> {
        let statements: Vec<_> = value.into();
        let mut ret = vec![];
        for s in statements {
            let x: IrResult<ir::StatementBlock> = Conv::conv(context, s);
            context.insert_ir_error(&x);
            if let Ok(x) = x {
                ret.append(&mut x.0.into_iter().filter(|x| !x.is_null()).collect());
            }
        }
        Ok(ir::StatementBlock(ret))
    }
}

impl Conv<&StatementBlockItem> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &StatementBlockItem) -> IrResult<Self> {
        match value {
            StatementBlockItem::VarDeclaration(x) => {
                let _: ir::Declaration = Conv::conv(context, x.var_declaration.as_ref())?;
                Ok(ir::StatementBlock::default())
            }
            StatementBlockItem::LetStatement(x) => Conv::conv(context, x.let_statement.as_ref()),
            StatementBlockItem::ConstDeclaration(x) => {
                let _: ir::Declaration = Conv::conv(context, x.const_declaration.as_ref())?;
                Ok(ir::StatementBlock::default())
            }
            StatementBlockItem::Statement(x) => Conv::conv(context, x.statement.as_ref()),
            StatementBlockItem::ConcatenationAssignment(x) => {
                Conv::conv(context, x.concatenation_assignment.as_ref())
            }
        }
    }
}

impl Conv<&LetStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &LetStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.r#let.let_token).into();
        if !define_context.is_default() {
            return Ok(ir::StatementBlock::default());
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

        Ok(ir::StatementBlock(vec![statement]))
    }
}

impl Conv<&Statement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &Statement) -> IrResult<Self> {
        let token: TokenRange = value.into();
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
            Statement::BreakStatement(_) => Err(ir_error!(token)),
            Statement::ForStatement(x) => Conv::conv(context, x.for_statement.as_ref()),
            Statement::CaseStatement(x) => Conv::conv(context, x.case_statement.as_ref()),
            Statement::SwitchStatement(x) => Conv::conv(context, x.switch_statement.as_ref()),
        }
    }
}

impl Conv<&IdentifierStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &IdentifierStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.semicolon.semicolon_token).into();
        if !define_context.is_default() {
            return Ok(ir::StatementBlock::default());
        }

        let token: TokenRange = value.into();
        let expr = value.expression_identifier.as_ref();

        match value.identifier_statement_group.as_ref() {
            IdentifierStatementGroup::Assignment(x) => {
                match x.assignment.assignment_group.as_ref() {
                    AssignmentGroup::Equ(_) => {
                        let dst: VarPathSelect = Conv::conv(context, expr)?;

                        if let Some(dst) = dst.to_assign_destination(context, false) {
                            let mut expr = eval_expr(
                                context,
                                Some(dst.comptime.r#type.clone()),
                                &x.assignment.expression,
                                false,
                            )?;

                            let statements =
                                eval_assign_statement(context, &dst, &mut expr, token)?;
                            Ok(ir::StatementBlock(statements))
                        } else {
                            Err(ir_error!(token))
                        }
                    }
                    AssignmentGroup::AssignmentOperator(op) => {
                        let op: ir::Op = Conv::conv(context, op.assignment_operator.as_ref())?;

                        let dst: VarPathSelect = Conv::conv(context, expr)?;
                        let src: VarPathSelect = Conv::conv(context, expr)?;

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
                            let expr = if op == ir::Op::Sub {
                                let expr = ir::Expression::Unary(ir::Op::Sub, Box::new(expr));
                                ir::Expression::Binary(Box::new(src), ir::Op::Add, Box::new(expr))
                            } else {
                                ir::Expression::Binary(Box::new(src), op, Box::new(expr))
                            };

                            let statement = ir::AssignStatement {
                                dst: vec![dst],
                                width,
                                expr,
                                token,
                            };
                            let statement = ir::Statement::Assign(statement);
                            Ok(ir::StatementBlock(vec![statement]))
                        } else {
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
                                // TODO error
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
                let args = if let Some(x) = &x.function_call.function_call_opt {
                    argument_list(context, x.argument_list.as_ref())?
                } else {
                    ir::Arguments::Null
                };

                let symbol = symbol_table::resolve(value.expression_identifier.as_ref())
                    .map_err(|_| ir_error!(token))?;

                match &symbol.found.kind {
                    SymbolKind::SystemFunction(_) => {
                        let name = symbol.found.token.text;
                        let args = args.to_system_function_args(context, &symbol.found);
                        let ret = ir::SystemFunctionCall::new(context, name, args, token)?;
                        Ok(ir::StatementBlock(vec![ir::Statement::SystemFunctionCall(
                            ret,
                        )]))
                    }
                    SymbolKind::Function(x) => {
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
                    SymbolKind::ProtoFunction(_) | SymbolKind::SystemVerilog => {
                        Err(ir_error!(token))
                    }
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
        if value.to_usize() != 0 {
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
        if !define_context.is_default() {
            return Ok(ir::StatementBlock::default());
        }

        let (comptime, cond) = eval_expr(context, None, &value.expression, false)?;

        if !comptime.r#type.is_binary() {
            let token: TokenRange = value.expression.as_ref().into();
            context.insert_error(AnalyzerError::invalid_logical_operand(false, &token));
        }

        let (true_side_only, false_side_only) = check_true_false(&comptime);

        let true_side = if false_side_only {
            vec![]
        } else {
            let true_side: ir::StatementBlock =
                Conv::conv(context, value.statement_block.as_ref())?;
            true_side.0
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
        if let Some(x) = &value.if_statement_opt
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
        if !define_context.is_default() {
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
        if !define_context.is_default() {
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
        if !define_context.is_default() {
            return Ok(ir::StatementBlock::default());
        }

        let token: TokenRange = (&value.identifier.identifier_token).into();

        let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref()) else {
            return Err(ir_error!(token));
        };

        let SymbolKind::Variable(x) = symbol.found.kind else {
            unreachable!();
        };
        let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
        let clock_domain = x.clock_domain;

        let rev = value.for_statement_opt.is_some();

        if rev && !r#type.signed {
            context.insert_error(
                AnalyzerError::unsigned_loop_variable_in_descending_order_for_loop(&token),
            );
        }

        let mut ret = ir::StatementBlock::default();

        let step = value
            .for_statement_opt0
            .as_ref()
            .map(|x| (x.assignment_operator.as_ref(), x.expression.as_ref()));

        let range = eval_for_range(context, &value.range, rev, step, token)?;

        for i in range {
            let label = format!("[{}]", i);
            let label = resource_table::insert_str(&label);

            context.push_hierarchy(label);

            let block = context.block(|c| {
                let index = value.identifier.text();
                let path = VarPath::new(index);
                let kind = VarKind::Const;
                let mut comptime = Comptime::from_type(r#type.clone(), clock_domain, token);
                comptime.is_const = true;
                if let Some(total_width) = r#type.total_width() {
                    comptime.value = ir::ValueVariant::Numeric(Value::new(
                        BigUint::from(i),
                        total_width,
                        r#type.signed,
                    ));
                }

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

                let block: IrResult<ir::StatementBlock> =
                    Conv::conv(c, value.statement_block.as_ref());
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

impl Conv<&CaseStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &CaseStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.case.case_token).into();
        if !define_context.is_default() {
            return Ok(ir::StatementBlock::default());
        }

        let tgt: ir::Expression = Conv::conv(context, value.expression.as_ref())?;
        let mut ret = ir::StatementBlock::default();

        for item in &value.case_statement_list {
            let cond = match item.case_item.case_item_group.as_ref() {
                CaseItemGroup::CaseCondition(x) => {
                    Some(case_condition(context, &tgt, x.case_condition.as_ref())?)
                }
                CaseItemGroup::Defaul(_) => None,
            };
            let true_side: ir::StatementBlock = match item.case_item.case_item_group0.as_ref() {
                CaseItemGroup0::Statement(x) => Conv::conv(context, x.statement.as_ref())?,
                CaseItemGroup0::StatementBlock(x) => {
                    Conv::conv(context, x.statement_block.as_ref())?
                }
            };

            let statements = if let Some(cond) = cond {
                ir::StatementBlock(vec![ir::Statement::If(ir::IfStatement {
                    cond,
                    true_side: true_side.0,
                    false_side: vec![],
                    token: item.case_item.as_ref().into(),
                })])
            } else {
                true_side
            };

            if ret.0.is_empty() {
                ret = statements;
            } else if let ir::Statement::If(x) = &mut ret.0[0] {
                x.insert_leaf_false(statements.0);
            }
        }

        Ok(ret)
    }
}

impl Conv<&SwitchStatement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &SwitchStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.switch.switch_token).into();
        if !define_context.is_default() {
            return Ok(ir::StatementBlock::default());
        }

        let mut ret = ir::StatementBlock::default();

        for item in &value.switch_statement_list {
            let cond = match item.switch_item.switch_item_group.as_ref() {
                SwitchItemGroup::SwitchCondition(x) => {
                    Some(switch_condition(context, x.switch_condition.as_ref())?)
                }
                SwitchItemGroup::Defaul(_) => None,
            };
            let true_side: ir::StatementBlock = match item.switch_item.switch_item_group0.as_ref() {
                SwitchItemGroup0::Statement(x) => Conv::conv(context, x.statement.as_ref())?,
                SwitchItemGroup0::StatementBlock(x) => {
                    Conv::conv(context, x.statement_block.as_ref())?
                }
            };

            let statements = if let Some(cond) = cond {
                ir::StatementBlock(vec![ir::Statement::If(ir::IfStatement {
                    cond,
                    true_side: true_side.0,
                    false_side: vec![],
                    token: item.switch_item.as_ref().into(),
                })])
            } else {
                true_side
            };

            if ret.0.is_empty() {
                ret = statements;
            } else if let ir::Statement::If(x) = &mut ret.0[0] {
                x.insert_leaf_false(statements.0);
            }
        }

        Ok(ret)
    }
}
