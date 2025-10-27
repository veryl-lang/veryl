use crate::analyzer_error::AnalyzerError;
use crate::conv::checker::function::check_function_call_statement;
use crate::conv::utils::{case_condition, eval_expr, switch_condition};
use crate::conv::{Context, Conv};
use crate::ir::{self, TypedValue, VarPathIndex};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&StatementBlock> for ir::StatementBlock {
    fn conv(context: &mut Context, value: &StatementBlock) -> Self {
        let statements: Vec<_> = value.into();
        ir::StatementBlock(
            statements
                .iter()
                .filter_map(|x| {
                    let x: ir::Statement = Conv::conv(context, x);
                    if x.is_null() { None } else { Some(x) }
                })
                .collect(),
        )
    }
}

impl Conv<&StatementBlockItem> for ir::Statement {
    fn conv(context: &mut Context, value: &StatementBlockItem) -> Self {
        match value {
            StatementBlockItem::VarDeclaration(_) => {
                // TODO
                ir::Statement::Null
            }
            StatementBlockItem::LetStatement(_) => {
                // TODO
                ir::Statement::Null
            }
            StatementBlockItem::ConstDeclaration(_) => {
                // TODO
                ir::Statement::Null
            }
            StatementBlockItem::Statement(x) => Conv::conv(context, x.statement.as_ref()),
        }
    }
}

impl Conv<&Statement> for ir::Statement {
    fn conv(context: &mut Context, value: &Statement) -> Self {
        match value {
            Statement::IdentifierStatement(x) => {
                check_function_call_statement(context, &x.identifier_statement);

                let expr = x.identifier_statement.expression_identifier.as_ref();
                let dst: VarPathIndex = Conv::conv(context, expr);
                let (dst, index) = dst.into();

                if let Some((dst, mut dst_typed_value)) = context.find_path(&dst) {
                    match x.identifier_statement.identifier_statement_group.as_ref() {
                        IdentifierStatementGroup::Assignment(x) => {
                            let (index, select) = index.split(dst_typed_value.r#type.array.len());
                            dst_typed_value.r#type.array.drain(0..index.dimension());
                            let (_, expr) = eval_expr(
                                context,
                                Some(dst_typed_value.r#type),
                                &x.assignment.expression,
                            );
                            ir::Statement::Assign(ir::AssignStatement {
                                dst,
                                index,
                                select,
                                expr,
                            })
                        }
                        IdentifierStatementGroup::FunctionCall(_) => {
                            // TODO
                            ir::Statement::Null
                        }
                    }
                } else {
                    ir::Statement::Null
                }
            }
            Statement::IfStatement(x) => {
                let (typed_value, cond) = eval_expr(context, None, &x.if_statement.expression);

                if !typed_value.r#type.is_binary() {
                    let range: TokenRange = x.if_statement.expression.as_ref().into();
                    context.insert_error(AnalyzerError::invalid_logical_operand(false, &range));
                }

                let true_side: ir::StatementBlock =
                    Conv::conv(context, x.if_statement.statement_block.as_ref());
                let true_side = true_side.0;

                let mut false_side = vec![];

                for x in &x.if_statement.if_statement_list {
                    let (typed_value, cond) = eval_expr(context, None, &x.expression);

                    if !typed_value.r#type.is_binary() {
                        let range: TokenRange = x.expression.as_ref().into();
                        context.insert_error(AnalyzerError::invalid_logical_operand(false, &range));
                    }

                    let true_side: ir::StatementBlock =
                        Conv::conv(context, x.statement_block.as_ref());
                    let true_side = true_side.0;

                    let statement = ir::Statement::If(ir::IfStatement {
                        cond,
                        true_side,
                        false_side: vec![],
                    });

                    if let Some(x) = false_side.last_mut() {
                        if let ir::Statement::If(x) = x {
                            x.insert_leaf_false(vec![statement]);
                        }
                    } else {
                        false_side.push(statement);
                    }
                }
                if let Some(x) = &x.if_statement.if_statement_opt {
                    let block: ir::StatementBlock = Conv::conv(context, x.statement_block.as_ref());
                    let mut block = block.0;

                    if let Some(x) = false_side.last_mut() {
                        if let ir::Statement::If(x) = x {
                            x.insert_leaf_false(block);
                        }
                    } else {
                        false_side.append(&mut block);
                    }
                }

                ir::Statement::If(ir::IfStatement {
                    cond,
                    true_side,
                    false_side,
                })
            }
            Statement::IfResetStatement(x) => {
                let true_side: ir::StatementBlock =
                    Conv::conv(context, x.if_reset_statement.statement_block.as_ref());
                let true_side = true_side.0;

                let mut false_side = vec![];

                for x in &x.if_reset_statement.if_reset_statement_list {
                    let (typed_value, cond) = eval_expr(context, None, &x.expression);

                    if !typed_value.r#type.is_binary() {
                        let range: TokenRange = x.expression.as_ref().into();
                        context.insert_error(AnalyzerError::invalid_logical_operand(false, &range));
                    }

                    let true_side: ir::StatementBlock =
                        Conv::conv(context, x.statement_block.as_ref());
                    let true_side = true_side.0;

                    let statement = ir::Statement::If(ir::IfStatement {
                        cond,
                        true_side,
                        false_side: vec![],
                    });

                    if let Some(x) = false_side.last_mut() {
                        if let ir::Statement::If(x) = x {
                            x.insert_leaf_false(vec![statement]);
                        }
                    } else {
                        false_side.push(statement);
                    }
                }
                if let Some(x) = &x.if_reset_statement.if_reset_statement_opt {
                    let block: ir::StatementBlock = Conv::conv(context, x.statement_block.as_ref());
                    let mut block = block.0;

                    if let Some(x) = false_side.last_mut() {
                        if let ir::Statement::If(x) = x {
                            x.insert_leaf_false(block);
                        }
                    } else {
                        false_side.append(&mut block);
                    }
                }

                ir::Statement::IfReset(ir::IfResetStatement {
                    true_side,
                    false_side,
                })
            }
            Statement::ReturnStatement(_) => {
                // TODO
                ir::Statement::Null
            }
            Statement::BreakStatement(_) => {
                // TODO
                ir::Statement::Null
            }
            Statement::ForStatement(_) => {
                // TODO
                ir::Statement::Null
            }
            Statement::CaseStatement(x) => {
                let tgt: ir::Expression = Conv::conv(context, x.case_statement.expression.as_ref());
                let mut ret = ir::Statement::Null;

                for item in &x.case_statement.case_statement_list {
                    let cond = match item.case_item.case_item_group.as_ref() {
                        CaseItemGroup::CaseCondition(x) => {
                            case_condition(context, &tgt, x.case_condition.as_ref())
                        }
                        CaseItemGroup::Defaul(x) => {
                            let range: TokenRange = x.defaul.as_ref().into();
                            let value = TypedValue::create_value(1u32.into(), 1);
                            let factor = ir::Factor::Value(value, range);
                            ir::Expression::Term(Box::new(factor))
                        }
                    };
                    let true_side: ir::StatementBlock =
                        match item.case_item.case_item_group0.as_ref() {
                            CaseItemGroup0::Statement(x) => {
                                let statement: ir::Statement =
                                    Conv::conv(context, x.statement.as_ref());
                                ir::StatementBlock(vec![statement])
                            }
                            CaseItemGroup0::StatementBlock(x) => {
                                Conv::conv(context, x.statement_block.as_ref())
                            }
                        };
                    let true_side = true_side.0;

                    let statement = ir::Statement::If(ir::IfStatement {
                        cond,
                        true_side,
                        false_side: vec![],
                    });

                    if ret.is_null() {
                        ret = statement;
                    } else if let ir::Statement::If(x) = &mut ret {
                        x.insert_leaf_false(vec![statement]);
                    }
                }

                ret
            }
            Statement::SwitchStatement(x) => {
                let mut ret = ir::Statement::Null;

                for item in &x.switch_statement.switch_statement_list {
                    let cond = match item.switch_item.switch_item_group.as_ref() {
                        SwitchItemGroup::SwitchCondition(x) => {
                            switch_condition(context, x.switch_condition.as_ref())
                        }
                        SwitchItemGroup::Defaul(x) => {
                            let range: TokenRange = x.defaul.as_ref().into();
                            let value = TypedValue::create_value(1u32.into(), 1);
                            let factor = ir::Factor::Value(value, range);
                            ir::Expression::Term(Box::new(factor))
                        }
                    };
                    let true_side: ir::StatementBlock =
                        match item.switch_item.switch_item_group0.as_ref() {
                            SwitchItemGroup0::Statement(x) => {
                                let statement: ir::Statement =
                                    Conv::conv(context, x.statement.as_ref());
                                ir::StatementBlock(vec![statement])
                            }
                            SwitchItemGroup0::StatementBlock(x) => {
                                Conv::conv(context, x.statement_block.as_ref())
                            }
                        };
                    let true_side = true_side.0;

                    let statement = ir::Statement::If(ir::IfStatement {
                        cond,
                        true_side,
                        false_side: vec![],
                    });

                    if ret.is_null() {
                        ret = statement;
                    } else if let ir::Statement::If(x) = &mut ret {
                        x.insert_leaf_false(vec![statement]);
                    }
                }

                ret
            }
        }
    }
}
