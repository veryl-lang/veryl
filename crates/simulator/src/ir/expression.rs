use crate::ir::context::{Context, Conv};
use crate::ir::variable::{
    VarOffset, native_bytes_for as calc_native_bytes_for, read_native_value,
};
use crate::ir::{Op, ProtoStatement, Value};
use crate::simulator_error::SimulatorError;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::{MaskCache, ValueU64};

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
        dynamic_select: Option<DynamicBitSelect>,
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
        dynamic_select: Option<DynamicBitSelect>,
        width: usize,
        signed: bool,
    },
}

// SAFETY: Same as Statement — see statement.rs.
unsafe impl Send for Expression {}

impl Expression {
    pub fn eval(&self, mask_cache: &mut MaskCache) -> Value {
        match self {
            Expression::Variable {
                value,
                native_bytes,
                use_4state,
                select,
                dynamic_select,
                width,
                signed,
            } => {
                // With a bit-select, `width` is the select output width (1
                // for x[63]); we must read the full variable so that
                // `val.select` can reach past `width` and so that mask_xz
                // bytes don't overlap the payload region in 4-state mode.
                let has_sel = select.is_some() || dynamic_select.is_some();
                let read_width = if has_sel {
                    (*native_bytes * 8) as u32
                } else {
                    *width as u32
                };
                let val = unsafe {
                    read_native_value(*value, *native_bytes, *use_4state, read_width, *signed)
                };
                if let Some(dyn_sel) = dynamic_select {
                    let idx = dyn_sel
                        .index_expr
                        .eval(mask_cache)
                        .to_usize()
                        .unwrap_or(0)
                        .min(dyn_sel.num_elements.saturating_sub(1));
                    let end = idx * dyn_sel.elem_width;
                    let beg = end + dyn_sel.elem_width - 1;
                    val.select(beg, end)
                } else if let Some((beg, end)) = select {
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
                dynamic_select,
                width,
                signed,
            } => {
                if *num_elements == 0 {
                    return Value::new(0, *width, *signed);
                }
                let idx_val = index_expr.eval(mask_cache);
                let idx = idx_val
                    .to_usize()
                    .unwrap_or(0)
                    .min(num_elements.saturating_sub(1));
                #[cfg(debug_assertions)]
                debug_assert!(
                    stride.checked_mul(idx as isize).is_some(),
                    "DynamicVariable: stride*idx overflow"
                );
                let ptr = unsafe { (*base_ptr).offset(*stride * idx as isize) };
                // Read the full variable when a bit-select is present (see
                // Expression::Variable above for the rationale).
                let has_sel = select.is_some() || dynamic_select.is_some();
                let read_width = if has_sel {
                    (*native_bytes * 8) as u32
                } else {
                    *width as u32
                };
                let value = unsafe {
                    read_native_value(ptr, *native_bytes, *use_4state, read_width, *signed)
                };
                if let Some(dyn_sel) = dynamic_select {
                    let idx = dyn_sel
                        .index_expr
                        .eval(mask_cache)
                        .to_usize()
                        .unwrap_or(0)
                        .min(dyn_sel.num_elements.saturating_sub(1));
                    let end = idx * dyn_sel.elem_width;
                    let beg = end + dyn_sel.elem_width - 1;
                    value.select(beg, end)
                } else if let Some((beg, end)) = select {
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
            Expression::Variable {
                value,
                dynamic_select,
                ..
            } => {
                inputs.push(*value);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.gather_variable(inputs, outputs);
                }
            }
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
                dynamic_select,
                ..
            } => {
                index_expr.gather_variable(inputs, outputs);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.gather_variable(inputs, outputs);
                }
                for i in 0..*num_elements {
                    let ptr = unsafe { (*base_ptr).offset(*stride * i as isize) };
                    inputs.push(ptr);
                }
            }
        }
    }
}

impl ProtoExpression {
    pub fn gather_variable_offsets(&self, inputs: &mut Vec<VarOffset>) {
        match self {
            ProtoExpression::Variable {
                var_offset,
                dynamic_select,
                ..
            } => {
                inputs.push(*var_offset);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets(inputs);
                }
            }
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
                index_expr,
                num_elements,
                dynamic_select,
                ..
            } => {
                index_expr.gather_variable_offsets(inputs);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets(inputs);
                }
                // Emit only the base offset to represent the entire array as a
                // single dependency unit.  Per-element expansion caused O(N²)
                // blowup in analyze_dependency for large arrays.
                inputs.push(*base_offset);
                // Also emit the last element offset so that static accesses to
                // any element of the same array create a dependency edge.
                if *num_elements > 1 {
                    let last_offset = VarOffset::new(
                        base_offset.is_ff(),
                        base_offset.raw() + *stride * (*num_elements as isize - 1),
                    );
                    inputs.push(last_offset);
                }
            }
        }
    }

    /// Same as `gather_variable_offsets` but fully expands DynamicVariable
    /// reads to every element offset. Used by the seeded-worklist schedule
    /// so diff-based dirty propagation can see writes to any element. Not
    /// used by `analyze_dependency` (which keeps the O(N²)-avoiding
    /// base+last encoding).
    pub fn gather_variable_offsets_expanded(&self, inputs: &mut Vec<VarOffset>) {
        match self {
            ProtoExpression::Variable {
                var_offset,
                dynamic_select,
                ..
            } => {
                inputs.push(*var_offset);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets_expanded(inputs);
                }
            }
            ProtoExpression::Value { .. } => (),
            ProtoExpression::Unary { x, .. } => x.gather_variable_offsets_expanded(inputs),
            ProtoExpression::Binary { x, y, .. } => {
                x.gather_variable_offsets_expanded(inputs);
                y.gather_variable_offsets_expanded(inputs);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (expr, _, _) in elements {
                    expr.gather_variable_offsets_expanded(inputs);
                }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                cond.gather_variable_offsets_expanded(inputs);
                true_expr.gather_variable_offsets_expanded(inputs);
                false_expr.gather_variable_offsets_expanded(inputs);
            }
            ProtoExpression::DynamicVariable {
                base_offset,
                stride,
                index_expr,
                num_elements,
                dynamic_select,
                ..
            } => {
                index_expr.gather_variable_offsets_expanded(inputs);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets_expanded(inputs);
                }
                for i in 0..*num_elements {
                    let off = VarOffset::new(
                        base_offset.is_ff(),
                        base_offset.raw() + *stride * (i as isize),
                    );
                    inputs.push(off);
                }
            }
        }
    }
}

/// Dynamic bit selection for packed arrays.
/// At runtime: end = index * elem_width, beg = end + elem_width - 1
#[derive(Clone, Debug)]
pub struct ProtoDynamicBitSelect {
    pub index_expr: Box<ProtoExpression>,
    pub elem_width: usize,
    pub num_elements: usize,
}

/// Runtime dynamic bit selection (after apply_values_ptr).
#[derive(Clone, Debug)]
pub struct DynamicBitSelect {
    pub index_expr: Box<Expression>,
    pub elem_width: usize,
    pub num_elements: usize,
}

#[derive(Clone, Debug)]
pub enum ProtoExpression {
    Variable {
        var_offset: VarOffset,
        select: Option<(usize, usize)>,
        dynamic_select: Option<ProtoDynamicBitSelect>,
        width: usize,
        /// Full bit width of the underlying variable, independent of
        /// `select`. Bit-select reads must size `native_bytes` by this
        /// width so the mask_xz section (at `ptr + nb` in 4-state storage)
        /// doesn't overlap the payload region when the select fits in
        /// fewer bytes than the variable.
        var_full_width: usize,
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
        base_offset: VarOffset,
        stride: isize,
        /// Array element's native byte width (`VariableMeta::native_bytes`).
        /// `width` is the post-select width; using it would under-size `nb`
        /// and mis-position the 4-state mask_xz half-read.
        element_native_bytes: usize,
        index_expr: Box<ProtoExpression>,
        num_elements: usize,
        select: Option<(usize, usize)>,
        dynamic_select: Option<ProtoDynamicBitSelect>,
        width: usize,
        expr_context: ExpressionContext,
    },
}

impl ProtoExpression {
    /// Adjust all embedded byte offsets by the given deltas.
    /// FF offsets are shifted by `ff_delta`, comb offsets by `comb_delta`.
    pub fn adjust_offsets(&mut self, ff_delta: isize, comb_delta: isize) {
        match self {
            ProtoExpression::Variable {
                var_offset,
                dynamic_select,
                ..
            } => {
                *var_offset = var_offset.adjust(ff_delta, comb_delta);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.adjust_offsets(ff_delta, comb_delta);
                }
            }
            ProtoExpression::DynamicVariable {
                base_offset,
                index_expr,
                dynamic_select,
                ..
            } => {
                *base_offset = base_offset.adjust(ff_delta, comb_delta);
                index_expr.adjust_offsets(ff_delta, comb_delta);
                if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.index_expr.adjust_offsets(ff_delta, comb_delta);
                }
            }
            ProtoExpression::Unary { x, .. } => {
                x.adjust_offsets(ff_delta, comb_delta);
            }
            ProtoExpression::Binary { x, y, .. } => {
                x.adjust_offsets(ff_delta, comb_delta);
                y.adjust_offsets(ff_delta, comb_delta);
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                cond.adjust_offsets(ff_delta, comb_delta);
                true_expr.adjust_offsets(ff_delta, comb_delta);
                false_expr.adjust_offsets(ff_delta, comb_delta);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (expr, _, _) in elements {
                    expr.adjust_offsets(ff_delta, comb_delta);
                }
            }
            ProtoExpression::Value { .. } => {}
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

    /// Sound predicate for "build_binary's payload has at most
    /// `target_width` significant bits".  Stricter than `effective_bits()`,
    /// which is unsound for Add/Sub/Mul/Shl due to carry-out / overflow.
    ///
    /// The writer-side `band_const(payload, gen_mask_for_width(dst_width))`
    /// in ProtoAssignStatement::build_binary's narrow-store arm can be
    /// elided when this returns true.
    ///
    /// Conservative: returns false unless the bound is provable from the
    /// expression structure (Variable read with appropriate width, masked
    /// shift/select, comparison/reduction yielding 0/1, BitAnd/BitOr/...
    /// over already-clean operands, masked Concatenation, ternary of
    /// clean branches).  Add/Sub/Mul/Shl/negation/ArithShiftR fall to the
    /// false branch.
    pub fn is_clean_to_width(&self, target_width: usize) -> bool {
        match self {
            ProtoExpression::Variable {
                width,
                select,
                dynamic_select,
                ..
            } => {
                let result_width = if let Some(d) = dynamic_select {
                    d.elem_width
                } else if let Some((beg, end)) = select {
                    beg - end + 1
                } else {
                    *width
                };
                result_width <= target_width
            }
            ProtoExpression::Value { value, width, .. } => {
                if let Value::U64(v) = value {
                    if target_width >= 64 {
                        true
                    } else {
                        (v.payload | v.mask_xz) >> target_width == 0
                    }
                } else {
                    *width <= target_width
                }
            }
            ProtoExpression::Concatenation { width, .. } => *width <= target_width,
            ProtoExpression::DynamicVariable {
                width,
                select,
                dynamic_select,
                ..
            } => {
                let result_width = if let Some(d) = dynamic_select {
                    d.elem_width
                } else if let Some((beg, end)) = select {
                    beg - end + 1
                } else {
                    *width
                };
                result_width <= target_width
            }
            ProtoExpression::Unary { op, x, width, .. } => match op {
                Op::BitAnd
                | Op::BitNand
                | Op::BitOr
                | Op::BitNor
                | Op::LogicNot
                | Op::BitXor
                | Op::BitXnor => target_width >= 1,
                Op::BitNot => *width <= target_width,
                Op::Add => x.is_clean_to_width(target_width),
                _ => false,
            },
            ProtoExpression::Binary { op, x, y, .. } => match op {
                Op::Eq
                | Op::Ne
                | Op::EqWildcard
                | Op::NeWildcard
                | Op::Greater
                | Op::GreaterEq
                | Op::Less
                | Op::LessEq
                | Op::LogicAnd
                | Op::LogicOr => target_width >= 1,
                Op::BitAnd => {
                    x.is_clean_to_width(target_width) || y.is_clean_to_width(target_width)
                }
                Op::BitOr | Op::BitXor | Op::BitXnor => {
                    x.is_clean_to_width(target_width) && y.is_clean_to_width(target_width)
                }
                Op::LogicShiftR => x.is_clean_to_width(target_width),
                _ => false,
            },
            ProtoExpression::Ternary {
                true_expr,
                false_expr,
                ..
            } => {
                true_expr.is_clean_to_width(target_width)
                    && false_expr.is_clean_to_width(target_width)
            }
        }
    }

    /// # Safety
    /// `ff_values_ptr` and `comb_values_ptr` must point to valid buffers.
    // `ff_len` / `comb_len` are used by the debug_assert! bounds checks
    // below; release builds compile those out, which tricks clippy into
    // thinking the parameters are only passed through to recursive calls.
    #[allow(clippy::only_used_in_recursion)]
    pub unsafe fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut u8,
        ff_len: usize,
        comb_values_ptr: *mut u8,
        comb_len: usize,
        use_4state: bool,
    ) -> Expression {
        unsafe {
            match self {
                ProtoExpression::Variable {
                    var_offset,
                    select,
                    dynamic_select,
                    width,
                    var_full_width,
                    expr_context,
                } => {
                    // Size `nb` from the variable's full width so a bit-
                    // select doesn't under-size the read (see the doc
                    // comment on `var_full_width` above).
                    let read_width = if let Some(dyn_sel) = dynamic_select {
                        std::cmp::max(*var_full_width, dyn_sel.elem_width * dyn_sel.num_elements)
                    } else {
                        match select {
                            Some((beg, _)) => {
                                std::cmp::max(*var_full_width, std::cmp::max(*width, *beg + 1))
                            }
                            None => std::cmp::max(*var_full_width, *width),
                        }
                    };
                    let nb = calc_native_bytes_for(read_width, var_offset.is_ff());
                    let _vs = if use_4state { nb * 2 } else { nb };
                    let value = if var_offset.is_ff() {
                        #[cfg(debug_assertions)]
                        debug_assert!(
                            (var_offset.raw() as usize) + _vs <= ff_len,
                            "apply_values_ptr: ff offset {} + vs {} > ff_len {} \
                             (width={}, read_width={}, select={:?})",
                            var_offset.raw(),
                            _vs,
                            ff_len,
                            width,
                            read_width,
                            select,
                        );
                        (ff_values_ptr as *const u8).add(var_offset.raw() as usize)
                    } else {
                        #[cfg(debug_assertions)]
                        debug_assert!(
                            (var_offset.raw() as usize) + _vs <= comb_len,
                            "apply_values_ptr: comb offset {} + vs {} > comb_len {} \
                             (width={}, read_width={}, select={:?})",
                            var_offset.raw(),
                            _vs,
                            comb_len,
                            width,
                            read_width,
                            select,
                        );
                        (comb_values_ptr as *const u8).add(var_offset.raw() as usize)
                    };
                    let dynamic_select = dynamic_select.as_ref().map(|dyn_sel| DynamicBitSelect {
                        index_expr: Box::new(dyn_sel.index_expr.apply_values_ptr(
                            ff_values_ptr,
                            ff_len,
                            comb_values_ptr,
                            comb_len,
                            use_4state,
                        )),
                        elem_width: dyn_sel.elem_width,
                        num_elements: dyn_sel.num_elements,
                    });
                    Expression::Variable {
                        value,
                        native_bytes: nb,
                        use_4state,
                        select: *select,
                        dynamic_select,
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
                    let x = x.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
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
                    let x = x.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
                    let y = y.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
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
                            let expr = expr.apply_values_ptr(
                                ff_values_ptr,
                                ff_len,
                                comb_values_ptr,
                                comb_len,
                                use_4state,
                            );
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
                    let cond = cond.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
                    let true_expr = true_expr.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
                    let false_expr = false_expr.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
                    Expression::Ternary {
                        cond: Box::new(cond),
                        true_expr: Box::new(true_expr),
                        false_expr: Box::new(false_expr),
                    }
                }
                ProtoExpression::DynamicVariable {
                    base_offset,
                    stride,
                    element_native_bytes,
                    index_expr,
                    num_elements,
                    select,
                    dynamic_select,
                    width,
                    expr_context,
                } => {
                    let nb = *element_native_bytes;
                    let _vs = if use_4state { nb * 2 } else { nb };
                    let base_ptr = if base_offset.is_ff() {
                        #[cfg(debug_assertions)]
                        debug_assert!(
                            (base_offset.raw() as usize) + _vs * *num_elements <= ff_len,
                            "apply_values_ptr: DynVar ff base_offset {} + vs {} * num {} > ff_len {}",
                            base_offset.raw(),
                            _vs,
                            num_elements,
                            ff_len,
                        );
                        (ff_values_ptr as *const u8).offset(base_offset.raw())
                    } else {
                        #[cfg(debug_assertions)]
                        debug_assert!(
                            (base_offset.raw() as usize) + _vs * *num_elements <= comb_len,
                            "apply_values_ptr: DynVar comb base_offset {} + vs {} * num {} > comb_len {}",
                            base_offset.raw(),
                            _vs,
                            num_elements,
                            comb_len,
                        );
                        (comb_values_ptr as *const u8).offset(base_offset.raw())
                    };
                    let index_expr = index_expr.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
                    let dynamic_select = dynamic_select.as_ref().map(|dyn_sel| DynamicBitSelect {
                        index_expr: Box::new(dyn_sel.index_expr.apply_values_ptr(
                            ff_values_ptr,
                            ff_len,
                            comb_values_ptr,
                            comb_len,
                            use_4state,
                        )),
                        elem_width: dyn_sel.elem_width,
                        num_elements: dyn_sel.num_elements,
                    });
                    Expression::DynamicVariable {
                        base_ptr,
                        native_bytes: nb,
                        use_4state,
                        stride: *stride,
                        index_expr: Box::new(index_expr),
                        num_elements: *num_elements,
                        select: *select,
                        dynamic_select,
                        width: *width,
                        signed: expr_context.signed,
                    }
                }
            }
        }
    }
}

/// Build a ProtoExpression computing the linear index from a multi-dimensional VarIndex.
/// Equivalent to calc_index_expr but produces ProtoExpression directly with correct widths.
pub fn build_linear_index_expr(
    context: &mut Context,
    array: &veryl_analyzer::ir::ShapeRef,
    index: &air::VarIndex,
) -> Result<ProtoExpression, SimulatorError> {
    let index_width = 32;
    let index_expr_context = ExpressionContext {
        width: index_width,
        signed: false,
    };

    if array.is_empty() || (array.dims() == 1 && array[0] == Some(1) && index.0.is_empty()) {
        return Ok(ProtoExpression::Value {
            value: Value::new(0, index_width, false),
            width: index_width,
            expr_context: index_expr_context,
        });
    }

    assert_eq!(
        index.0.len(),
        array.dims(),
        "index dimension mismatch: {} != {}",
        index.0.len(),
        array.dims()
    );

    let mut ret: Option<ProtoExpression> = None;
    let mut base: usize = 1;

    for (i, dim_size) in array.iter().enumerate().rev() {
        let dim_size = dim_size.expect("array dimension size must be known");
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
                op: Op::Mul,
                y: Box::new(base_val),
                width: index_width,
                expr_context: index_expr_context,
            }
        };

        ret = Some(if let Some(prev) = ret {
            ProtoExpression::Binary {
                x: Box::new(prev),
                op: Op::Add,
                y: Box::new(mul_expr),
                width: index_width,
                expr_context: index_expr_context,
            }
        } else {
            mul_expr
        });

        base *= dim_size;
    }

    Ok(ret.expect("non-empty array must produce index expression"))
}

/// Build a ProtoDynamicBitSelect from a VarSelect that contains variable expressions.
pub fn build_dynamic_bit_select(
    context: &mut Context,
    width_shape: &veryl_analyzer::ir::ShapeRef,
    select: &air::VarSelect,
    kind_width: usize,
) -> Result<ProtoDynamicBitSelect, SimulatorError> {
    let select_dims = select.dimension();

    // Consumed = first select_dims dims (outermost), remaining = the rest (innermost).
    // elem_width = product of remaining dims * kind_width.
    // num_elements = product of consumed dims.
    let mut elem_width = kind_width;
    for i in select_dims..width_shape.dims() {
        if let Some(Some(d)) = width_shape.get(i) {
            elem_width *= d;
        }
    }

    let mut num_elements = 1;
    for i in 0..select_dims {
        if let Some(Some(d)) = width_shape.get(i) {
            num_elements *= d;
        }
    }

    let index_width = 32;
    let index_expr_context = ExpressionContext {
        width: index_width,
        signed: false,
    };

    let mut ret: Option<ProtoExpression> = None;
    let mut base: usize = 1;

    let consumed_dims: Vec<usize> = (0..select_dims)
        .map(|i| width_shape.get(i).unwrap().unwrap())
        .collect();

    for (i, &dim_size) in consumed_dims.iter().enumerate().rev() {
        let idx_proto: ProtoExpression = Conv::conv(context, &select.0[i])?;

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
                op: Op::Mul,
                y: Box::new(base_val),
                width: index_width,
                expr_context: index_expr_context,
            }
        };

        ret = Some(if let Some(prev) = ret {
            ProtoExpression::Binary {
                x: Box::new(prev),
                op: Op::Add,
                y: Box::new(mul_expr),
                width: index_width,
                expr_context: index_expr_context,
            }
        } else {
            mul_expr
        });

        base *= dim_size;
    }

    let index_expr = ret.unwrap_or(ProtoExpression::Value {
        value: Value::new(0, index_width, false),
        width: index_width,
        expr_context: index_expr_context,
    });

    Ok(ProtoDynamicBitSelect {
        index_expr: Box::new(index_expr),
        elem_width,
        num_elements,
    })
}

impl Conv<&air::Expression> for ProtoExpression {
    fn conv(context: &mut Context, src: &air::Expression) -> Result<Self, SimulatorError> {
        match src {
            air::Expression::Term(x) => match x.as_ref() {
                air::Factor::Variable(id, index, select, comptime) => {
                    let width = comptime.r#type.total_width().unwrap();
                    let expr_context: ExpressionContext = (&comptime.expr_context).into();

                    // Try constant index first
                    let (select_val, const_index, need_dynamic_select, width_shape, kind_width) = {
                        let scope = context.scope();
                        let meta = scope.variable_meta.get(id).unwrap();
                        let select_val = if !select.is_empty() {
                            // Use the variable's original type (from meta) rather
                            // than comptime.r#type, which may have had its width
                            // dimensions drained by gather_context/eval_comptime.
                            select.eval_value(&mut scope.analyzer_context, &meta.r#type, false)
                        } else {
                            None
                        };
                        let const_index = if index.is_const() {
                            index.eval_value(&mut scope.analyzer_context)
                        } else {
                            None
                        };
                        let need_dynamic = !select.is_empty() && !select.is_const();
                        let select_val = if need_dynamic { None } else { select_val };
                        let width_shape = meta.r#type.width().clone();
                        let kind_width = meta.r#type.kind.width().unwrap_or(1);
                        (
                            select_val,
                            const_index,
                            need_dynamic,
                            width_shape,
                            kind_width,
                        )
                    };
                    let dynamic_select = if need_dynamic_select {
                        Some(build_dynamic_bit_select(
                            context,
                            &width_shape,
                            select,
                            kind_width,
                        )?)
                    } else {
                        None
                    };

                    if let Some(idx_vals) = const_index {
                        let scope = context.scope();
                        let meta = scope.variable_meta.get(id).unwrap();
                        let index = meta.r#type.array.calc_index(&idx_vals).unwrap();
                        let element = &meta.elements[index];
                        let var_full_width = kind_width
                            * width_shape
                                .iter()
                                .map(|d| d.unwrap_or(1))
                                .product::<usize>();

                        Ok(ProtoExpression::Variable {
                            var_offset: element.current,
                            select: select_val,
                            dynamic_select,
                            width,
                            var_full_width,
                            expr_context,
                        })
                    } else {
                        // Dynamic index: build linear index ProtoExpression directly
                        let (array_shape, num_elements, base_offset, stride, is_ff, element_nb) = {
                            let scope = context.scope();
                            let meta = scope.variable_meta.get(id).unwrap();
                            let dyn_info = meta.dynamic_index_info().unwrap();
                            (
                                meta.r#type.array.clone(),
                                meta.elements.len(),
                                dyn_info.0,
                                dyn_info.2,
                                dyn_info.3,
                                meta.native_bytes,
                            )
                        };

                        let index_proto = build_linear_index_expr(context, &array_shape, index)?;

                        Ok(ProtoExpression::DynamicVariable {
                            base_offset: VarOffset::new(is_ff, base_offset),
                            stride,
                            element_native_bytes: element_nb,
                            index_expr: Box::new(index_proto),
                            num_elements,
                            select: select_val,
                            dynamic_select,
                            width,
                            expr_context,
                        })
                    }
                }
                air::Factor::Value(comptime) => {
                    let value = comptime
                        .get_value()
                        .map_err(|_| SimulatorError::unresolved_expression(&comptime.token))?
                        .clone();
                    let width = comptime
                        .r#type
                        .total_width()
                        .ok_or_else(|| SimulatorError::unresolved_expression(&comptime.token))?;
                    let expr_context: ExpressionContext = (&comptime.expr_context).into();

                    Ok(ProtoExpression::Value {
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
                        .get(&call.id)
                        .unwrap()
                        .clone();
                    let body = if let Some(ref idx) = call.index {
                        func.get_function(idx).unwrap()
                    } else {
                        func.get_function(&[]).unwrap()
                    };
                    let ret_id = body.ret.unwrap();

                    let scope = context.scope();
                    let meta = scope.variable_meta.get(&ret_id).unwrap();
                    let element = &meta.elements[0];
                    let width = call.comptime.r#type.total_width().unwrap();
                    let var_full_width = width;
                    let expr_context: ExpressionContext = (&call.comptime.expr_context).into();

                    Ok(ProtoExpression::Variable {
                        var_offset: element.current,
                        select: None,
                        dynamic_select: None,
                        width,
                        var_full_width,
                        expr_context,
                    })
                }
                air::Factor::SystemFunctionCall(call) => match &call.kind {
                    air::SystemFunctionKind::Signed(input)
                    | air::SystemFunctionKind::Unsigned(input) => {
                        let mut inner: ProtoExpression = Conv::conv(context, &input.0)?;
                        // Stamp the cast's signedness onto the inner
                        // expression's expr_context so downstream
                        // consumers see the post-cast flag.
                        let signed = matches!(call.kind, air::SystemFunctionKind::Signed(_));
                        let ctx = match &mut inner {
                            ProtoExpression::Variable { expr_context, .. }
                            | ProtoExpression::Value { expr_context, .. }
                            | ProtoExpression::Unary { expr_context, .. }
                            | ProtoExpression::Binary { expr_context, .. }
                            | ProtoExpression::Concatenation { expr_context, .. }
                            | ProtoExpression::Ternary { expr_context, .. }
                            | ProtoExpression::DynamicVariable { expr_context, .. } => expr_context,
                        };
                        ctx.signed = signed;
                        Ok(inner)
                    }
                    _ => {
                        unreachable!("system function calls are resolved by the analyzer")
                    }
                },
                air::Factor::Anonymous(comptime) | air::Factor::Unknown(comptime) => {
                    Err(SimulatorError::unsupported_description(&comptime.token))
                }
            },
            air::Expression::Unary(op, x, comptime) => {
                let x_kind = x.comptime().r#type.kind.clone();
                let x: ProtoExpression = Conv::conv(context, x.as_ref())?;
                let width = comptime.expr_context.width;
                let expr_context: ExpressionContext = (&comptime.expr_context).into();

                // Float constant folding
                if x_kind.is_float()
                    && let ProtoExpression::Value { value: xv, .. } = &x
                    && let Some(result) = op.eval_float_unary(xv, &x_kind)
                {
                    return Ok(ProtoExpression::Value {
                        value: result,
                        width,
                        expr_context,
                    });
                }

                // Integer constant folding
                if let ProtoExpression::Value {
                    value: xv @ Value::U64(_),
                    ..
                } = &x
                {
                    let mut mc = MaskCache::default();
                    let result = op.eval_value_unary(xv, width, expr_context.signed, &mut mc);
                    if matches!(&result, Value::U64(_)) {
                        return Ok(ProtoExpression::Value {
                            value: result,
                            width,
                            expr_context,
                        });
                    }
                }

                Ok(ProtoExpression::Unary {
                    op: *op,
                    x: Box::new(x),
                    width,
                    expr_context,
                })
            }
            air::Expression::Binary(x, op, y, comptime) => {
                if matches!(op, Op::As) {
                    let src_kind = &x.comptime().r#type.kind;
                    let dst_kind = &comptime.r#type.kind;
                    let src_float = src_kind.is_float();
                    let dst_float = dst_kind.is_float();

                    if src_float != dst_float {
                        let proto = Conv::conv(context, x.as_ref())?;
                        let dst_width = comptime.expr_context.width;
                        if let ProtoExpression::Value { value: val, .. } = &proto {
                            let converted =
                                air::convert_cast(val.clone(), src_kind, dst_kind, dst_width);
                            let expr_context: ExpressionContext = (&comptime.expr_context).into();
                            return Ok(ProtoExpression::Value {
                                value: converted,
                                width: dst_width,
                                expr_context,
                            });
                        }
                        // Non-constant float<->int: fall through (pass unchanged for now)
                        return Ok(proto);
                    }

                    return Conv::conv(context, x.as_ref());
                }

                let x_kind = x.comptime().r#type.kind.clone();
                let y_kind = y.comptime().r#type.kind.clone();
                let x: ProtoExpression = Conv::conv(context, x.as_ref())?;
                let y: ProtoExpression = Conv::conv(context, y.as_ref())?;
                let width = comptime.expr_context.width;
                let mut expr_context: ExpressionContext = (&comptime.expr_context).into();
                if matches!(op, Op::Div | Op::Rem) {
                    // See build_binary for the merge() rationale.
                    expr_context.signed = x.expr_context().signed & y.expr_context().signed;
                }

                // Float constant folding
                if (x_kind.is_float() || y_kind.is_float())
                    && let (
                        ProtoExpression::Value { value: xv, .. },
                        ProtoExpression::Value { value: yv, .. },
                    ) = (&x, &y)
                {
                    let float_kind = if x_kind.is_float() { &x_kind } else { &y_kind };
                    let float_width = if matches!(float_kind, air::TypeKind::F32) {
                        32
                    } else {
                        64
                    };
                    let xv = if !x_kind.is_float() {
                        air::convert_cast(xv.clone(), &x_kind, float_kind, float_width)
                    } else {
                        xv.clone()
                    };
                    let yv = if !y_kind.is_float() {
                        air::convert_cast(yv.clone(), &y_kind, float_kind, float_width)
                    } else {
                        yv.clone()
                    };
                    if let Some(result) = op.eval_float_binary(&xv, &yv, float_kind) {
                        return Ok(ProtoExpression::Value {
                            value: result,
                            width,
                            expr_context,
                        });
                    }
                }

                // Integer constant folding
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
                        return Ok(ProtoExpression::Value {
                            value: result,
                            width,
                            expr_context,
                        });
                    }
                }

                // Algebraic identity folding: 0 && X = 0, 1 || X = 1, etc.
                // This eliminates dependency edges on X that would
                // otherwise feed into comb-loop analysis, e.g. a gated-off
                // expression `1'b0 && live_signal && ...` still carries a
                // dep on `live_signal` without this simplification.
                // Applies only to pure-logic ops over U64 values; preserves
                // widths and signedness.
                fn is_zero_u64(e: &ProtoExpression) -> bool {
                    matches!(
                        e,
                        ProtoExpression::Value { value: Value::U64(v), .. }
                            if v.payload == 0 && v.mask_xz == 0
                    )
                }
                fn is_nonzero_u64(e: &ProtoExpression) -> bool {
                    if let ProtoExpression::Value {
                        value: Value::U64(v),
                        ..
                    } = e
                    {
                        v.mask_xz == 0 && v.payload != 0
                    } else {
                        false
                    }
                }
                let zero_result = |w: usize, ec: ExpressionContext| -> ProtoExpression {
                    ProtoExpression::Value {
                        value: Value::U64(ValueU64::new(0, w, ec.signed)),
                        width: w,
                        expr_context: ec,
                    }
                };
                let one_result = |w: usize, ec: ExpressionContext| -> ProtoExpression {
                    ProtoExpression::Value {
                        value: Value::U64(ValueU64::new(1, w, ec.signed)),
                        width: w,
                        expr_context: ec,
                    }
                };
                match op {
                    Op::LogicAnd if is_zero_u64(&x) || is_zero_u64(&y) => {
                        return Ok(zero_result(width, expr_context));
                    }
                    Op::LogicOr if is_nonzero_u64(&x) || is_nonzero_u64(&y) => {
                        return Ok(one_result(width, expr_context));
                    }
                    Op::BitAnd if is_zero_u64(&x) || is_zero_u64(&y) => {
                        return Ok(zero_result(width, expr_context));
                    }
                    _ => {}
                }

                Ok(ProtoExpression::Binary {
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
                        let val = rep
                            .eval_value(&mut context.scope().analyzer_context)
                            .unwrap();
                        val.to_usize().unwrap()
                    } else {
                        1
                    };

                    elements.push((Box::new(converted), repeat, elem_width));
                }
                // Concatenation's comptime.expr_context.width is not set by apply_context,
                // so we must use comptime.r#type.total_width() instead.
                let width = comptime.r#type.total_width().unwrap();
                let expr_context = ExpressionContext {
                    width,
                    signed: comptime.r#type.signed,
                };

                Ok(ProtoExpression::Concatenation {
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

                Ok(ProtoExpression::Ternary {
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
                    _ => panic!("StructConstructor with non-Struct type"),
                };

                let mut elements = Vec::new();
                for ((_name, expr), member_type) in members.iter().zip(struct_members.iter()) {
                    let converted: ProtoExpression = Conv::conv(context, expr)?;
                    let elem_width = member_type.width().unwrap();
                    elements.push((Box::new(converted), 1, elem_width));
                }

                let width = r#type.total_width().unwrap();
                let expr_context = ExpressionContext {
                    width,
                    signed: false,
                };

                Ok(ProtoExpression::Concatenation {
                    elements,
                    width,
                    expr_context,
                })
            }
            _ => panic!("unhandled Expression variant"),
        }
    }
}
