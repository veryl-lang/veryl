use crate::cranelift::Context as CraneliftContext;
use crate::ir::context::{Context as ConvContext, Conv};
use crate::ir::variable::{native_bytes as calc_native_bytes, read_native_value};
use crate::ir::{Op, ProtoStatement, Value};
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::{I32, I64, I128};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::ir as air;
use veryl_analyzer::value::{MaskCache, ValueU64};

/// Build an I128 constant from a u128 value.
/// Since `iconst` only accepts Imm64, we build I128 via `iconcat(lo, hi)`.
pub(crate) fn iconst_128(builder: &mut FunctionBuilder, val: u128) -> CraneliftValue {
    let lo = builder.ins().iconst(I64, val as u64 as i64);
    let hi = builder.ins().iconst(I64, (val >> 64) as u64 as i64);
    builder.ins().iconcat(lo, hi)
}

/// Generate a bitmask for the given width as u128.
pub(crate) fn gen_mask_128(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    }
}

/// Generate a bitmask for a bit range [beg:end] as u128.
pub(crate) fn gen_mask_range_128(beg: usize, end: usize) -> u128 {
    gen_mask_128(beg - end + 1) << end
}

/// Apply a 128-bit bitmask to a value.
fn apply_mask_128(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    mask: u128,
) -> CraneliftValue {
    let mask_val = iconst_128(builder, mask);
    builder.ins().band(val, mask_val)
}

/// Create a zero constant of the appropriate type.
fn zero_for_width(
    context: &CraneliftContext,
    _builder: &mut FunctionBuilder,
    width: usize,
) -> CraneliftValue {
    if width > 64 {
        context.zero_128
    } else {
        context.zero
    }
}

/// bxor with a constant. Uses bxor_imm for I64, explicit const for I128.
fn bxor_const(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    imm: u128,
    wide: bool,
) -> CraneliftValue {
    if wide {
        let c = iconst_128(builder, imm);
        builder.ins().bxor(val, c)
    } else {
        builder.ins().bxor_imm(val, imm as i64)
    }
}

/// band with a constant. Uses band_imm for I64, explicit const for I128.
pub(crate) fn band_const(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    imm: u128,
    wide: bool,
) -> CraneliftValue {
    if wide {
        let c = iconst_128(builder, imm);
        builder.ins().band(val, c)
    } else {
        builder.ins().band_imm(val, imm as i64)
    }
}

/// bor with a constant. Uses bor_imm for I64, explicit const for I128.
fn bor_const(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    imm: u128,
    wide: bool,
) -> CraneliftValue {
    if wide {
        let c = iconst_128(builder, imm);
        builder.ins().bor(val, c)
    } else {
        builder.ins().bor_imm(val, imm as i64)
    }
}

/// iadd with a small constant. Uses iadd_imm for I64, explicit const for I128.
fn iadd_const(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    imm: i64,
    wide: bool,
) -> CraneliftValue {
    if wide {
        let c = iconst_128(builder, imm as u128);
        builder.ins().iadd(val, c)
    } else {
        builder.ins().iadd_imm(val, imm)
    }
}

/// icmp with a constant. Uses icmp_imm for I64, explicit const for I128.
fn icmp_const(
    builder: &mut FunctionBuilder,
    cc: IntCC,
    val: CraneliftValue,
    imm: u128,
    wide: bool,
) -> CraneliftValue {
    if wide {
        let c = iconst_128(builder, imm);
        builder.ins().icmp(cc, val, c)
    } else {
        builder.ins().icmp_imm(cc, val, imm as i64)
    }
}

/// Create a constant of the correct width.
pub(crate) fn iconst_for_width(
    builder: &mut FunctionBuilder,
    val: u128,
    wide: bool,
) -> CraneliftValue {
    if wide {
        iconst_128(builder, val)
    } else {
        builder.ins().iconst(I64, val as i64)
    }
}

/// Generate a mask for the given width, returning a u128.
pub(crate) fn gen_mask_for_width(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else if width == 0 {
        0
    } else {
        (1u128 << width) - 1
    }
}

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
        value: *const u8,
        native_bytes: usize,
        use_4state: bool,
        select: Option<(usize, usize)>,
        width: usize,
        signed: bool,
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
        base_ptr: *const u8,
        native_bytes: usize,
        use_4state: bool,
        stride: isize,
        index_expr: Box<Expression>,
        num_elements: usize,
        select: Option<(usize, usize)>,
        width: usize,
        signed: bool,
    },
}

impl Expression {
    pub fn eval(&self, mask_cache: &mut MaskCache) -> Value {
        match self {
            Expression::Variable {
                value,
                native_bytes,
                use_4state,
                select,
                width,
                signed,
            } => {
                let val = unsafe {
                    read_native_value(*value, *native_bytes, *use_4state, *width as u32, *signed)
                };
                if let Some((beg, end)) = select {
                    val.select(*beg, *end)
                } else {
                    val
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
                native_bytes,
                use_4state,
                stride,
                index_expr,
                num_elements,
                select,
                width,
                signed,
            } => {
                let idx_val = index_expr.eval(mask_cache);
                let idx = idx_val.to_usize().unwrap_or(0).min(*num_elements - 1);
                let ptr = unsafe { (*base_ptr).offset(*stride * idx as isize) };
                let value = unsafe {
                    read_native_value(ptr, *native_bytes, *use_4state, *width as u32, *signed)
                };
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
    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>, outputs: &mut Vec<*const u8>) {
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
                    let ptr = unsafe { (*base_ptr).offset(*stride * i as isize) };
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
            ProtoExpression::Variable { width, .. } => *width <= 128,
            ProtoExpression::Value { value, .. } => match value {
                Value::U64(_) => true,
                Value::BigUint(v) => v.width as usize <= 128,
            },
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
            ProtoExpression::Binary {
                x,
                op,
                y,
                expr_context,
                ..
            } => {
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
                    // Reject I128 div/rem (not reliably supported on all backends)
                    && !(matches!(op, Op::Div | Op::Rem) && expr_context.width > 64)
            }
            ProtoExpression::Concatenation {
                elements, width, ..
            } => *width <= 128 && elements.iter().all(|(expr, _, _)| expr.can_build_binary()),
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                width,
                ..
            } => {
                *width <= 128
                    && cond.can_build_binary()
                    && true_expr.can_build_binary()
                    && false_expr.can_build_binary()
            }
            ProtoExpression::DynamicVariable {
                width, index_expr, ..
            } => *width <= 128 && index_expr.can_build_binary(),
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

    /// Returns a guaranteed upper bound on the number of significant bits
    /// in the Cranelift value produced by build_binary().
    /// Used to skip redundant truncation masks at store time.
    /// Returns a guaranteed upper bound on the number of significant bits
    /// in the Cranelift value produced by build_binary().
    /// Used to skip redundant truncation masks at store time.
    ///
    /// Note: For Variable/DynamicVariable, `width` is the expression result width
    /// (equals select width when select is present), and build_binary guarantees
    /// at most this many significant bits via band_imm.
    pub fn effective_bits(&self) -> usize {
        match self {
            ProtoExpression::Variable { width, .. } => *width,
            ProtoExpression::Value {
                value: Value::U64(v),
                ..
            } => {
                if v.payload == 0 && v.mask_xz == 0 {
                    0
                } else {
                    (v.payload | v.mask_xz)
                        .checked_ilog2()
                        .map_or(0, |b| b as usize + 1)
                }
            }
            ProtoExpression::Value { width, .. } => *width,
            ProtoExpression::Unary { op, x, width, .. } => match op {
                Op::BitAnd
                | Op::BitNand
                | Op::BitOr
                | Op::BitNor
                | Op::LogicNot
                | Op::BitXor
                | Op::BitXnor => 1,
                Op::Add => x.effective_bits(),
                _ => *width,
            },
            ProtoExpression::Binary {
                op, x, y, width, ..
            } => match op {
                Op::Eq
                | Op::Ne
                | Op::EqWildcard
                | Op::NeWildcard
                | Op::Greater
                | Op::GreaterEq
                | Op::Less
                | Op::LessEq
                | Op::LogicAnd
                | Op::LogicOr => 1,
                Op::BitAnd => x.effective_bits().min(y.effective_bits()),
                Op::BitOr | Op::BitXor | Op::BitXnor => x.effective_bits().max(y.effective_bits()),
                _ => *width,
            },
            ProtoExpression::Concatenation { width, .. } => *width,
            ProtoExpression::Ternary {
                true_expr,
                false_expr,
                ..
            } => true_expr.effective_bits().max(false_expr.effective_bits()),
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

    /// # Safety
    /// `ff_values_ptr` and `comb_values_ptr` must point to valid buffers.
    pub unsafe fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut u8,
        comb_values_ptr: *mut u8,
        use_4state: bool,
    ) -> Expression {
        unsafe {
            match self {
                ProtoExpression::Variable {
                    offset,
                    is_ff,
                    select,
                    width,
                    expr_context,
                    ..
                } => {
                    let nb = calc_native_bytes(*width);
                    let value = if *is_ff {
                        (ff_values_ptr as *const u8).add(*offset as usize)
                    } else {
                        (comb_values_ptr as *const u8).add(*offset as usize)
                    };
                    Expression::Variable {
                        value,
                        native_bytes: nb,
                        use_4state,
                        select: *select,
                        width: *width,
                        signed: expr_context.signed,
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
                    let x = x.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
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
                    let x = x.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
                    let y = y.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
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
                            let expr =
                                expr.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
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
                    let cond = cond.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
                    let true_expr =
                        true_expr.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
                    let false_expr =
                        false_expr.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
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
                    width,
                    expr_context,
                    ..
                } => {
                    let nb = calc_native_bytes(*width);
                    let base_ptr = if *is_ff {
                        (ff_values_ptr as *const u8).offset(*base_offset)
                    } else {
                        (comb_values_ptr as *const u8).offset(*base_offset)
                    };
                    let index_expr =
                        index_expr.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
                    Expression::DynamicVariable {
                        base_ptr,
                        native_bytes: nb,
                        use_4state,
                        stride: *stride,
                        index_expr: Box::new(index_expr),
                        num_elements: *num_elements,
                        select: *select,
                        width: *width,
                        signed: expr_context.signed,
                    }
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
                if *width > 128 {
                    return None;
                }

                let nb = calc_native_bytes(*width);
                let offset = *offset as i32;
                let cache_key = (*is_ff, offset);
                let wide = *width > 64;

                // Load CSE: reuse previously loaded values for the same address
                let (mut payload, mut mask_xz) = if let Some(&(cached_payload, cached_mask_xz)) =
                    context.load_cache.get(&cache_key)
                {
                    (cached_payload, cached_mask_xz)
                } else {
                    let load_mem_flag = MemFlags::trusted().with_readonly();

                    let base_addr = if *is_ff {
                        context.ff_values
                    } else {
                        context.comb_values
                    };

                    let payload = if nb == 16 {
                        builder.ins().load(I128, load_mem_flag, base_addr, offset)
                    } else if nb == 4 {
                        let v = builder.ins().load(I32, load_mem_flag, base_addr, offset);
                        builder.ins().uextend(I64, v)
                    } else {
                        builder.ins().load(I64, load_mem_flag, base_addr, offset)
                    };
                    let mask_xz = if context.use_4state {
                        let mask_xz_offset = offset + nb as i32;
                        let mask_xz = if nb == 16 {
                            builder
                                .ins()
                                .load(I128, load_mem_flag, base_addr, mask_xz_offset)
                        } else if nb == 4 {
                            let v =
                                builder
                                    .ins()
                                    .load(I32, load_mem_flag, base_addr, mask_xz_offset);
                            builder.ins().uextend(I64, v)
                        } else {
                            builder
                                .ins()
                                .load(I64, load_mem_flag, base_addr, mask_xz_offset)
                        };
                        Some(mask_xz)
                    } else {
                        None
                    };

                    context.load_cache.insert(cache_key, (payload, mask_xz));
                    (payload, mask_xz)
                };

                if let Some((beg, end)) = select {
                    let select_width = beg - end + 1;

                    if wide {
                        let mask = gen_mask_128(select_width);
                        if *end != 0 {
                            payload = builder.ins().ushr_imm(payload, *end as i64);
                        }
                        payload = apply_mask_128(builder, payload, mask);

                        if context.use_4state {
                            let mxz = mask_xz.unwrap();
                            let mxz = if *end != 0 {
                                builder.ins().ushr_imm(mxz, *end as i64)
                            } else {
                                mxz
                            };
                            mask_xz = Some(apply_mask_128(builder, mxz, mask));
                        }
                    } else {
                        let mask = ValueU64::gen_mask(select_width);
                        if *end != 0 {
                            payload = builder.ins().ushr_imm(payload, *end as i64);
                        }
                        payload = builder.ins().band_imm(payload, mask as i64);

                        if context.use_4state {
                            let mxz = mask_xz.unwrap();
                            let mxz = if *end != 0 {
                                builder.ins().ushr_imm(mxz, *end as i64)
                            } else {
                                mxz
                            };
                            mask_xz = Some(builder.ins().band_imm(mxz, mask as i64));
                        }
                    }
                }

                Some((payload, mask_xz))
            }
            ProtoExpression::Value { value, .. } => match value {
                Value::U64(x) => {
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
                }
                Value::BigUint(x) => {
                    if x.width as usize > 128 {
                        return None;
                    }
                    let payload = x.payload_u128();
                    let payload = iconst_128(builder, payload);

                    let mask_xz = if context.use_4state {
                        let mask_xz = x.mask_xz_u128();
                        let mask_xz = iconst_128(builder, mask_xz);
                        Some(mask_xz)
                    } else {
                        None
                    };
                    Some((payload, mask_xz))
                }
            },
            ProtoExpression::Unary {
                op,
                x,
                expr_context,
                ..
            } => {
                let (mut x_payload, mut x_mask_xz) = x.build_binary(context, builder)?;

                let width = expr_context.width;
                let wide = width > 64;
                let x_wide = x.width() > 64;
                if expr_context.signed {
                    (x_payload, x_mask_xz) =
                        expand_sign(width, x.width(), x_payload, x_mask_xz, builder);
                }

                let payload = match op {
                    Op::Add => x_payload,
                    Op::Sub => {
                        let mask = gen_mask_for_width(width);
                        let x0 = bxor_const(builder, x_payload, mask, wide);
                        iadd_const(builder, x0, 1, wide)
                    }
                    Op::BitNot => {
                        let mask = gen_mask_for_width(width);
                        bxor_const(builder, x_payload, mask, wide)
                    }
                    Op::BitAnd => {
                        let mask = gen_mask_for_width(x.width());
                        let ret = icmp_const(builder, IntCC::Equal, x_payload, mask, x_wide);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitNand => {
                        let mask = gen_mask_for_width(x.width());
                        let ret = icmp_const(builder, IntCC::NotEqual, x_payload, mask, x_wide);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitOr => {
                        let ret = icmp_const(builder, IntCC::NotEqual, x_payload, 0, x_wide);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitNor | Op::LogicNot => {
                        let ret = icmp_const(builder, IntCC::Equal, x_payload, 0, x_wide);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitXor => {
                        if x_wide {
                            // I128 popcnt: split into two I64 halves, popcnt each, add
                            let (lo, hi) = builder.ins().isplit(x_payload);
                            let p_lo = builder.ins().popcnt(lo);
                            let p_hi = builder.ins().popcnt(hi);
                            let total = builder.ins().iadd(p_lo, p_hi);
                            builder.ins().urem_imm(total, 2)
                        } else {
                            let x0 = builder.ins().popcnt(x_payload);
                            builder.ins().urem_imm(x0, 2)
                        }
                    }
                    Op::BitXnor => {
                        if x_wide {
                            let (lo, hi) = builder.ins().isplit(x_payload);
                            let p_lo = builder.ins().popcnt(lo);
                            let p_hi = builder.ins().popcnt(hi);
                            let total = builder.ins().iadd(p_lo, p_hi);
                            let x1 = builder.ins().icmp_imm(IntCC::Equal, total, 0);
                            builder.ins().uextend(I64, x1)
                        } else {
                            let x0 = builder.ins().popcnt(x_payload);
                            let x1 = builder.ins().icmp_imm(IntCC::Equal, x0, 0);
                            builder.ins().uextend(I64, x1)
                        }
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
                            let mask = iconst_for_width(builder, 0xffffffff, wide);
                            let is_xz = icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, wide);
                            let z = zero_for_width(context, builder, width);

                            let payload = builder.ins().select(is_xz, z, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, z);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitNot => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = gen_mask_for_width(width);
                            let x0 = bxor_const(builder, x_mask_xz, mask, wide);
                            let payload = builder.ins().band(payload, x0);

                            Some((payload, Some(x_mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitAnd | Op::BitNand | Op::BitOr | Op::BitNor | Op::LogicNot => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = gen_mask_for_width(x.width());

                            let (is_one, is_zero, is_x) = match op {
                                Op::BitAnd => {
                                    let x0 = builder.ins().bor(x_payload, x_mask_xz);
                                    let x1 = icmp_const(builder, IntCC::NotEqual, x0, mask, x_wide);
                                    let is_zero = x1;
                                    let is_x =
                                        icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, x_wide);
                                    (None, Some(is_zero), is_x)
                                }
                                Op::BitNand => {
                                    let x0 = builder.ins().bor(x_payload, x_mask_xz);
                                    let x1 = icmp_const(builder, IntCC::NotEqual, x0, mask, x_wide);
                                    let is_one = x1;
                                    let is_x =
                                        icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, x_wide);
                                    (Some(is_one), None, is_x)
                                }
                                Op::BitOr => {
                                    let x0 = bxor_const(builder, x_mask_xz, mask, x_wide);
                                    let x1 = builder.ins().band(x_payload, x0);
                                    let x2 = icmp_const(builder, IntCC::NotEqual, x1, 0, x_wide);
                                    let is_one = x2;
                                    let is_x =
                                        icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, x_wide);
                                    (Some(is_one), None, is_x)
                                }
                                Op::BitNor | Op::LogicNot => {
                                    let x0 = bxor_const(builder, x_mask_xz, mask, x_wide);
                                    let x1 = builder.ins().band(x_payload, x0);
                                    let x2 = icmp_const(builder, IntCC::NotEqual, x1, 0, x_wide);
                                    let is_zero = x2;
                                    let is_x =
                                        icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, x_wide);
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
                            let is_xz = icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, x_wide);

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
                let wide = expr_context.width > 64;
                let x_wide = x.width() > 64;
                let y_wide = y.width() > 64;

                // Ensure operand types match: widen I64 to I128 if the other is I128
                // (for operations where both operands must be the same type)
                let needs_wide = wide || x_wide || y_wide;
                if needs_wide && !x_wide {
                    x_payload = builder.ins().uextend(I128, x_payload);
                    if let Some(xm) = x_mask_xz {
                        x_mask_xz = Some(builder.ins().uextend(I128, xm));
                    }
                }
                if needs_wide && !y_wide {
                    y_payload = builder.ins().uextend(I128, y_payload);
                    if let Some(ym) = y_mask_xz {
                        y_mask_xz = Some(builder.ins().uextend(I128, ym));
                    }
                }

                if signed {
                    let width = if matches!(
                        op,
                        Op::Div | Op::Rem | Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq
                    ) {
                        if needs_wide { 128 } else { 64 }
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
                        // I128 div/rem rejected in can_build_binary
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
                        // I128 div/rem rejected in can_build_binary
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
                            let native_bits = if needs_wide { 128 } else { 64 };
                            let shl_amount = (native_bits - x.width()) as i64;
                            let shifted_up = builder.ins().ishl_imm(x_payload, shl_amount);
                            let sign_extended = builder.ins().sshr_imm(shifted_up, shl_amount);
                            builder.ins().sshr(sign_extended, y_payload)
                        } else {
                            builder.ins().ushr(x_payload, y_payload)
                        }
                    }
                    Op::Pow => {
                        let ty = if needs_wide { I128 } else { I64 };
                        // Binary exponentiation: result=1, while exp>0 { if odd: result*=base; base*=base; exp>>=1 }
                        let loop_header = builder.create_block();
                        builder.append_block_param(loop_header, ty); // result
                        builder.append_block_param(loop_header, ty); // base
                        builder.append_block_param(loop_header, ty); // exp

                        let loop_body = builder.create_block();

                        let exit_block = builder.create_block();
                        builder.append_block_param(exit_block, ty); // final result

                        let one_val = iconst_for_width(builder, 1, needs_wide);
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
                        let exp_zero = icmp_const(builder, IntCC::Equal, exp, 0, needs_wide);
                        builder.ins().brif(
                            exp_zero,
                            exit_block,
                            &[BlockArg::Value(result)],
                            loop_body,
                            &[],
                        );

                        builder.switch_to_block(loop_body);
                        let odd = band_const(builder, exp, 1, needs_wide);
                        let is_odd = icmp_const(builder, IntCC::NotEqual, odd, 0, needs_wide);
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
                            icmp_const(builder, IntCC::NotEqual, known, 0, needs_wide)
                        } else {
                            icmp_const(builder, IntCC::NotEqual, x_payload, 0, needs_wide)
                        };
                        let y_nonzero = if let Some(ref ym) = y_mask_xz {
                            let known = builder.ins().band_not(y_payload, *ym);
                            icmp_const(builder, IntCC::NotEqual, known, 0, needs_wide)
                        } else {
                            icmp_const(builder, IntCC::NotEqual, y_payload, 0, needs_wide)
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
                                let x_is_xz =
                                    icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, needs_wide);
                                let y_is_xz =
                                    icmp_const(builder, IntCC::NotEqual, y_mask_xz, 0, needs_wide);
                                let is_xz = builder.ins().bor(x_is_xz, y_is_xz);
                                Some(is_xz)
                            }
                            (Some(x_mask_xz), None) => {
                                let is_xz =
                                    icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, needs_wide);
                                Some(is_xz)
                            }
                            (None, Some(y_mask_xz)) => {
                                let is_xz =
                                    icmp_const(builder, IntCC::NotEqual, y_mask_xz, 0, needs_wide);
                                Some(is_xz)
                            }
                            (None, None) => None,
                        };

                        if matches!(op, Op::Div | Op::Rem) && context.use_4state {
                            let zero_div =
                                icmp_const(builder, IntCC::Equal, y_payload, 0, needs_wide);
                            if let Some(x) = is_xz {
                                is_xz = Some(builder.ins().bor(x, zero_div));
                            } else {
                                is_xz = Some(zero_div);
                            }
                        }

                        if let Some(is_xz) = is_xz {
                            // Comparison ops produce 1-bit results (I64)
                            let is_cmp =
                                matches!(op, Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq);
                            let mask = if is_cmp {
                                builder.ins().iconst(I64, 1)
                            } else {
                                iconst_for_width(builder, 0xffffffff, needs_wide)
                            };

                            let z = if is_cmp {
                                context.zero
                            } else {
                                zero_for_width(context, builder, expr_context.width)
                            };
                            let payload = builder.ins().select(is_xz, z, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, z);
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
                                let x0 = builder.ins().bnot(x_mask_xz);
                                let x1 = builder.ins().band(x_payload, x0);
                                let x2 = builder.ins().bnot(y_mask_xz);
                                let x3 = builder.ins().band(y_payload, x2);
                                let x4 = builder.ins().icmp(IntCC::NotEqual, x1, x3);

                                let x5 =
                                    icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, needs_wide);
                                let x6 =
                                    icmp_const(builder, IntCC::NotEqual, y_mask_xz, 0, needs_wide);
                                let x7 = builder.ins().bor(x5, x6);
                                Some((x4, x7))
                            }
                            (Some(x_mask_xz), None) => {
                                let x0 = builder.ins().bnot(x_mask_xz);
                                let x1 = builder.ins().band(x_payload, x0);
                                let x2 = builder.ins().icmp(IntCC::NotEqual, x1, y_payload);

                                let x3 =
                                    icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, needs_wide);
                                Some((x2, x3))
                            }
                            (None, Some(y_mask_xz)) => {
                                let x0 = builder.ins().bnot(y_mask_xz);
                                let x1 = builder.ins().band(y_payload, x0);
                                let x2 = builder.ins().icmp(IntCC::NotEqual, x_payload, x1);

                                let x3 =
                                    icmp_const(builder, IntCC::NotEqual, y_mask_xz, 0, needs_wide);
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
                                let is_mismatch = icmp_const(
                                    builder,
                                    IntCC::NotEqual,
                                    definite_diff,
                                    0,
                                    needs_wide,
                                );
                                let x_in_compare = builder.ins().band(x_mask_xz, compare_mask);
                                let is_x = icmp_const(
                                    builder,
                                    IntCC::NotEqual,
                                    x_in_compare,
                                    0,
                                    needs_wide,
                                );
                                Some((is_mismatch, is_x))
                            }
                            (Some(x_mask_xz), None) => {
                                let xor_val = builder.ins().bxor(x_payload, y_payload);
                                let not_x_mask = builder.ins().bnot(x_mask_xz);
                                let definite_diff = builder.ins().band(xor_val, not_x_mask);
                                let is_mismatch = icmp_const(
                                    builder,
                                    IntCC::NotEqual,
                                    definite_diff,
                                    0,
                                    needs_wide,
                                );
                                let is_x =
                                    icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, needs_wide);
                                Some((is_mismatch, is_x))
                            }
                            (None, Some(y_mask_xz)) => {
                                let compare_mask = builder.ins().bnot(y_mask_xz);
                                let xor_val = builder.ins().bxor(x_payload, y_payload);
                                let val_diff = builder.ins().band(xor_val, compare_mask);
                                let is_mismatch =
                                    icmp_const(builder, IntCC::NotEqual, val_diff, 0, needs_wide);
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
                            Op::NeWildcard => builder.ins().select(is_mismatch, one, context.zero),
                            _ => unreachable!(),
                        };
                        let not_mismatch = builder.ins().bnot(is_mismatch);
                        let uncertain = builder.ins().band(not_mismatch, is_x);
                        let mask_xz = builder.ins().select(uncertain, one, context.zero);
                        Some((payload, Some(mask_xz)))
                    }
                    Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR => {
                        if let Some(y_mask_xz) = y_mask_xz {
                            let y_is_xz =
                                icmp_const(builder, IntCC::NotEqual, y_mask_xz, 0, needs_wide);
                            let full_mask = iconst_for_width(
                                builder,
                                gen_mask_for_width(expr_context.width),
                                needs_wide,
                            );

                            let z = zero_for_width(context, builder, expr_context.width);
                            let shifted_mask_xz = if let Some(x_mask_xz) = x_mask_xz {
                                let shifted = shift_mask_xz(
                                    op,
                                    signed,
                                    x.width(),
                                    x_mask_xz,
                                    y_payload,
                                    builder,
                                    needs_wide,
                                );
                                builder.ins().select(y_is_xz, full_mask, shifted)
                            } else {
                                builder.ins().select(y_is_xz, full_mask, z)
                            };

                            let final_payload = builder.ins().select(y_is_xz, z, payload);
                            Some((final_payload, Some(shifted_mask_xz)))
                        } else if let Some(x_mask_xz) = x_mask_xz {
                            let shifted = shift_mask_xz(
                                op,
                                signed,
                                x.width(),
                                x_mask_xz,
                                y_payload,
                                builder,
                                needs_wide,
                            );
                            Some((payload, Some(shifted)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::LogicAnd | Op::LogicOr => {
                        let is_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                let x_is_xz =
                                    icmp_const(builder, IntCC::NotEqual, x_mask_xz, 0, needs_wide);
                                let y_is_xz =
                                    icmp_const(builder, IntCC::NotEqual, y_mask_xz, 0, needs_wide);
                                Some(builder.ins().bor(x_is_xz, y_is_xz))
                            }
                            (Some(x_mask_xz), None) => Some(icmp_const(
                                builder,
                                IntCC::NotEqual,
                                x_mask_xz,
                                0,
                                needs_wide,
                            )),
                            (None, Some(y_mask_xz)) => Some(icmp_const(
                                builder,
                                IntCC::NotEqual,
                                y_mask_xz,
                                0,
                                needs_wide,
                            )),
                            (None, None) => None,
                        };
                        if let Some(is_xz) = is_xz {
                            // payload is always I64 for LogicAnd/LogicOr (result is 1-bit)
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
                if *width > 128 {
                    return None;
                }

                let wide = *width > 64;
                let z = zero_for_width(context, builder, *width);
                let mut acc_payload = z;
                let mut acc_mask_xz: Option<CraneliftValue> =
                    if context.use_4state { Some(z) } else { None };

                // Optimize: when the first element is a 1-bit repeat, use
                // ineg + ishl + bor (3 insn) instead of N * (ishl + bor) (2N insn).
                // Pattern: {sign_bit repeat N, field1, field2, ...}
                // → build lower fields, then fill upper bits via negation.
                let first_is_bit_repeat = elements
                    .first()
                    .is_some_and(|(_, repeat, ew)| *ew == 1 && *repeat > 1);

                if first_is_bit_repeat && elements.len() >= 2 {
                    let (sign_expr, sign_repeat, _) = &elements[0];
                    let (sign_payload, sign_mask_xz) = sign_expr.build_binary(context, builder)?;
                    // Widen sign bit to accumulator width if needed
                    let sign_payload = if wide && sign_expr.width() <= 64 {
                        builder.ins().uextend(I128, sign_payload)
                    } else {
                        sign_payload
                    };
                    let sign_mask_xz = sign_mask_xz.map(|v| {
                        if wide && sign_expr.width() <= 64 {
                            builder.ins().uextend(I128, v)
                        } else {
                            v
                        }
                    });

                    // Build the lower part from remaining elements
                    let mut lower_width = 0usize;
                    for (expr, repeat, elem_width) in &elements[1..] {
                        let (elem_payload, elem_mask_xz) = expr.build_binary(context, builder)?;
                        let elem_payload = if wide && expr.width() <= 64 {
                            builder.ins().uextend(I128, elem_payload)
                        } else {
                            elem_payload
                        };
                        let elem_mask_xz = elem_mask_xz.map(|v| {
                            if wide && expr.width() <= 64 {
                                builder.ins().uextend(I128, v)
                            } else {
                                v
                            }
                        });
                        let ew = *elem_width;
                        for _ in 0..*repeat {
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
                            lower_width += ew;
                        }
                    }

                    // Fill upper bits: ineg(sign) gives 0→0, 1→0xFFFF...FFFF
                    let fill = builder.ins().ineg(sign_payload);
                    let fill = builder.ins().ishl_imm(fill, lower_width as i64);
                    acc_payload = builder.ins().bor(acc_payload, fill);

                    if let Some(acc_xz) = acc_mask_xz
                        && let Some(sign_xz) = sign_mask_xz
                    {
                        let xz_fill = builder.ins().ineg(sign_xz);
                        let xz_fill = builder.ins().ishl_imm(xz_fill, lower_width as i64);
                        acc_mask_xz = Some(builder.ins().bor(acc_xz, xz_fill));
                    }

                    let _ = sign_repeat;
                } else {
                    for (expr, repeat, elem_width) in elements {
                        let (elem_payload, elem_mask_xz) = expr.build_binary(context, builder)?;
                        let elem_payload = if wide && expr.width() <= 64 {
                            builder.ins().uextend(I128, elem_payload)
                        } else {
                            elem_payload
                        };
                        let elem_mask_xz = elem_mask_xz.map(|v| {
                            if wide && expr.width() <= 64 {
                                builder.ins().uextend(I128, v)
                            } else {
                                v
                            }
                        });
                        let ew = *elem_width;

                        for _ in 0..*repeat {
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
                }

                // Mask to width
                let mask = gen_mask_for_width(*width);
                acc_payload = band_const(builder, acc_payload, mask, wide);
                if let Some(acc_xz) = acc_mask_xz {
                    acc_mask_xz = Some(band_const(builder, acc_xz, mask, wide));
                }

                Some((acc_payload, acc_mask_xz))
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                width,
                ..
            } => {
                let (cond_payload, cond_mask_xz) = cond.build_binary(context, builder)?;
                let (mut true_payload, mut true_mask_xz) =
                    true_expr.build_binary(context, builder)?;
                let (mut false_payload, mut false_mask_xz) =
                    false_expr.build_binary(context, builder)?;

                let cond_wide = cond.width() > 64;
                let result_wide = *width > 64;
                let t_wide = true_expr.width() > 64;
                let f_wide = false_expr.width() > 64;

                // Widen branches to match if needed
                if result_wide || t_wide || f_wide {
                    if !t_wide {
                        true_payload = builder.ins().uextend(I128, true_payload);
                        if let Some(v) = true_mask_xz {
                            true_mask_xz = Some(builder.ins().uextend(I128, v));
                        }
                    }
                    if !f_wide {
                        false_payload = builder.ins().uextend(I128, false_payload);
                        if let Some(v) = false_mask_xz {
                            false_mask_xz = Some(builder.ins().uextend(I128, v));
                        }
                    }
                }

                let effective_cond = if let Some(mask_xz) = cond_mask_xz {
                    builder.ins().band_not(cond_payload, mask_xz)
                } else {
                    cond_payload
                };
                let cond_nz = icmp_const(builder, IntCC::NotEqual, effective_cond, 0, cond_wide);
                let payload = builder.ins().select(cond_nz, true_payload, false_payload);

                let z = zero_for_width(context, builder, *width);
                let mask_xz = match (true_mask_xz, false_mask_xz) {
                    (Some(t_xz), Some(f_xz)) => Some(builder.ins().select(cond_nz, t_xz, f_xz)),
                    (Some(t_xz), None) => Some(builder.ins().select(cond_nz, t_xz, z)),
                    (None, Some(f_xz)) => Some(builder.ins().select(cond_nz, z, f_xz)),
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
                if *width > 128 {
                    return None;
                }

                let nb = calc_native_bytes(*width);
                let wide = *width > 64;
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

                // Compute address: base_addr + base_offset + byte_offset
                let base_addr = if *is_ff {
                    context.ff_values
                } else {
                    context.comb_values
                };
                let static_offset = builder.ins().iconst(I64, *base_offset as i64);
                let addr = builder.ins().iadd(base_addr, static_offset);
                let addr = builder.ins().iadd(addr, byte_offset);

                let load_mem_flag = MemFlags::trusted().with_readonly();

                let mut payload = if nb == 16 {
                    builder.ins().load(I128, load_mem_flag, addr, 0)
                } else if nb == 4 {
                    let v = builder.ins().load(I32, load_mem_flag, addr, 0);
                    builder.ins().uextend(I64, v)
                } else {
                    builder.ins().load(I64, load_mem_flag, addr, 0)
                };
                let mut mask_xz = if context.use_4state {
                    let mask_xz = if nb == 16 {
                        builder.ins().load(I128, load_mem_flag, addr, nb as i32)
                    } else if nb == 4 {
                        let v = builder.ins().load(I32, load_mem_flag, addr, nb as i32);
                        builder.ins().uextend(I64, v)
                    } else {
                        builder.ins().load(I64, load_mem_flag, addr, nb as i32)
                    };
                    Some(mask_xz)
                } else {
                    None
                };

                if let Some((beg, end)) = select {
                    let select_width = beg - end + 1;
                    if wide {
                        let mask = gen_mask_128(select_width);
                        payload = builder.ins().ushr_imm(payload, *end as i64);
                        payload = apply_mask_128(builder, payload, mask);
                        if context.use_4state {
                            let x = builder.ins().ushr_imm(mask_xz.unwrap(), *end as i64);
                            mask_xz = Some(apply_mask_128(builder, x, mask));
                        }
                    } else {
                        let mask = ValueU64::gen_mask(select_width);
                        payload = builder.ins().ushr_imm(payload, *end as i64);
                        payload = builder.ins().band_imm(payload, mask as i64);
                        if context.use_4state {
                            let x = builder.ins().ushr_imm(mask_xz.unwrap(), *end as i64);
                            let x = builder.ins().band_imm(x, mask as i64);
                            mask_xz = Some(x);
                        }
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
        let wide = dst_width > 64;
        let mask = gen_mask_for_width(dst_width) ^ gen_mask_for_width(src_width);
        let msb = builder.ins().ushr_imm(payload, (src_width - 1) as i64);
        let ext = bor_const(builder, payload, mask, wide);
        payload = builder.ins().select(msb, ext, payload);
        if let Some(x) = mask_xz {
            let msb_xz = builder.ins().ushr_imm(x, (src_width - 1) as i64);
            let ext_xz = bor_const(builder, x, mask, wide);
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
    wide: bool,
) -> CraneliftValue {
    match op {
        Op::LogicShiftL | Op::ArithShiftL => builder.ins().ishl(mask_xz, y_payload),
        Op::LogicShiftR => builder.ins().ushr(mask_xz, y_payload),
        Op::ArithShiftR => {
            if signed {
                let native_bits = if wide { 128 } else { 64 };
                let shl_amount = (native_bits - x_width) as i64;
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

                // Constant folding for unary operations
                if let ProtoExpression::Value {
                    value: xv @ Value::U64(_),
                    ..
                } = &x
                {
                    let mut mc = MaskCache::default();
                    let result = op.eval_value_unary(xv, width, expr_context.signed, &mut mc);
                    if matches!(&result, Value::U64(_)) {
                        return Some(ProtoExpression::Value {
                            value: result,
                            width,
                            expr_context,
                        });
                    }
                }

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

                // Constant folding: evaluate at compile time if both operands are constants
                if let (
                    ProtoExpression::Value {
                        value: xv @ Value::U64(_),
                        ..
                    },
                    ProtoExpression::Value {
                        value: yv @ Value::U64(_),
                        ..
                    },
                ) = (&x, &y)
                {
                    let mut mc = MaskCache::default();
                    let result = op.eval_value_binary(xv, yv, width, expr_context.signed, &mut mc);
                    if matches!(&result, Value::U64(_)) {
                        return Some(ProtoExpression::Value {
                            value: result,
                            width,
                            expr_context,
                        });
                    }
                }

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
