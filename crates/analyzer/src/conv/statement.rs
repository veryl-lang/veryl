use crate::analyzer_error::AnalyzerError;
use crate::conv::utils::{
    argument_list, case_condition, eval_array_literal, eval_expr, eval_range, expand_connect,
    expand_connect_const, function_call, get_return_str, switch_condition,
};
use crate::conv::{Context, Conv};
use crate::ir::{
    self, Comptime, IrResult, VarIndex, VarKind, VarPath, VarPathSelect, VarSelect, Variable,
};
use crate::ir_error;
use crate::namespace::DefineContext;
use crate::symbol::SymbolKind;
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

                let comptime = Comptime::from_type(r#type.clone(), token);
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
                Ok(ir::StatementBlock(statements))
            } else {
                let statement = ir::Statement::Assign(ir::AssignStatement {
                    dst,
                    width,
                    expr,
                    token,
                });
                Ok(ir::StatementBlock(vec![statement]))
            }
        } else {
            Err(ir_error!(token))
        }
    }
}

impl Conv<&Statement> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &Statement) -> IrResult<Self> {
        let token: TokenRange = value.into();
        match value {
            Statement::IdentifierStatement(x) => {
                Conv::conv(context, x.identifier_statement.as_ref())
            }
            Statement::IfStatement(x) => Ok(ir::StatementBlock(vec![Conv::conv(
                context,
                x.if_statement.as_ref(),
            )?])),
            Statement::IfResetStatement(x) => Ok(ir::StatementBlock(vec![Conv::conv(
                context,
                x.if_reset_statement.as_ref(),
            )?])),
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

                        if let Some(dst) = dst.to_assign_destination(context)
                            && let Some(width) = dst.total_width(context)
                        {
                            let (_, expr) = eval_expr(
                                context,
                                Some(dst.r#type.clone()),
                                &x.assignment.expression,
                            )?;

                            let exprs =
                                eval_array_literal(context, Some(&dst.r#type.array), &expr)?;
                            if let Some(exprs) = exprs {
                                let mut statements = vec![];
                                for (i, expr) in exprs.into_iter().enumerate() {
                                    let index = VarIndex::from_index(i, &dst.r#type.array);
                                    let mut dst = dst.clone();
                                    dst.index = index.clone();
                                    let statement = ir::Statement::Assign(ir::AssignStatement {
                                        dst: vec![dst],
                                        width,
                                        expr,
                                        token,
                                    });
                                    statements.push(statement);
                                }
                                Ok(ir::StatementBlock(statements))
                            } else {
                                let statement = ir::Statement::Assign(ir::AssignStatement {
                                    dst: vec![dst],
                                    width,
                                    expr,
                                    token,
                                });
                                Ok(ir::StatementBlock(vec![statement]))
                            }
                        } else {
                            Err(ir_error!(token))
                        }
                    }
                    AssignmentGroup::AssignmentOperator(op) => {
                        let op: ir::Op = Conv::conv(context, op.assignment_operator.as_ref())?;

                        let dst: VarPathSelect = Conv::conv(context, expr)?;
                        let src: VarPathSelect = Conv::conv(context, expr)?;

                        if let Some(dst) = dst.to_assign_destination(context)
                            && let Some(width) = dst.total_width(context)
                            && let Some(src) = src.to_expression(context)
                        {
                            let (_, expr) = eval_expr(
                                context,
                                Some(dst.r#type.clone()),
                                &x.assignment.expression,
                            )?;

                            let expr = ir::Expression::Binary(Box::new(src), op, Box::new(expr));
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
                        // TODO enable after removing checker_connect_operation
                        //check_connect(
                        //    context,
                        //    value.expression_identifier.as_ref(),
                        //    x.assignment.expression.as_ref(),
                        //);

                        let lhs: VarPathSelect = Conv::conv(context, expr)?;
                        let rhs: Vec<VarPathSelect> =
                            Conv::conv(context, x.assignment.expression.as_ref())?;

                        let (comptime, _) =
                            eval_expr(context, None, x.assignment.expression.as_ref())?;

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
                            &symbol.found,
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
                                &symbol,
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

impl Conv<&IfStatement> for ir::Statement {
    fn conv(context: &mut Context, value: &IfStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.r#if.if_token).into();
        if !define_context.is_default() {
            return Ok(ir::Statement::Null);
        }

        let (comptime, cond) = eval_expr(context, None, &value.expression)?;

        if !comptime.r#type.is_binary() {
            let token: TokenRange = value.expression.as_ref().into();
            context.insert_error(AnalyzerError::invalid_logical_operand(false, &token));
        }

        let true_side: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref())?;
        let true_side = true_side.0;

        let mut false_side = vec![];

        for x in &value.if_statement_list {
            let (comptime, cond) = eval_expr(context, None, &x.expression)?;

            if !comptime.r#type.is_binary() {
                let token: TokenRange = x.expression.as_ref().into();
                context.insert_error(AnalyzerError::invalid_logical_operand(false, &token));
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
        }
        if let Some(x) = &value.if_statement_opt {
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

        Ok(ir::Statement::If(ir::IfStatement {
            cond,
            true_side,
            false_side,
            token: value.into(),
        }))
    }
}

impl Conv<&IfResetStatement> for ir::Statement {
    fn conv(context: &mut Context, value: &IfResetStatement) -> IrResult<Self> {
        let define_context: DefineContext = (&value.if_reset.if_reset_token).into();
        if !define_context.is_default() {
            return Ok(ir::Statement::Null);
        }

        let true_side: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref())?;
        let true_side = true_side.0;

        let mut false_side = vec![];

        for x in &value.if_reset_statement_list {
            let (comptime, cond) = eval_expr(context, None, &x.expression)?;

            if !comptime.r#type.is_binary() {
                let token: TokenRange = x.expression.as_ref().into();
                context.insert_error(AnalyzerError::invalid_logical_operand(false, &token));
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
        }
        if let Some(x) = &value.if_reset_statement_opt {
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

        Ok(ir::Statement::IfReset(ir::IfResetStatement {
            true_side,
            false_side,
            token: value.into(),
        }))
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

        if let Some(dst) = dst.to_assign_destination(context)
            && let Some(width) = dst.total_width(context)
        {
            let (_, expr) = eval_expr(context, Some(dst.r#type.clone()), &value.expression)?;
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

        let (beg, end) = eval_range(context, &value.range)?;
        let mut ret = ir::StatementBlock::default();

        for i in beg..end {
            let label = format!("[{}]", i);
            let label = resource_table::insert_str(&label);

            context.push_hier(label);

            // TODO refer type of loop variable
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

            let block: IrResult<ir::StatementBlock> =
                Conv::conv(context, value.statement_block.as_ref());
            context.insert_ir_error(&block);

            if let Ok(mut block) = block {
                ret.0.append(&mut block.0);
            }

            context.pop_hier();
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
