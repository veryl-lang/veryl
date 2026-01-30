use crate::cranelift::Context as CraneliftContext;
use crate::ir::context::{Context as ConvContext, Conv};
use crate::ir::{CombValue, FfValue, Op, ProtoStatement, Value};
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::I64;
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::ir as air;
use veryl_analyzer::value::{MaskCache, ValueU64, value_u64_offset};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExpressionContext {
    pub width: usize,
    pub signed: bool,
}

impl From<&air::ExpressionContext> for ExpressionContext {
    fn from(value: &air::ExpressionContext) -> Self {
        Self {
            width: value.width,
            signed: value.signed,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Expression {
    Variable {
        value: *const Value,
        select: Option<(usize, usize)>,
    },
    Value {
        value: Value,
    },
    Unary {
        op: Op,
        x: Box<Expression>,
        expr_context: ExpressionContext,
    },
    Binary {
        x: Box<Expression>,
        op: Op,
        y: Box<Expression>,
        expr_context: ExpressionContext,
    },
    Concatenation {
        elements: Vec<(Box<Expression>, usize, usize)>, // (expr, repeat, elem_width)
    },
    Ternary {
        cond: Box<Expression>,
        true_expr: Box<Expression>,
        false_expr: Box<Expression>,
    },
    DynamicVariable {
        base_ptr: *const Value,
        stride: isize,
        index_expr: Box<Expression>,
        num_elements: usize,
        select: Option<(usize, usize)>,
    },
}

impl Expression {
    pub fn eval(&self, mask_cache: &mut MaskCache) -> Value {
        match self {
            Expression::Variable { value, select } => {
                if let Some((beg, end)) = select {
                    unsafe { (**value).select(*beg, *end) }
                } else {
                    unsafe { (**value).clone() }
                }
            }
            Expression::Value { value } => value.clone(),
            Expression::Unary {
                op,
                x,
                expr_context,
            } => {
                let x = x.eval(mask_cache);
                op.eval_value_unary(&x, expr_context.width, expr_context.signed, mask_cache)
            }
            Expression::Binary {
                x,
                op,
                y,
                expr_context,
            } => {
                let x = x.eval(mask_cache);
                let y = y.eval(mask_cache);
                op.eval_value_binary(&x, &y, expr_context.width, expr_context.signed, mask_cache)
            }
            Expression::Concatenation { elements } => {
                let mut ret = Value::new(0, 0, false);
                for (expr, repeat, _elem_width) in elements {
                    let val = expr.eval(mask_cache);
                    for _ in 0..*repeat {
                        ret = ret.concat(&val);
                    }
                }
                ret
            }
            Expression::Ternary {
                cond,
                true_expr,
                false_expr,
            } => {
                let cond_val = cond.eval(mask_cache);
                let is_nonzero = match &cond_val {
                    Value::U64(x) => (x.payload & !x.mask_xz) != 0,
                    Value::BigUint(x) => *x.payload != (&*x.payload & &*x.mask_xz),
                };
                if is_nonzero {
                    true_expr.eval(mask_cache)
                } else {
                    false_expr.eval(mask_cache)
                }
            }
            Expression::DynamicVariable {
                base_ptr,
                stride,
                index_expr,
                num_elements,
                select,
            } => {
                let idx_val = index_expr.eval(mask_cache);
                let idx = idx_val.to_usize().unwrap_or(0).min(*num_elements - 1);
                let ptr = unsafe {
                    (*base_ptr as *const u8).offset(*stride * idx as isize) as *const Value
                };
                let value = unsafe { (*ptr).clone() };
                if let Some((beg, end)) = select {
                    value.select(*beg, *end)
                } else {
                    value
                }
            }
        }
    }

    pub fn expand(&mut self, width: usize) {
        if let Expression::Value { value } = self {
            *value = value.expand(width, value.signed()).into_owned();
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    pub fn gather_variable(&self, inputs: &mut Vec<*const Value>, outputs: &mut Vec<*const Value>) {
        match self {
            Expression::Variable { value, .. } => inputs.push(*value),
            Expression::Value { .. } => (),
            Expression::Unary { x, .. } => {
                x.gather_variable(inputs, outputs);
            }
            Expression::Binary { x, y, .. } => {
                x.gather_variable(inputs, outputs);
                y.gather_variable(inputs, outputs);
            }
            Expression::Concatenation { elements } => {
                for (expr, _, _) in elements {
                    expr.gather_variable(inputs, outputs);
                }
            }
            Expression::Ternary {
                cond,
                true_expr,
                false_expr,
            } => {
                cond.gather_variable(inputs, outputs);
                true_expr.gather_variable(inputs, outputs);
                false_expr.gather_variable(inputs, outputs);
            }
            Expression::DynamicVariable {
                base_ptr,
                stride,
                index_expr,
                num_elements,
                ..
            } => {
                index_expr.gather_variable(inputs, outputs);
                for i in 0..*num_elements {
                    let ptr = unsafe {
                        (*base_ptr as *const u8).offset(*stride * i as isize) as *const Value
                    };
                    inputs.push(ptr);
                }
            }
        }
    }
}

impl ProtoExpression {
    pub fn gather_variable_offsets(&self, inputs: &mut Vec<(bool, isize)>) {
        match self {
            ProtoExpression::Variable { offset, is_ff, .. } => inputs.push((*is_ff, *offset)),
            ProtoExpression::Value { .. } => (),
            ProtoExpression::Unary { x, .. } => x.gather_variable_offsets(inputs),
            ProtoExpression::Binary { x, y, .. } => {
                x.gather_variable_offsets(inputs);
                y.gather_variable_offsets(inputs);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (expr, _, _) in elements {
                    expr.gather_variable_offsets(inputs);
                }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                cond.gather_variable_offsets(inputs);
                true_expr.gather_variable_offsets(inputs);
                false_expr.gather_variable_offsets(inputs);
            }
            ProtoExpression::DynamicVariable {
                base_offset,
                stride,
                is_ff,
                index_expr,
                num_elements,
                ..
            } => {
                index_expr.gather_variable_offsets(inputs);
                for i in 0..*num_elements {
                    let offset = *base_offset + *stride * i as isize;
                    inputs.push((*is_ff, offset));
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum ProtoExpression {
    Variable {
        offset: isize,
        is_ff: bool,
        select: Option<(usize, usize)>,
        width: usize,
        expr_context: ExpressionContext,
    },
    Value {
        value: Value,
        width: usize,
        expr_context: ExpressionContext,
    },
    Unary {
        op: Op,
        x: Box<ProtoExpression>,
        width: usize,
        expr_context: ExpressionContext,
    },
    Binary {
        x: Box<ProtoExpression>,
        op: Op,
        y: Box<ProtoExpression>,
        width: usize,
        expr_context: ExpressionContext,
    },
    Concatenation {
        elements: Vec<(Box<ProtoExpression>, usize, usize)>, // (expr, repeat, elem_width)
        width: usize,
        expr_context: ExpressionContext,
    },
    Ternary {
        cond: Box<ProtoExpression>,
        true_expr: Box<ProtoExpression>,
        false_expr: Box<ProtoExpression>,
        width: usize,
        expr_context: ExpressionContext,
    },
    DynamicVariable {
        base_offset: isize,
        stride: isize,
        is_ff: bool,
        index_expr: Box<ProtoExpression>,
        num_elements: usize,
        select: Option<(usize, usize)>,
        width: usize,
        expr_context: ExpressionContext,
    },
}

impl ProtoExpression {
    pub fn can_build_binary(&self) -> bool {
        match self {
            ProtoExpression::Variable { width, .. } => *width <= 64,
            ProtoExpression::Value { value, .. } => matches!(value, Value::U64(_)),
            ProtoExpression::Unary { op, x, .. } => {
                x.can_build_binary()
                    && matches!(
                        op,
                        Op::Add
                            | Op::Sub
                            | Op::BitNot
                            | Op::BitAnd
                            | Op::BitNand
                            | Op::BitOr
                            | Op::BitNor
                            | Op::LogicNot
                            | Op::BitXor
                            | Op::BitXnor
                    )
            }
            ProtoExpression::Binary { x, op, y, .. } => {
                x.can_build_binary()
                    && y.can_build_binary()
                    && matches!(
                        op,
                        Op::Add
                            | Op::Sub
                            | Op::Mul
                            | Op::Div
                            | Op::Rem
                            | Op::BitAnd
                            | Op::BitOr
                            | Op::BitXor
                            | Op::BitXnor
                            | Op::Eq
                            | Op::Ne
                            | Op::EqWildcard
                            | Op::NeWildcard
                            | Op::Greater
                            | Op::GreaterEq
                            | Op::Less
                            | Op::LessEq
                            | Op::LogicShiftL
                            | Op::LogicShiftR
                            | Op::ArithShiftL
                            | Op::ArithShiftR
                            | Op::Pow
                            | Op::LogicAnd
                            | Op::LogicOr
                    )
            }
            ProtoExpression::Concatenation {
                elements, width, ..
            } => *width <= 64 && elements.iter().all(|(expr, _, _)| expr.can_build_binary()),
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                width,
                ..
            } => {
                *width <= 64
                    && cond.can_build_binary()
                    && true_expr.can_build_binary()
                    && false_expr.can_build_binary()
            }
            ProtoExpression::DynamicVariable {
                width, index_expr, ..
            } => *width <= 64 && index_expr.can_build_binary(),
        }
    }

    pub fn width(&self) -> usize {
        match self {
            ProtoExpression::Variable { width, .. } => *width,
            ProtoExpression::Value { width, .. } => *width,
            ProtoExpression::Unary { width, .. } => *width,
            ProtoExpression::Binary { width, .. } => *width,
            ProtoExpression::Concatenation { width, .. } => *width,
            ProtoExpression::Ternary { width, .. } => *width,
            ProtoExpression::DynamicVariable { width, .. } => *width,
        }
    }

    pub fn expr_context(&self) -> &ExpressionContext {
        match self {
            ProtoExpression::Variable { expr_context, .. } => expr_context,
            ProtoExpression::Value { expr_context, .. } => expr_context,
            ProtoExpression::Unary { expr_context, .. } => expr_context,
            ProtoExpression::Binary { expr_context, .. } => expr_context,
            ProtoExpression::Concatenation { expr_context, .. } => expr_context,
            ProtoExpression::Ternary { expr_context, .. } => expr_context,
            ProtoExpression::DynamicVariable { expr_context, .. } => expr_context,
        }
    }

    pub fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut FfValue,
        comb_values_ptr: *mut CombValue,
    ) -> Expression {
        match self {
            ProtoExpression::Variable {
                offset,
                is_ff,
                select,
                ..
            } => {
                let value = if *is_ff {
                    unsafe { (ff_values_ptr as *const u8).add(*offset as usize) as *const Value }
                } else {
                    unsafe { (comb_values_ptr as *const u8).add(*offset as usize) as *const Value }
                };
                Expression::Variable {
                    value,
                    select: *select,
                }
            }
            ProtoExpression::Value { value, .. } => Expression::Value {
                value: value.clone(),
            },
            ProtoExpression::Unary {
                op,
                x,
                expr_context,
                ..
            } => {
                let x = x.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                Expression::Unary {
                    op: *op,
                    x: Box::new(x),
                    expr_context: *expr_context,
                }
            }
            ProtoExpression::Binary {
                x,
                op,
                y,
                expr_context,
                ..
            } => {
                let x = x.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                let y = y.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                Expression::Binary {
                    x: Box::new(x),
                    op: *op,
                    y: Box::new(y),
                    expr_context: *expr_context,
                }
            }
            ProtoExpression::Concatenation { elements, .. } => {
                let elements = elements
                    .iter()
                    .map(|(expr, repeat, elem_width)| {
                        let expr = expr.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                        (Box::new(expr), *repeat, *elem_width)
                    })
                    .collect();
                Expression::Concatenation { elements }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                let cond = cond.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                let true_expr = true_expr.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                let false_expr = false_expr.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                Expression::Ternary {
                    cond: Box::new(cond),
                    true_expr: Box::new(true_expr),
                    false_expr: Box::new(false_expr),
                }
            }
            ProtoExpression::DynamicVariable {
                base_offset,
                stride,
                is_ff,
                index_expr,
                num_elements,
                select,
                ..
            } => {
                let base_ptr = if *is_ff {
                    unsafe { (ff_values_ptr as *const u8).offset(*base_offset) as *const Value }
                } else {
                    unsafe { (comb_values_ptr as *const u8).offset(*base_offset) as *const Value }
                };
                let index_expr = index_expr.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                Expression::DynamicVariable {
                    base_ptr,
                    stride: *stride,
                    index_expr: Box::new(index_expr),
                    num_elements: *num_elements,
                    select: *select,
                }
            }
        }
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<(CraneliftValue, Option<CraneliftValue>)> {
        match self {
            ProtoExpression::Variable {
                offset,
                width,
                is_ff,
                select,
                ..
            } => {
                if *width > 64 {
                    return None;
                }

                let load_mem_flag = MemFlags::trusted().with_readonly();

                let base_addr = if *is_ff {
                    context.ff_values
                } else {
                    context.comb_values
                };

                let offset = (*offset + value_u64_offset()) as i32;

                let mut payload = builder.ins().load(I64, load_mem_flag, base_addr, offset);
                let mut mask_xz = if context.use_4state {
                    let mask_xz = builder
                        .ins()
                        .load(I64, load_mem_flag, base_addr, offset + 8);
                    Some(mask_xz)
                } else {
                    None
                };

                if let Some((beg, end)) = select {
                    let mask = ValueU64::gen_mask(beg - end + 1);

                    payload = builder.ins().ushr_imm(payload, *end as i64);
                    payload = builder.ins().band_imm(payload, mask as i64);

                    if context.use_4state {
                        let x = builder.ins().ushr_imm(mask_xz.unwrap(), *end as i64);
                        let x = builder.ins().band_imm(x, mask as i64);
                        mask_xz = Some(x);
                    }
                }

                Some((payload, mask_xz))
            }
            ProtoExpression::Value { value, .. } => {
                if let Value::U64(x) = value {
                    let payload = x.payload as i64;
                    let payload = builder.ins().iconst(I64, payload);

                    let mask_xz = if context.use_4state {
                        let mask_xz = x.mask_xz as i64;
                        let mask_xz = builder.ins().iconst(I64, mask_xz);
                        Some(mask_xz)
                    } else {
                        None
                    };
                    Some((payload, mask_xz))
                } else {
                    None
                }
            }
            ProtoExpression::Unary {
                op,
                x,
                expr_context,
                ..
            } => {
                let (mut x_payload, mut x_mask_xz) = x.build_binary(context, builder)?;

                let width = expr_context.width;
                if expr_context.signed {
                    (x_payload, x_mask_xz) =
                        expand_sign(width, x.width(), x_payload, x_mask_xz, builder);
                }

                let payload = match op {
                    Op::Add => x_payload,
                    Op::Sub => {
                        let mask = ValueU64::gen_mask(width);
                        let x0 = builder.ins().bxor_imm(x_payload, mask as i64);
                        builder.ins().iadd_imm(x0, 1)
                    }
                    Op::BitNot => {
                        let mask = ValueU64::gen_mask(width);
                        builder.ins().bxor_imm(x_payload, mask as i64)
                    }
                    Op::BitAnd => {
                        let mask = ValueU64::gen_mask(x.width());
                        let ret = builder.ins().icmp_imm(IntCC::Equal, x_payload, mask as i64);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitNand => {
                        let mask = ValueU64::gen_mask(x.width());
                        let ret = builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, x_payload, mask as i64);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitOr => {
                        let ret = builder.ins().icmp_imm(IntCC::NotEqual, x_payload, 0);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitNor | Op::LogicNot => {
                        let ret = builder.ins().icmp_imm(IntCC::Equal, x_payload, 0);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitXor => {
                        let x0 = builder.ins().popcnt(x_payload);
                        builder.ins().urem_imm(x0, 2)
                    }
                    Op::BitXnor => {
                        let x0 = builder.ins().popcnt(x_payload);
                        let x1 = builder.ins().icmp_imm(IntCC::Equal, x0, 0);
                        builder.ins().uextend(I64, x1)
                    }
                    _ => return None,
                };

                match op {
                    Op::Add => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            Some((payload, Some(x_mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::Sub => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = builder.ins().iconst(I64, 0xffffffff);
                            let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);

                            let payload = builder.ins().select(is_xz, context.zero, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitNot => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = ValueU64::gen_mask(width);
                            let x0 = builder.ins().bxor_imm(x_mask_xz, mask as i64);
                            let payload = builder.ins().band(payload, x0);

                            Some((payload, Some(x_mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitAnd | Op::BitNand | Op::BitOr | Op::BitNor | Op::LogicNot => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = ValueU64::gen_mask(x.width());

                            let (is_one, is_zero, is_x) = match op {
                                Op::BitAnd => {
                                    let x0 = builder.ins().bor(x_payload, x_mask_xz);
                                    let x1 =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x0, mask as i64);
                                    let is_zero = x1;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (None, Some(is_zero), is_x)
                                }
                                Op::BitNand => {
                                    let x0 = builder.ins().bor(x_payload, x_mask_xz);
                                    let x1 =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x0, mask as i64);
                                    let is_one = x1;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (Some(is_one), None, is_x)
                                }
                                Op::BitOr => {
                                    let x0 = builder.ins().bxor_imm(x_mask_xz, mask as i64);
                                    let x1 = builder.ins().band(x_payload, x0);
                                    let x2 = builder.ins().icmp_imm(IntCC::NotEqual, x1, 0);
                                    let is_one = x2;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (Some(is_one), None, is_x)
                                }
                                Op::BitNor | Op::LogicNot => {
                                    let x0 = builder.ins().bxor_imm(x_mask_xz, mask as i64);
                                    let x1 = builder.ins().band(x_payload, x0);
                                    let x2 = builder.ins().icmp_imm(IntCC::NotEqual, x1, 0);
                                    let is_zero = x2;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (None, Some(is_zero), is_x)
                                }
                                _ => unreachable!(),
                            };

                            if let Some(is_one) = is_one {
                                let one = builder.ins().iconst(I64, 1);
                                let payload = builder.ins().select(is_one, one, context.zero);

                                let x1 = builder.ins().bnot(is_one);
                                let x2 = builder.ins().band(x1, is_x);
                                let mask_xz = builder.ins().select(x2, one, context.zero);

                                Some((payload, Some(mask_xz)))
                            } else if let Some(is_zero) = is_zero {
                                let one = builder.ins().iconst(I64, 1);
                                let x0 = builder.ins().bor(is_zero, is_x);
                                let payload = builder.ins().select(x0, context.zero, one);

                                let x1 = builder.ins().bnot(is_zero);
                                let x2 = builder.ins().band(x1, is_x);
                                let mask_xz = builder.ins().select(x2, one, context.zero);

                                Some((payload, Some(mask_xz)))
                            } else {
                                unreachable!();
                            }
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitXor | Op::BitXnor => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = builder.ins().iconst(I64, 1);
                            let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);

                            let payload = builder.ins().select(is_xz, context.zero, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            ProtoExpression::Binary {
                x,
                op,
                y,
                expr_context,
                ..
            } => {
                let (mut x_payload, mut x_mask_xz) = x.build_binary(context, builder)?;
                let (mut y_payload, mut y_mask_xz) = y.build_binary(context, builder)?;

                let signed = expr_context.signed;

                if signed {
                    let width = if matches!(
                        op,
                        Op::Div | Op::Rem | Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq
                    ) {
                        64
                    } else {
                        expr_context.width
                    };
                    (x_payload, x_mask_xz) =
                        expand_sign(width, x.width(), x_payload, x_mask_xz, builder);
                    (y_payload, y_mask_xz) =
                        expand_sign(width, y.width(), y_payload, y_mask_xz, builder);
                }

                let payload = match op {
                    Op::Add => builder.ins().iadd(x_payload, y_payload),
                    Op::Sub => builder.ins().isub(x_payload, y_payload),
                    Op::Mul => builder.ins().imul(x_payload, y_payload),
                    Op::Div => {
                        let block0 = builder.create_block();
                        let block1 = builder.create_block();
                        let block2 = builder.create_block();
                        builder.append_block_param(block2, I64);

                        let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                        builder.ins().brif(zero_div, block0, &[], block1, &[]);

                        builder.switch_to_block(block0);
                        let ret = builder.ins().iconst(I64, 0);
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);

                        builder.switch_to_block(block1);
                        let ret = if signed {
                            builder.ins().sdiv(x_payload, y_payload)
                        } else {
                            builder.ins().udiv(x_payload, y_payload)
                        };
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);
                        builder.switch_to_block(block2);
                        builder.block_params(block2)[0]
                    }
                    Op::Rem => {
                        let block0 = builder.create_block();
                        let block1 = builder.create_block();
                        let block2 = builder.create_block();
                        builder.append_block_param(block2, I64);

                        let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                        builder.ins().brif(zero_div, block0, &[], block1, &[]);

                        builder.switch_to_block(block0);
                        let ret = builder.ins().iconst(I64, 0);
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);

                        builder.switch_to_block(block1);
                        let ret = if signed {
                            builder.ins().srem(x_payload, y_payload)
                        } else {
                            builder.ins().urem(x_payload, y_payload)
                        };
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);
                        builder.switch_to_block(block2);
                        builder.block_params(block2)[0]
                    }
                    Op::BitAnd => builder.ins().band(x_payload, y_payload),
                    Op::BitOr => builder.ins().bor(x_payload, y_payload),
                    Op::BitXor => builder.ins().bxor(x_payload, y_payload),
                    Op::BitXnor => builder.ins().bxor_not(x_payload, y_payload),
                    Op::Eq => {
                        let ret = builder.ins().icmp(IntCC::Equal, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::Ne => {
                        let ret = builder.ins().icmp(IntCC::NotEqual, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::EqWildcard => {
                        let ret = builder.ins().icmp(IntCC::Equal, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::NeWildcard => {
                        let ret = builder.ins().icmp(IntCC::NotEqual, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::Greater => {
                        let cc = if signed {
                            IntCC::SignedGreaterThan
                        } else {
                            IntCC::UnsignedGreaterThan
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::GreaterEq => {
                        let cc = if signed {
                            IntCC::SignedGreaterThanOrEqual
                        } else {
                            IntCC::UnsignedGreaterThanOrEqual
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::Less => {
                        let cc = if signed {
                            IntCC::SignedLessThan
                        } else {
                            IntCC::UnsignedLessThan
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::LessEq => {
                        let cc = if signed {
                            IntCC::SignedLessThanOrEqual
                        } else {
                            IntCC::UnsignedLessThanOrEqual
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::LogicShiftL | Op::ArithShiftL => builder.ins().ishl(x_payload, y_payload),
                    Op::LogicShiftR => builder.ins().ushr(x_payload, y_payload),
                    Op::ArithShiftR => {
                        if signed {
                            // Sign-extend x_payload to 64-bit, then sshr, to get MSB fill
                            let shl_amount = (64 - x.width()) as i64;
                            let shifted_up = builder.ins().ishl_imm(x_payload, shl_amount);
                            let sign_extended = builder.ins().sshr_imm(shifted_up, shl_amount);
                            builder.ins().sshr(sign_extended, y_payload)
                        } else {
                            builder.ins().ushr(x_payload, y_payload)
                        }
                    }
                    Op::Pow => {
                        // Binary exponentiation: result=1, while exp>0 { if odd: result*=base; base*=base; exp>>=1 }
                        let loop_header = builder.create_block();
                        builder.append_block_param(loop_header, I64); // result
                        builder.append_block_param(loop_header, I64); // base
                        builder.append_block_param(loop_header, I64); // exp

                        let loop_body = builder.create_block();

                        let exit_block = builder.create_block();
                        builder.append_block_param(exit_block, I64); // final result

                        let one_val = builder.ins().iconst(I64, 1);
                        builder.ins().jump(
                            loop_header,
                            &[
                                BlockArg::Value(one_val),
                                BlockArg::Value(x_payload),
                                BlockArg::Value(y_payload),
                            ],
                        );

                        builder.switch_to_block(loop_header);
                        let result = builder.block_params(loop_header)[0];
                        let base = builder.block_params(loop_header)[1];
                        let exp = builder.block_params(loop_header)[2];
                        let exp_zero = builder.ins().icmp_imm(IntCC::Equal, exp, 0);
                        builder.ins().brif(
                            exp_zero,
                            exit_block,
                            &[BlockArg::Value(result)],
                            loop_body,
                            &[],
                        );

                        builder.switch_to_block(loop_body);
                        let odd = builder.ins().band_imm(exp, 1);
                        let is_odd = builder.ins().icmp_imm(IntCC::NotEqual, odd, 0);
                        let result_mul = builder.ins().imul(result, base);
                        let new_result = builder.ins().select(is_odd, result_mul, result);
                        let new_base = builder.ins().imul(base, base);
                        let new_exp = builder.ins().ushr_imm(exp, 1);
                        builder.ins().jump(
                            loop_header,
                            &[
                                BlockArg::Value(new_result),
                                BlockArg::Value(new_base),
                                BlockArg::Value(new_exp),
                            ],
                        );

                        builder.switch_to_block(exit_block);
                        builder.block_params(exit_block)[0]
                    }
                    Op::LogicAnd | Op::LogicOr => {
                        let x_nonzero = if let Some(ref xm) = x_mask_xz {
                            let known = builder.ins().band_not(x_payload, *xm);
                            builder.ins().icmp_imm(IntCC::NotEqual, known, 0)
                        } else {
                            builder.ins().icmp_imm(IntCC::NotEqual, x_payload, 0)
                        };
                        let y_nonzero = if let Some(ref ym) = y_mask_xz {
                            let known = builder.ins().band_not(y_payload, *ym);
                            builder.ins().icmp_imm(IntCC::NotEqual, known, 0)
                        } else {
                            builder.ins().icmp_imm(IntCC::NotEqual, y_payload, 0)
                        };
                        let is_one = if matches!(op, Op::LogicAnd) {
                            builder.ins().band(x_nonzero, y_nonzero)
                        } else {
                            builder.ins().bor(x_nonzero, y_nonzero)
                        };
                        builder.ins().uextend(I64, is_one)
                    }
                    _ => return None,
                };

                match op {
                    Op::Add
                    | Op::Sub
                    | Op::Mul
                    | Op::Div
                    | Op::Rem
                    | Op::Greater
                    | Op::GreaterEq
                    | Op::Less
                    | Op::LessEq
                    | Op::Pow => {
                        let mut is_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                let x_is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                let y_is_xz = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                let is_xz = builder.ins().bor(x_is_xz, y_is_xz);
                                Some(is_xz)
                            }
                            (Some(x_mask_xz), None) => {
                                let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                Some(is_xz)
                            }
                            (None, Some(y_mask_xz)) => {
                                let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                Some(is_xz)
                            }
                            (None, None) => None,
                        };

                        if matches!(op, Op::Div | Op::Rem) && context.use_4state {
                            let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                            if let Some(x) = is_xz {
                                is_xz = Some(builder.ins().bor(x, zero_div));
                            } else {
                                is_xz = Some(zero_div);
                            }
                        }

                        if let Some(is_xz) = is_xz {
                            let mask = if matches!(
                                op,
                                Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq
                            ) {
                                builder.ins().iconst(I64, 1)
                            } else {
                                builder.ins().iconst(I64, 0xffffffff)
                            };

                            let payload = builder.ins().select(is_xz, context.zero, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitAnd => {
                        let mask_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // (x_mask_xz & y_mask_xz) | (x_mask_xz & !y_mask_xz & y_payload) | (y_mask_xz & !x_mask_xz & x_payload)
                                let x0 = builder.ins().band(x_mask_xz, y_mask_xz);
                                let x1 = builder.ins().bnot(y_mask_xz);
                                let x2 = builder.ins().band(x_mask_xz, x1);
                                let x3 = builder.ins().band(x2, y_payload);
                                let x4 = builder.ins().bnot(x_mask_xz);
                                let x5 = builder.ins().band(y_mask_xz, x4);
                                let x6 = builder.ins().band(x5, x_payload);
                                let x7 = builder.ins().bor(x0, x3);
                                let x8 = builder.ins().bor(x7, x6);
                                Some(x8)
                            }
                            (Some(x_mask_xz), None) => {
                                // (x_mask_xz & y_payload)
                                let x0 = builder.ins().band(x_mask_xz, y_payload);
                                Some(x0)
                            }
                            (None, Some(y_mask_xz)) => {
                                // (y_mask_xz & x_payload)
                                let x0 = builder.ins().band(y_mask_xz, x_payload);
                                Some(x0)
                            }
                            (None, None) => None,
                        };
                        if let Some(mask_xz) = mask_xz {
                            let x0 = builder.ins().bnot(mask_xz);
                            let payload = builder.ins().band(payload, x0);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitOr => {
                        let mask_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // (x_mask_xz & y_mask_xz) | (x_mask_xz & !y_mask_xz & !y_payload) | (y_mask_xz & !x_mask_xz & !x_payload)
                                let x0 = builder.ins().band(x_mask_xz, y_mask_xz);
                                let x1 = builder.ins().bnot(y_mask_xz);
                                let x2 = builder.ins().bnot(y_payload);
                                let x3 = builder.ins().band(x_mask_xz, x1);
                                let x4 = builder.ins().band(x3, x2);
                                let x5 = builder.ins().bnot(x_mask_xz);
                                let x6 = builder.ins().bnot(x_payload);
                                let x7 = builder.ins().band(y_mask_xz, x5);
                                let x8 = builder.ins().band(x7, x6);
                                let x9 = builder.ins().bor(x0, x4);
                                let x10 = builder.ins().bor(x9, x8);
                                Some(x10)
                            }
                            (Some(x_mask_xz), None) => {
                                // (x_mask_xz & !y_payload)
                                let x0 = builder.ins().bnot(y_payload);
                                let x1 = builder.ins().band(x_mask_xz, x0);
                                Some(x1)
                            }
                            (None, Some(y_mask_xz)) => {
                                // (y_mask_xz & !x_payload)
                                let x0 = builder.ins().bnot(x_payload);
                                let x1 = builder.ins().band(y_mask_xz, x0);
                                Some(x1)
                            }
                            (None, None) => None,
                        };
                        if let Some(mask_xz) = mask_xz {
                            let x0 = builder.ins().bnot(mask_xz);
                            let payload = builder.ins().band(payload, x0);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitXor | Op::BitXnor => {
                        let mask_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // x_mask_xz | y_mask_xz
                                let x0 = builder.ins().bor(x_mask_xz, y_mask_xz);
                                Some(x0)
                            }
                            (Some(x_mask_xz), None) => Some(x_mask_xz),
                            (None, Some(y_mask_xz)) => Some(y_mask_xz),
                            (None, None) => None,
                        };
                        if let Some(mask_xz) = mask_xz {
                            let x0 = builder.ins().bnot(mask_xz);
                            let payload = builder.ins().band(payload, x0);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::Eq | Op::Ne => {
                        let is_onezero_x = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // (x_payload & !x_mask_xz) != (y_payload & !y_mask_xz)
                                let x0 = builder.ins().bnot(x_mask_xz);
                                let x1 = builder.ins().band(x_payload, x0);
                                let x2 = builder.ins().bnot(y_mask_xz);
                                let x3 = builder.ins().band(y_payload, x2);
                                let x4 = builder.ins().icmp(IntCC::NotEqual, x1, x3);

                                // x_mask_xz != 0 | y_mask_xz != 0
                                let x5 = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                let x6 = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                let x7 = builder.ins().bor(x5, x6);
                                Some((x4, x7))
                            }
                            (Some(x_mask_xz), None) => {
                                // (x_payload & !x_mask_xz) != y_payload
                                let x0 = builder.ins().bnot(x_mask_xz);
                                let x1 = builder.ins().band(x_payload, x0);
                                let x2 = builder.ins().icmp(IntCC::NotEqual, x1, y_payload);

                                // x_mask_xz != 0
                                let x3 = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                Some((x2, x3))
                            }
                            (None, Some(y_mask_xz)) => {
                                // x_payload != (y_payload & !y_mask_xz)
                                let x0 = builder.ins().bnot(y_mask_xz);
                                let x1 = builder.ins().band(y_payload, x0);
                                let x2 = builder.ins().icmp(IntCC::NotEqual, x_payload, x1);

                                // y_mask_xz != 0
                                let x3 = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                Some((x2, x3))
                            }
                            (None, None) => None,
                        };
                        if let Some((is_onezero, is_x)) = is_onezero_x {
                            let one = builder.ins().iconst(I64, 1);

                            let payload = match op {
                                Op::Eq => {
                                    let is_zero = is_onezero;
                                    let x0 = builder.ins().bor(is_zero, is_x);
                                    builder.ins().select(x0, context.zero, one)
                                }
                                Op::Ne => {
                                    let is_one = is_onezero;
                                    builder.ins().select(is_one, one, context.zero)
                                }
                                _ => unreachable!(),
                            };

                            let x1 = builder.ins().bnot(is_onezero);
                            let x2 = builder.ins().band(x1, is_x);
                            let mask_xz = builder.ins().select(x2, one, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::EqWildcard | Op::NeWildcard => {
                        let result = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                let compare_mask = builder.ins().bnot(y_mask_xz);
                                let xor_val = builder.ins().bxor(x_payload, y_payload);
                                let val_diff = builder.ins().band(xor_val, compare_mask);
                                let not_x_mask = builder.ins().bnot(x_mask_xz);
                                let definite_diff = builder.ins().band(val_diff, not_x_mask);
                                let is_mismatch =
                                    builder.ins().icmp_imm(IntCC::NotEqual, definite_diff, 0);
                                let x_in_compare = builder.ins().band(x_mask_xz, compare_mask);
                                let is_x = builder.ins().icmp_imm(IntCC::NotEqual, x_in_compare, 0);
                                Some((is_mismatch, is_x))
                            }
                            (Some(x_mask_xz), None) => {
                                let xor_val = builder.ins().bxor(x_payload, y_payload);
                                let not_x_mask = builder.ins().bnot(x_mask_xz);
                                let definite_diff = builder.ins().band(xor_val, not_x_mask);
                                let is_mismatch =
                                    builder.ins().icmp_imm(IntCC::NotEqual, definite_diff, 0);
                                let is_x = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                Some((is_mismatch, is_x))
                            }
                            (None, Some(y_mask_xz)) => {
                                let compare_mask = builder.ins().bnot(y_mask_xz);
                                let xor_val = builder.ins().bxor(x_payload, y_payload);
                                let val_diff = builder.ins().band(xor_val, compare_mask);
                                let is_mismatch =
                                    builder.ins().icmp_imm(IntCC::NotEqual, val_diff, 0);
                                let one = builder.ins().iconst(I64, 1);
                                let payload = match op {
                                    Op::EqWildcard => {
                                        builder.ins().select(is_mismatch, context.zero, one)
                                    }
                                    Op::NeWildcard => {
                                        builder.ins().select(is_mismatch, one, context.zero)
                                    }
                                    _ => unreachable!(),
                                };
                                return Some((payload, Some(context.zero)));
                            }
                            (None, None) => {
                                // 2-state mode: if y is a literal without X/Z,
                                // wildcard comparison is equivalent to Eq/Ne
                                let y_has_no_xz = match y.as_ref() {
                                    ProtoExpression::Value {
                                        value: Value::U64(v),
                                        ..
                                    } => v.mask_xz == 0,
                                    _ => false,
                                };
                                if y_has_no_xz {
                                    let cc = match op {
                                        Op::EqWildcard => IntCC::Equal,
                                        _ => IntCC::NotEqual,
                                    };
                                    let ret = builder.ins().icmp(cc, x_payload, y_payload);
                                    let ret = builder.ins().uextend(I64, ret);
                                    return Some((ret, None));
                                }
                                return None;
                            }
                        };

                        let (is_mismatch, is_x) = result.unwrap();
                        let one = builder.ins().iconst(I64, 1);
                        let payload = match op {
                            Op::EqWildcard => {
                                let x0 = builder.ins().bor(is_mismatch, is_x);
                                builder.ins().select(x0, context.zero, one)
                            }
                            Op::NeWildcard => {
                                builder.ins().select(is_mismatch, one, context.zero)
                            }
                            _ => unreachable!(),
                        };
                        let not_mismatch = builder.ins().bnot(is_mismatch);
                        let uncertain = builder.ins().band(not_mismatch, is_x);
                        let mask_xz = builder.ins().select(uncertain, one, context.zero);
                        Some((payload, Some(mask_xz)))
                    }
                    Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR => {
                        if let Some(y_mask_xz) = y_mask_xz {
                            // y has X/Z → entire result becomes X
                            let y_is_xz = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                            let full_mask = builder
                                .ins()
                                .iconst(I64, ValueU64::gen_mask(expr_context.width) as i64);

                            let shifted_mask_xz = if let Some(x_mask_xz) = x_mask_xz {
                                let shifted = shift_mask_xz(
                                    op,
                                    signed,
                                    x.width(),
                                    x_mask_xz,
                                    y_payload,
                                    builder,
                                );
                                builder.ins().select(y_is_xz, full_mask, shifted)
                            } else {
                                builder.ins().select(y_is_xz, full_mask, context.zero)
                            };

                            let final_payload =
                                builder.ins().select(y_is_xz, context.zero, payload);
                            Some((final_payload, Some(shifted_mask_xz)))
                        } else if let Some(x_mask_xz) = x_mask_xz {
                            let shifted =
                                shift_mask_xz(op, signed, x.width(), x_mask_xz, y_payload, builder);
                            Some((payload, Some(shifted)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::LogicAnd | Op::LogicOr => {
                        let is_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                let x_is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                let y_is_xz = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                Some(builder.ins().bor(x_is_xz, y_is_xz))
                            }
                            (Some(x_mask_xz), None) => {
                                Some(builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0))
                            }
                            (None, Some(y_mask_xz)) => {
                                Some(builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0))
                            }
                            (None, None) => None,
                        };
                        if let Some(is_xz) = is_xz {
                            let is_one = builder.ins().icmp_imm(IntCC::NotEqual, payload, 0);
                            let not_one = builder.ins().bnot(is_one);
                            let has_x = builder.ins().band(not_one, is_xz);
                            let one = builder.ins().iconst(I64, 1);
                            let mask_xz = builder.ins().select(has_x, one, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            ProtoExpression::Concatenation {
                elements, width, ..
            } => {
                if *width > 64 {
                    return None;
                }

                let mut acc_payload = context.zero;
                let mut acc_mask_xz: Option<CraneliftValue> = if context.use_4state {
                    Some(context.zero)
                } else {
                    None
                };

                for (expr, repeat, elem_width) in elements {
                    let (elem_payload, elem_mask_xz) = expr.build_binary(context, builder)?;
                    let ew = *elem_width;

                    for _ in 0..*repeat {
                        // acc = (acc << elem_width) | elem
                        acc_payload = builder.ins().ishl_imm(acc_payload, ew as i64);
                        acc_payload = builder.ins().bor(acc_payload, elem_payload);

                        if let Some(acc_xz) = acc_mask_xz {
                            let shifted = builder.ins().ishl_imm(acc_xz, ew as i64);
                            acc_mask_xz = if let Some(elem_xz) = elem_mask_xz {
                                Some(builder.ins().bor(shifted, elem_xz))
                            } else {
                                Some(shifted)
                            };
                        }
                    }
                }

                // Mask to width
                let mask = ValueU64::gen_mask(*width);
                acc_payload = builder.ins().band_imm(acc_payload, mask as i64);
                if let Some(acc_xz) = acc_mask_xz {
                    acc_mask_xz = Some(builder.ins().band_imm(acc_xz, mask as i64));
                }

                Some((acc_payload, acc_mask_xz))
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                let (cond_payload, cond_mask_xz) = cond.build_binary(context, builder)?;
                let (true_payload, true_mask_xz) = true_expr.build_binary(context, builder)?;
                let (false_payload, false_mask_xz) = false_expr.build_binary(context, builder)?;

                let effective_cond = if let Some(mask_xz) = cond_mask_xz {
                    builder.ins().band_not(cond_payload, mask_xz)
                } else {
                    cond_payload
                };
                let cond_nz = builder.ins().icmp_imm(IntCC::NotEqual, effective_cond, 0);
                let payload = builder.ins().select(cond_nz, true_payload, false_payload);

                let mask_xz = match (true_mask_xz, false_mask_xz) {
                    (Some(t_xz), Some(f_xz)) => Some(builder.ins().select(cond_nz, t_xz, f_xz)),
                    (Some(t_xz), None) => Some(builder.ins().select(cond_nz, t_xz, context.zero)),
                    (None, Some(f_xz)) => Some(builder.ins().select(cond_nz, context.zero, f_xz)),
                    (None, None) => None,
                };

                Some((payload, mask_xz))
            }
            ProtoExpression::DynamicVariable {
                base_offset,
                stride,
                is_ff,
                index_expr,
                num_elements,
                select,
                width,
                ..
            } => {
                if *width > 64 {
                    return None;
                }

                let (idx_payload, _idx_mask_xz) = index_expr.build_binary(context, builder)?;

                // Clamp index to [0, num_elements - 1]
                let max_idx = builder
                    .ins()
                    .iconst(I64, (*num_elements as i64).saturating_sub(1));
                let in_bounds = builder
                    .ins()
                    .icmp(IntCC::UnsignedLessThan, idx_payload, max_idx);
                let clamped = builder.ins().select(in_bounds, idx_payload, max_idx);

                // Compute byte offset: clamped * stride
                let stride_val = builder.ins().iconst(I64, *stride as i64);
                let byte_offset = builder.ins().imul(clamped, stride_val);

                // Compute address: base_addr + base_offset + value_u64_offset() + byte_offset
                let base_addr = if *is_ff {
                    context.ff_values
                } else {
                    context.comb_values
                };
                let static_offset = builder
                    .ins()
                    .iconst(I64, (*base_offset + value_u64_offset()) as i64);
                let addr = builder.ins().iadd(base_addr, static_offset);
                let addr = builder.ins().iadd(addr, byte_offset);

                let load_mem_flag = MemFlags::trusted().with_readonly();

                let mut payload = builder.ins().load(I64, load_mem_flag, addr, 0);
                let mut mask_xz = if context.use_4state {
                    let mask_xz = builder.ins().load(I64, load_mem_flag, addr, 8);
                    Some(mask_xz)
                } else {
                    None
                };

                if let Some((beg, end)) = select {
                    let mask = ValueU64::gen_mask(beg - end + 1);

                    payload = builder.ins().ushr_imm(payload, *end as i64);
                    payload = builder.ins().band_imm(payload, mask as i64);

                    if context.use_4state {
                        let x = builder.ins().ushr_imm(mask_xz.unwrap(), *end as i64);
                        let x = builder.ins().band_imm(x, mask as i64);
                        mask_xz = Some(x);
                    }
                }

                Some((payload, mask_xz))
            }
        }
    }
}

fn expand_sign(
    dst_width: usize,
    src_width: usize,
    mut payload: CraneliftValue,
    mut mask_xz: Option<CraneliftValue>,
    builder: &mut FunctionBuilder,
) -> (CraneliftValue, Option<CraneliftValue>) {
    if dst_width != src_width {
        let mask = ValueU64::gen_mask(dst_width) ^ ValueU64::gen_mask(src_width);
        let msb = builder.ins().ushr_imm(payload, (src_width - 1) as i64);
        let ext = builder.ins().bor_imm(payload, mask as i64);
        payload = builder.ins().select(msb, ext, payload);
        if let Some(x) = mask_xz {
            let msb_xz = builder.ins().ushr_imm(x, (src_width - 1) as i64);
            let ext_xz = builder.ins().bor_imm(x, mask as i64);
            mask_xz = Some(builder.ins().select(msb_xz, ext_xz, x));
        }
    }
    (payload, mask_xz)
}

fn shift_mask_xz(
    op: &Op,
    signed: bool,
    x_width: usize,
    mask_xz: CraneliftValue,
    y_payload: CraneliftValue,
    builder: &mut FunctionBuilder,
) -> CraneliftValue {
    match op {
        Op::LogicShiftL | Op::ArithShiftL => builder.ins().ishl(mask_xz, y_payload),
        Op::LogicShiftR => builder.ins().ushr(mask_xz, y_payload),
        Op::ArithShiftR => {
            if signed {
                // Sign-extend mask_xz MSB, then sshr (same pattern as payload)
                let shl_amount = (64 - x_width) as i64;
                let shifted_up = builder.ins().ishl_imm(mask_xz, shl_amount);
                let sign_extended = builder.ins().sshr_imm(shifted_up, shl_amount);
                builder.ins().sshr(sign_extended, y_payload)
            } else {
                builder.ins().ushr(mask_xz, y_payload)
            }
        }
        _ => unreachable!(),
    }
}

/// Build a ProtoExpression computing the linear index from a multi-dimensional VarIndex.
/// Equivalent to calc_index_expr but produces ProtoExpression directly with correct widths.
pub fn build_linear_index_expr(
    context: &mut ConvContext,
    array: &veryl_analyzer::ir::ShapeRef,
    index: &air::VarIndex,
) -> Option<ProtoExpression> {
    let index_width = 32;
    let index_expr_context = ExpressionContext {
        width: index_width,
        signed: false,
    };

    if array.is_empty() || (array.dims() == 1 && array[0] == Some(1) && index.0.is_empty()) {
        return Some(ProtoExpression::Value {
            value: Value::new(0, index_width, false),
            width: index_width,
            expr_context: index_expr_context,
        });
    }

    if index.0.len() != array.dims() {
        return None;
    }

    let mut ret: Option<ProtoExpression> = None;
    let mut base: usize = 1;

    for (i, dim_size) in array.iter().enumerate().rev() {
        let dim_size = (*dim_size)?;
        let idx_proto: ProtoExpression = Conv::conv(context, &index.0[i])?;

        let mul_expr = if base == 1 {
            idx_proto
        } else {
            let base_val = ProtoExpression::Value {
                value: Value::new(base as u64, index_width, false),
                width: index_width,
                expr_context: index_expr_context,
            };
            ProtoExpression::Binary {
                x: Box::new(idx_proto),
                op: crate::ir::Op::Mul,
                y: Box::new(base_val),
                width: index_width,
                expr_context: index_expr_context,
            }
        };

        ret = Some(if let Some(prev) = ret {
            ProtoExpression::Binary {
                x: Box::new(prev),
                op: crate::ir::Op::Add,
                y: Box::new(mul_expr),
                width: index_width,
                expr_context: index_expr_context,
            }
        } else {
            mul_expr
        });

        base *= dim_size;
    }

    ret
}

impl Conv<&air::Expression> for ProtoExpression {
    fn conv(context: &mut ConvContext, src: &air::Expression) -> Option<Self> {
        match src {
            air::Expression::Term(x) => match x.as_ref() {
                air::Factor::Variable(id, index, select, comptime) => {
                    let width = comptime.r#type.total_width()?;
                    let expr_context: ExpressionContext = (&comptime.expr_context).into();

                    // Try constant index first
                    let scope = context.scope();
                    let meta = scope.variable_meta.get(id).unwrap();
                    let select_val = if !select.is_empty() {
                        select.eval_value(&mut scope.analyzer_context, &comptime.r#type, false)
                    } else {
                        None
                    };
                    let const_index = if index.is_const() {
                        index.eval_value(&mut scope.analyzer_context)
                    } else {
                        None
                    };

                    if let Some(idx_vals) = const_index {
                        let index = meta.r#type.array.calc_index(&idx_vals)?;
                        let element = &meta.elements[index];
                        let is_ff = element.is_ff;
                        let offset = element.current_offset;

                        Some(ProtoExpression::Variable {
                            offset,
                            is_ff,
                            select: select_val,
                            width,
                            expr_context,
                        })
                    } else {
                        // Dynamic index: build linear index ProtoExpression directly
                        let array_shape = meta.r#type.array.clone();
                        let dyn_info = meta.dynamic_index_info()?;
                        let num_elements = meta.elements.len();
                        let (base_offset, _, stride, is_ff) = dyn_info;

                        let index_proto = build_linear_index_expr(context, &array_shape, index)?;

                        Some(ProtoExpression::DynamicVariable {
                            base_offset,
                            stride,
                            is_ff,
                            index_expr: Box::new(index_proto),
                            num_elements,
                            select: select_val,
                            width,
                            expr_context,
                        })
                    }
                }
                air::Factor::Value(comptime) => {
                    let value = comptime.get_value().ok().cloned()?;
                    let width = comptime.r#type.total_width()?;
                    let expr_context: ExpressionContext = (&comptime.expr_context).into();

                    Some(ProtoExpression::Value {
                        value,
                        width,
                        expr_context,
                    })
                }
                air::Factor::FunctionCall(call) => {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, call)?;
                    context.pending_statements.extend(stmts);

                    // Return a reference to the return value variable
                    let func = context
                        .scope()
                        .analyzer_context
                        .functions
                        .get(&call.id)?
                        .clone();
                    let body = if let Some(ref idx) = call.index {
                        func.get_function(idx)?
                    } else {
                        func.get_function(&[])?
                    };
                    let ret_id = body.ret?;

                    let scope = context.scope();
                    let meta = scope.variable_meta.get(&ret_id)?;
                    let element = &meta.elements[0];
                    let width = call.comptime.r#type.total_width()?;
                    let expr_context: ExpressionContext = (&call.comptime.expr_context).into();

                    Some(ProtoExpression::Variable {
                        offset: element.current_offset,
                        is_ff: element.is_ff,
                        select: None,
                        width,
                        expr_context,
                    })
                }
                _ => None,
            },
            air::Expression::Unary(op, x, comptime) => {
                let x: ProtoExpression = Conv::conv(context, x.as_ref())?;
                let width = comptime.expr_context.width;
                let expr_context: ExpressionContext = (&comptime.expr_context).into();

                Some(ProtoExpression::Unary {
                    op: *op,
                    x: Box::new(x),
                    width,
                    expr_context,
                })
            }
            air::Expression::Binary(x, op, y, comptime) => {
                let x: ProtoExpression = Conv::conv(context, x.as_ref())?;
                let y: ProtoExpression = Conv::conv(context, y.as_ref())?;
                let width = comptime.expr_context.width;
                let expr_context: ExpressionContext = (&comptime.expr_context).into();

                Some(ProtoExpression::Binary {
                    x: Box::new(x),
                    op: *op,
                    y: Box::new(y),
                    width,
                    expr_context,
                })
            }
            air::Expression::Concatenation(items, comptime) => {
                let mut elements = Vec::new();
                for (expr, rep) in items {
                    let converted: ProtoExpression = Conv::conv(context, expr)?;
                    let elem_width = converted.width();

                    let repeat = if let Some(rep) = rep {
                        let val = rep.eval_value(&mut context.scope().analyzer_context)?;
                        val.to_usize()?
                    } else {
                        1
                    };

                    elements.push((Box::new(converted), repeat, elem_width));
                }
                // Concatenation's comptime.expr_context.width is not set by apply_context,
                // so we must use comptime.r#type.total_width() instead.
                let width = comptime.r#type.total_width()?;
                let expr_context = ExpressionContext {
                    width,
                    signed: comptime.r#type.signed,
                };

                Some(ProtoExpression::Concatenation {
                    elements,
                    width,
                    expr_context,
                })
            }
            air::Expression::Ternary(cond, true_expr, false_expr, comptime) => {
                let cond: ProtoExpression = Conv::conv(context, cond.as_ref())?;
                let true_expr: ProtoExpression = Conv::conv(context, true_expr.as_ref())?;
                let false_expr: ProtoExpression = Conv::conv(context, false_expr.as_ref())?;
                let width = comptime.expr_context.width;
                let expr_context: ExpressionContext = (&comptime.expr_context).into();

                Some(ProtoExpression::Ternary {
                    cond: Box::new(cond),
                    true_expr: Box::new(true_expr),
                    false_expr: Box::new(false_expr),
                    width,
                    expr_context,
                })
            }
            air::Expression::StructConstructor(r#type, members, _comptime) => {
                let struct_members = match &r#type.kind {
                    air::TypeKind::Struct(s) => &s.members,
                    _ => return None,
                };

                let mut elements = Vec::new();
                for ((_name, expr), member_type) in members.iter().zip(struct_members.iter()) {
                    let converted: ProtoExpression = Conv::conv(context, expr)?;
                    let elem_width = member_type.width()?;
                    elements.push((Box::new(converted), 1, elem_width));
                }

                let width = r#type.total_width()?;
                let expr_context = ExpressionContext {
                    width,
                    signed: false,
                };

                Some(ProtoExpression::Concatenation {
                    elements,
                    width,
                    expr_context,
                })
            }
            _ => None,
        }
    }
}
