#[cfg(not(target_family = "wasm"))]
use crate::cranelift::{
    Context as CraneliftContext, HelperSig, alloc_wide_slot, call_helper_ret, call_helper_void,
};
use crate::ir::context::{Context as ConvContext, Conv};
use crate::ir::variable::{VarOffset, native_bytes as calc_native_bytes, read_native_value};
use crate::ir::{Op, ProtoStatement, Value};
use crate::simulator_error::SimulatorError;
#[cfg(not(target_family = "wasm"))]
use crate::wide_ops;
#[cfg(not(target_family = "wasm"))]
use cranelift::codegen::ir::BlockArg;
#[cfg(not(target_family = "wasm"))]
use cranelift::prelude::Value as CraneliftValue;
#[cfg(not(target_family = "wasm"))]
use cranelift::prelude::types::{I32, I64, I128};
#[cfg(not(target_family = "wasm"))]
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::ir as air;
use veryl_analyzer::value::MaskCache;
#[cfg(not(target_family = "wasm"))]
use veryl_analyzer::value::ValueU64;

/// Build an I128 constant from a u128 value.
/// Since `iconst` only accepts Imm64, we build I128 via `iconcat(lo, hi)`.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn iconst_128(builder: &mut FunctionBuilder, val: u128) -> CraneliftValue {
    let lo = builder.ins().iconst(I64, val as u64 as i64);
    let hi = builder.ins().iconst(I64, (val >> 64) as u64 as i64);
    builder.ins().iconcat(lo, hi)
}

/// Generate a bitmask for the given width as u128.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn gen_mask_128(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    }
}

/// Generate a bitmask for a bit range [beg:end] as u128.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn gen_mask_range_128(beg: usize, end: usize) -> u128 {
    gen_mask_128(beg - end + 1) << end
}

/// Apply a 128-bit bitmask to a value.
#[cfg(not(target_family = "wasm"))]
fn apply_mask_128(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    mask: u128,
) -> CraneliftValue {
    let mask_val = iconst_128(builder, mask);
    builder.ins().band(val, mask_val)
}

/// Create a zero constant of the appropriate type.
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
pub(crate) fn gen_mask_for_width(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else if width == 0 {
        0
    } else {
        (1u128 << width) - 1
    }
}

/// JIT: compute clamped index and shift amount for dynamic bit select.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn build_dynamic_select_shift(
    dyn_sel: &ProtoDynamicBitSelect,
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
) -> Option<CraneliftValue> {
    let (idx_payload, _) = dyn_sel.index_expr.build_binary(context, builder)?;
    let num_elem = builder.ins().iconst(I64, dyn_sel.num_elements as i64);
    let max_idx = builder.ins().iconst(I64, (dyn_sel.num_elements - 1) as i64);
    let in_bounds = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, idx_payload, num_elem);
    let clamped = builder.ins().select(in_bounds, idx_payload, max_idx);
    let shift = builder.ins().imul_imm(clamped, dyn_sel.elem_width as i64);
    Some(shift)
}

// ── Wide (>128-bit) helper utilities ────────────────────────────────

/// Returns true if this width requires pointer-based wide representation.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn is_wide_ptr(width: usize) -> bool {
    width > 128
}

/// Allocate a wide stack slot and zero-fill it.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn alloc_wide_zero(builder: &mut FunctionBuilder, nb: usize) -> CraneliftValue {
    let ptr = alloc_wide_slot(builder, nb);
    let zero = builder.ins().iconst(I64, 0);
    for i in 0..(nb / 8) {
        builder
            .ins()
            .store(MemFlags::trusted(), zero, ptr, (i * 8) as i32);
    }
    ptr
}

/// Promote a narrow register value to a wide pointer (store into stack slot).
/// If already wide (>128 bit), returns as-is.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn ensure_wide_ptr_val(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    src_width: usize,
    dst_nb: usize,
) -> CraneliftValue {
    if is_wide_ptr(src_width) {
        return val; // already a pointer
    }
    let ptr = alloc_wide_zero(builder, dst_nb);
    // Store the value at offset 0
    builder.ins().store(MemFlags::trusted(), val, ptr, 0);
    ptr
}

/// Helper function addresses for wide operations, used with call_indirect.
///
/// Converts extern "C" function pointers to usize via a two-step cast
/// (fn → *const () → usize) to satisfy both the compiler and clippy.
#[cfg(not(target_family = "wasm"))]
pub(crate) mod wide_fn_addrs {
    use crate::wide_ops;

    macro_rules! fn_addr {
        ($f:expr) => {{
            // Two-step cast: fn item → raw pointer → usize
            let ptr = $f as *const ();
            ptr as usize
        }};
    }

    pub fn band() -> usize {
        fn_addr!(wide_ops::wide_band)
    }
    pub fn bor() -> usize {
        fn_addr!(wide_ops::wide_bor)
    }
    pub fn bxor() -> usize {
        fn_addr!(wide_ops::wide_bxor)
    }
    pub fn bxor_not() -> usize {
        fn_addr!(wide_ops::wide_bxor_not)
    }
    pub fn band_not() -> usize {
        fn_addr!(wide_ops::wide_band_not)
    }
    pub fn bnot() -> usize {
        fn_addr!(wide_ops::wide_bnot)
    }
    pub fn add() -> usize {
        fn_addr!(wide_ops::wide_add)
    }
    pub fn sub() -> usize {
        fn_addr!(wide_ops::wide_sub)
    }
    pub fn mul() -> usize {
        fn_addr!(wide_ops::wide_mul)
    }
    pub fn negate() -> usize {
        fn_addr!(wide_ops::wide_negate)
    }
    pub fn copy() -> usize {
        fn_addr!(wide_ops::wide_copy)
    }
    pub fn eq() -> usize {
        fn_addr!(wide_ops::wide_eq)
    }
    pub fn ne() -> usize {
        fn_addr!(wide_ops::wide_ne)
    }
    pub fn ucmp() -> usize {
        fn_addr!(wide_ops::wide_ucmp)
    }
    pub fn scmp() -> usize {
        fn_addr!(wide_ops::wide_scmp)
    }
    pub fn shl() -> usize {
        fn_addr!(wide_ops::wide_shl)
    }
    pub fn lshr() -> usize {
        fn_addr!(wide_ops::wide_lshr)
    }
    pub fn ashr() -> usize {
        fn_addr!(wide_ops::wide_ashr)
    }
    pub fn is_nonzero() -> usize {
        fn_addr!(wide_ops::wide_is_nonzero)
    }
    pub fn is_all_ones() -> usize {
        fn_addr!(wide_ops::wide_is_all_ones)
    }
    pub fn popcnt_parity() -> usize {
        fn_addr!(wide_ops::wide_popcnt_parity)
    }
    pub fn apply_mask() -> usize {
        fn_addr!(wide_ops::wide_apply_mask)
    }
    pub fn fill_ones() -> usize {
        fn_addr!(wide_ops::wide_fill_ones)
    }
}

/// Emit a wide bitwise binary op via helper call.
#[cfg(not(target_family = "wasm"))]
fn emit_wide_binary_op(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    func_addr: usize,
    a: CraneliftValue,
    b: CraneliftValue,
    nb: usize,
) -> CraneliftValue {
    let dst = alloc_wide_slot(builder, nb);
    let nb_val = builder.ins().iconst(I32, nb as i64);
    call_helper_void(
        context,
        builder,
        HelperSig::BinaryOp,
        func_addr,
        &[dst, a, b, nb_val],
    );
    dst
}

/// Emit a wide unary op via helper call.
#[cfg(not(target_family = "wasm"))]
fn emit_wide_unary_op(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    func_addr: usize,
    a: CraneliftValue,
    nb: usize,
) -> CraneliftValue {
    let dst = alloc_wide_slot(builder, nb);
    let nb_val = builder.ins().iconst(I32, nb as i64);
    call_helper_void(
        context,
        builder,
        HelperSig::UnaryOp,
        func_addr,
        &[dst, a, nb_val],
    );
    dst
}

/// Build a wide constant from u64 digit values, stored in a stack slot.
#[cfg(not(target_family = "wasm"))]
fn emit_wide_const(builder: &mut FunctionBuilder, digits: &[u64], nb: usize) -> CraneliftValue {
    let ptr = alloc_wide_slot(builder, nb);
    let n_words = nb / 8;
    for i in 0..n_words {
        let val = digits.get(i).copied().unwrap_or(0);
        let v = builder.ins().iconst(I64, val as i64);
        builder
            .ins()
            .store(MemFlags::trusted(), v, ptr, (i * 8) as i32);
    }
    ptr
}

/// Apply width mask to a wide buffer (clear bits >= width).
#[cfg(not(target_family = "wasm"))]
pub(crate) fn emit_wide_apply_mask(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    nb: usize,
    width: usize,
) {
    let packed = wide_ops::pack_nb_width(nb, width);
    let packed_val = builder.ins().iconst(I32, packed as i64);
    let dummy = builder.ins().iconst(I64, 0);
    call_helper_void(
        context,
        builder,
        HelperSig::UnaryOp,
        wide_fn_addrs::apply_mask(),
        &[ptr, dummy, packed_val],
    );
}

/// Check if a wide mask_xz buffer is nonzero. Returns an I8 truth value.
#[cfg(not(target_family = "wasm"))]
fn emit_wide_is_nonzero(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    nb: usize,
) -> CraneliftValue {
    let nb_val = builder.ins().iconst(I32, nb as i64);
    let result = call_helper_ret(
        context,
        builder,
        HelperSig::Reduce,
        wide_fn_addrs::is_nonzero(),
        &[ptr, nb_val],
    );
    builder.ins().icmp_imm(IntCC::NotEqual, result, 0)
}

/// Create a wide all-ones mask for the given width (ones in [0..width), zeros above).
#[cfg(not(target_family = "wasm"))]
fn emit_wide_fill_ones(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    nb: usize,
    width: usize,
) -> CraneliftValue {
    let dst = alloc_wide_slot(builder, nb);
    let packed = wide_ops::pack_nb_width(nb, width);
    let packed_val = builder.ins().iconst(I32, packed as i64);
    let dummy = builder.ins().iconst(I64, 0);
    call_helper_void(
        context,
        builder,
        HelperSig::UnaryOp,
        wide_fn_addrs::fill_ones(),
        &[dst, dummy, packed_val],
    );
    dst
}

/// Conditionally select between two wide values word-by-word.
#[cfg(not(target_family = "wasm"))]
fn emit_wide_select(
    builder: &mut FunctionBuilder,
    cond: CraneliftValue,
    true_ptr: CraneliftValue,
    false_ptr: CraneliftValue,
    nb: usize,
) -> CraneliftValue {
    let dst = alloc_wide_slot(builder, nb);
    let n_words = nb / 8;
    let flags = MemFlags::trusted();
    for i in 0..n_words {
        let off = (i * 8) as i32;
        let t = builder.ins().load(I64, flags, true_ptr, off);
        let f = builder.ins().load(I64, flags, false_ptr, off);
        let r = builder.ins().select(cond, t, f);
        builder.ins().store(flags, r, dst, off);
    }
    dst
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
                let val = unsafe {
                    read_native_value(*value, *native_bytes, *use_4state, *width as u32, *signed)
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
                let value = unsafe {
                    read_native_value(ptr, *native_bytes, *use_4state, *width as u32, *signed)
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

    #[cfg(not(target_family = "wasm"))]
    pub fn can_build_binary(&self) -> bool {
        match self {
            ProtoExpression::Variable { dynamic_select, .. } => match dynamic_select {
                Some(dyn_sel) => {
                    dyn_sel.elem_width * dyn_sel.num_elements <= 128
                        && dyn_sel.index_expr.can_build_binary()
                }
                None => true,
            },
            ProtoExpression::Value { .. } => true,
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
                    // Reject div/rem for width > 64 (not supported on all backends,
                    // and not implemented for wide values)
                    && !(matches!(op, Op::Div | Op::Rem) && expr_context.width > 64)
            }
            ProtoExpression::Concatenation { elements, .. } => {
                elements.iter().all(|(expr, _, _)| expr.can_build_binary())
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                cond.can_build_binary()
                    && true_expr.can_build_binary()
                    && false_expr.can_build_binary()
            }
            ProtoExpression::DynamicVariable {
                index_expr,
                dynamic_select,
                ..
            } => {
                let dyn_ok = match dynamic_select {
                    Some(dyn_sel) => {
                        dyn_sel.elem_width * dyn_sel.num_elements <= 128
                            && dyn_sel.index_expr.can_build_binary()
                    }
                    None => true,
                };
                dyn_ok && index_expr.can_build_binary()
            }
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
                    expr_context,
                } => {
                    // When select is present, width is the select output width
                    // (e.g., 1 for x[63]). But native_bytes must cover the FULL
                    // variable so that read_native_value reads all bytes needed
                    // for the bit-select to work correctly.
                    let read_width = if let Some(dyn_sel) = dynamic_select {
                        dyn_sel.elem_width * dyn_sel.num_elements
                    } else {
                        match select {
                            Some((beg, _)) => std::cmp::max(*width, *beg + 1),
                            None => *width,
                        }
                    };
                    let nb = calc_native_bytes(read_width);
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
                    index_expr,
                    num_elements,
                    select,
                    dynamic_select,
                    width,
                    expr_context,
                } => {
                    let nb = calc_native_bytes(*width);
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

    #[cfg(not(target_family = "wasm"))]
    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<(CraneliftValue, Option<CraneliftValue>)> {
        match self {
            ProtoExpression::Variable {
                var_offset,
                dynamic_select,
                width,
                select,
                ..
            } => {
                // Wide path: >128-bit variable → return memory pointer
                if is_wide_ptr(*width) {
                    let nb = calc_native_bytes(*width);
                    let base_addr = if var_offset.is_ff() {
                        context.ff_values
                    } else {
                        context.comb_values
                    };
                    let ptr = builder.ins().iadd_imm(base_addr, var_offset.raw() as i64);

                    // Select on >128-bit values: fall back to interpreter
                    if select.is_some() || dynamic_select.is_some() {
                        return None;
                    }

                    let mask_xz = if context.use_4state {
                        Some(builder.ins().iadd_imm(ptr, nb as i64))
                    } else {
                        None
                    };

                    return Some((ptr, mask_xz));
                }

                // native_bytes must cover the full variable for correct bit-select.
                let read_width = if let Some(dyn_sel) = dynamic_select {
                    dyn_sel.elem_width * dyn_sel.num_elements
                } else {
                    match select {
                        Some((beg, _)) => std::cmp::max(*width, *beg + 1),
                        None => *width,
                    }
                };
                let nb = calc_native_bytes(read_width);
                let offset = var_offset.raw() as i32;
                let cache_key = *var_offset;
                let wide = read_width > 64;

                // Load CSE: reuse previously loaded values for the same address
                let (mut payload, mut mask_xz) = if !context.disable_load_cache
                    && let Some(&(cached_payload, cached_mask_xz)) =
                        context.load_cache.get(&cache_key)
                {
                    (cached_payload, cached_mask_xz)
                } else {
                    let load_mem_flag = MemFlags::trusted();

                    let base_addr = if var_offset.is_ff() {
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

                if let Some(dyn_sel) = dynamic_select {
                    let shift = build_dynamic_select_shift(dyn_sel, context, builder)?;
                    let mask = gen_mask_for_width(dyn_sel.elem_width);
                    payload = builder.ins().ushr(payload, shift);
                    payload = band_const(builder, payload, mask, wide);
                    if context.use_4state {
                        let mxz = mask_xz.unwrap();
                        let mxz = builder.ins().ushr(mxz, shift);
                        mask_xz = Some(band_const(builder, mxz, mask, wide));
                    }
                } else if let Some((beg, end)) = select {
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
            ProtoExpression::Value { value, width, .. } => {
                // If expression width is >128, always return a wide pointer
                // to ensure consistency with is_wide_ptr() checks in callers.
                if is_wide_ptr(*width) {
                    let nb = calc_native_bytes(*width);
                    let (payload_digits, mask_digits): (Vec<u64>, Vec<u64>) = match value {
                        Value::U64(x) => (vec![x.payload], vec![x.mask_xz]),
                        Value::BigUint(x) => (x.payload.to_u64_digits(), x.mask_xz.to_u64_digits()),
                    };
                    let payload = emit_wide_const(builder, &payload_digits, nb);
                    let mask_xz = if context.use_4state {
                        Some(emit_wide_const(builder, &mask_digits, nb))
                    } else {
                        None
                    };
                    return Some((payload, mask_xz));
                }

                match value {
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
                        // Use expression width (not BigUint x.width) for consistency
                        // with is_wide_ptr() checks in callers.
                        if is_wide_ptr(*width) {
                            let nb = calc_native_bytes(*width);
                            let payload_digits = x.payload.to_u64_digits();
                            let payload = emit_wide_const(builder, &payload_digits, nb);

                            let mask_xz = if context.use_4state {
                                let mask_digits = x.mask_xz.to_u64_digits();
                                Some(emit_wide_const(builder, &mask_digits, nb))
                            } else {
                                None
                            };
                            return Some((payload, mask_xz));
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
                }
            }
            ProtoExpression::Unary {
                op,
                x,
                expr_context,
                ..
            } => {
                let width = expr_context.width;

                // Wide path for >128-bit unary operations
                if is_wide_ptr(width) || is_wide_ptr(x.width()) {
                    return self.build_binary_wide_unary(context, builder);
                }

                let (mut x_payload, mut x_mask_xz) = x.build_binary(context, builder)?;

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
                // Wide path for >128-bit binary operations
                let needs_wide_ptr = is_wide_ptr(expr_context.width)
                    || is_wide_ptr(x.width())
                    || is_wide_ptr(y.width());
                if needs_wide_ptr {
                    return self.build_binary_wide_binary(context, builder);
                }

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
                // Wide path for >128-bit concatenation
                if is_wide_ptr(*width) {
                    return self.build_binary_wide_concat(context, builder);
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
                // Wide path for >128-bit ternary
                if is_wide_ptr(*width)
                    || is_wide_ptr(true_expr.width())
                    || is_wide_ptr(false_expr.width())
                {
                    return self.build_binary_wide_ternary(context, builder);
                }

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
                index_expr,
                num_elements,
                select,
                dynamic_select,
                width,
                ..
            } => {
                // Wide DynamicVariable: fall back to interpreter for now
                if is_wide_ptr(*width) {
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
                let base_addr = if base_offset.is_ff() {
                    context.ff_values
                } else {
                    context.comb_values
                };
                let static_offset = builder.ins().iconst(I64, base_offset.raw() as i64);
                let addr = builder.ins().iadd(base_addr, static_offset);
                let addr = builder.ins().iadd(addr, byte_offset);

                let load_mem_flag = MemFlags::trusted();

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

                if let Some(dyn_sel) = dynamic_select {
                    let shift = build_dynamic_select_shift(dyn_sel, context, builder)?;
                    let mask = gen_mask_for_width(dyn_sel.elem_width);
                    payload = builder.ins().ushr(payload, shift);
                    payload = band_const(builder, payload, mask, wide);
                    if context.use_4state {
                        let mxz = mask_xz.unwrap();
                        let mxz = builder.ins().ushr(mxz, shift);
                        mask_xz = Some(band_const(builder, mxz, mask, wide));
                    }
                } else if let Some((beg, end)) = select {
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

    // ── Wide (>128-bit) build_binary implementations ───────────────

    #[cfg(not(target_family = "wasm"))]
    fn build_binary_wide_unary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<(CraneliftValue, Option<CraneliftValue>)> {
        let ProtoExpression::Unary {
            op,
            x,
            expr_context,
            ..
        } = self
        else {
            unreachable!()
        };

        let width = expr_context.width;
        let x_width = x.width();
        let (x_payload, x_mask_xz) = x.build_binary(context, builder)?;

        // Reduction ops always produce 1-bit I64 results
        let is_reduction = matches!(
            op,
            Op::BitAnd
                | Op::BitNand
                | Op::BitOr
                | Op::BitNor
                | Op::LogicNot
                | Op::BitXor
                | Op::BitXnor
        );

        if is_reduction {
            // Input is wide, output is I64 (1-bit)
            let x_ptr = if is_wide_ptr(x_width) {
                x_payload
            } else {
                ensure_wide_ptr_val(builder, x_payload, x_width, calc_native_bytes(x_width))
            };
            let x_nb = calc_native_bytes(x_width);
            let packed = wide_ops::pack_nb_width(x_nb, x_width);
            let nb_val = builder.ins().iconst(I32, x_nb as i64);
            let packed_val = builder.ins().iconst(I32, packed as i64);

            let payload = match op {
                Op::BitAnd => call_helper_ret(
                    context,
                    builder,
                    HelperSig::Reduce,
                    wide_fn_addrs::is_all_ones(),
                    &[x_ptr, packed_val],
                ),
                Op::BitNand => {
                    let r = call_helper_ret(
                        context,
                        builder,
                        HelperSig::Reduce,
                        wide_fn_addrs::is_all_ones(),
                        &[x_ptr, packed_val],
                    );
                    builder.ins().bxor_imm(r, 1)
                }
                Op::BitOr | Op::LogicNot => {
                    let r = call_helper_ret(
                        context,
                        builder,
                        HelperSig::Reduce,
                        wide_fn_addrs::is_nonzero(),
                        &[x_ptr, nb_val],
                    );
                    if matches!(op, Op::LogicNot | Op::BitNor) {
                        builder.ins().bxor_imm(r, 1)
                    } else {
                        r
                    }
                }
                Op::BitNor => {
                    let r = call_helper_ret(
                        context,
                        builder,
                        HelperSig::Reduce,
                        wide_fn_addrs::is_nonzero(),
                        &[x_ptr, nb_val],
                    );
                    builder.ins().bxor_imm(r, 1)
                }
                Op::BitXor => call_helper_ret(
                    context,
                    builder,
                    HelperSig::Reduce,
                    wide_fn_addrs::popcnt_parity(),
                    &[x_ptr, nb_val],
                ),
                Op::BitXnor => {
                    let r = call_helper_ret(
                        context,
                        builder,
                        HelperSig::Reduce,
                        wide_fn_addrs::popcnt_parity(),
                        &[x_ptr, nb_val],
                    );
                    builder.ins().bxor_imm(r, 1)
                }
                _ => unreachable!(),
            };

            // 4-state: if x has any X/Z bits, result is X
            if let Some(x_mask_xz) = x_mask_xz {
                let x_mask_ptr = if is_wide_ptr(x_width) {
                    x_mask_xz
                } else {
                    ensure_wide_ptr_val(builder, x_mask_xz, x_width, x_nb)
                };
                let is_xz = emit_wide_is_nonzero(context, builder, x_mask_ptr, x_nb);
                let one = builder.ins().iconst(I64, 1);
                let payload = builder.ins().select(is_xz, context.zero, payload);
                let mask_xz = builder.ins().select(is_xz, one, context.zero);
                return Some((payload, Some(mask_xz)));
            }
            return Some((payload, None));
        }

        // Non-reduction unary ops with wide result
        let nb = calc_native_bytes(width);
        let x_nb = calc_native_bytes(x_width);
        let x_ptr = if is_wide_ptr(x_width) {
            x_payload
        } else {
            ensure_wide_ptr_val(builder, x_payload, x_width, nb)
        };

        let payload = match op {
            Op::Add => {
                // Identity: just copy
                if x_nb == nb {
                    x_ptr
                } else {
                    emit_wide_unary_op(context, builder, wide_fn_addrs::copy(), x_ptr, nb)
                }
            }
            Op::Sub => {
                // Negate: ~x + 1
                let dst = emit_wide_unary_op(context, builder, wide_fn_addrs::negate(), x_ptr, nb);
                emit_wide_apply_mask(context, builder, dst, nb, width);
                dst
            }
            Op::BitNot => {
                let dst = emit_wide_unary_op(context, builder, wide_fn_addrs::bnot(), x_ptr, nb);
                emit_wide_apply_mask(context, builder, dst, nb, width);
                dst
            }
            _ => return None,
        };

        // 4-state handling for non-reduction ops
        if let Some(x_mask_xz) = x_mask_xz {
            let x_mask_ptr = if is_wide_ptr(x_width) {
                x_mask_xz
            } else {
                ensure_wide_ptr_val(builder, x_mask_xz, x_width, nb)
            };

            let mask_xz = match op {
                Op::Add => x_mask_ptr,
                Op::Sub | Op::BitNot => {
                    // If any X/Z, set result mask to all-ones for the width
                    let is_xz = emit_wide_is_nonzero(context, builder, x_mask_ptr, nb);
                    let full_mask = emit_wide_fill_ones(context, builder, nb, width);
                    let zero = alloc_wide_zero(builder, nb);
                    emit_wide_select(builder, is_xz, full_mask, zero, nb)
                }
                _ => return None,
            };
            Some((payload, Some(mask_xz)))
        } else {
            Some((payload, None))
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn build_binary_wide_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<(CraneliftValue, Option<CraneliftValue>)> {
        let ProtoExpression::Binary {
            x,
            op,
            y,
            expr_context,
            ..
        } = self
        else {
            unreachable!()
        };

        let width = expr_context.width;
        let x_width = x.width();
        let y_width = y.width();

        let (x_payload, x_mask_xz) = x.build_binary(context, builder)?;
        let (y_payload, y_mask_xz) = y.build_binary(context, builder)?;

        // Determine the native byte size for the operation
        let op_nb = calc_native_bytes(width.max(x_width).max(y_width));

        // Ensure both operands are wide pointers of the right size
        let x_ptr = if is_wide_ptr(x_width) {
            x_payload
        } else {
            ensure_wide_ptr_val(builder, x_payload, x_width, op_nb)
        };
        let y_ptr = if is_wide_ptr(y_width) {
            y_payload
        } else {
            ensure_wide_ptr_val(builder, y_payload, y_width, op_nb)
        };

        // Result is comparison (1-bit I64)?
        let is_cmp = matches!(
            op,
            Op::Eq
                | Op::Ne
                | Op::Greater
                | Op::GreaterEq
                | Op::Less
                | Op::LessEq
                | Op::LogicAnd
                | Op::LogicOr
        );

        if is_cmp {
            let nb_val = builder.ins().iconst(I32, op_nb as i64);
            let payload = match op {
                Op::Eq => call_helper_ret(
                    context,
                    builder,
                    HelperSig::Compare,
                    wide_fn_addrs::eq(),
                    &[x_ptr, y_ptr, nb_val],
                ),
                Op::Ne => call_helper_ret(
                    context,
                    builder,
                    HelperSig::Compare,
                    wide_fn_addrs::ne(),
                    &[x_ptr, y_ptr, nb_val],
                ),
                Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq => {
                    let cmp_result = if expr_context.signed {
                        let packed = wide_ops::pack_nb_width(op_nb, width);
                        let packed_val = builder.ins().iconst(I32, packed as i64);
                        call_helper_ret(
                            context,
                            builder,
                            HelperSig::Compare,
                            wide_fn_addrs::scmp(),
                            &[x_ptr, y_ptr, packed_val],
                        )
                    } else {
                        call_helper_ret(
                            context,
                            builder,
                            HelperSig::Compare,
                            wide_fn_addrs::ucmp(),
                            &[x_ptr, y_ptr, nb_val],
                        )
                    };
                    // cmp_result is -1/0/1, convert to boolean
                    match op {
                        Op::Greater => {
                            let r = builder
                                .ins()
                                .icmp_imm(IntCC::SignedGreaterThan, cmp_result, 0);
                            builder.ins().uextend(I64, r)
                        }
                        Op::GreaterEq => {
                            let r = builder.ins().icmp_imm(
                                IntCC::SignedGreaterThanOrEqual,
                                cmp_result,
                                0,
                            );
                            builder.ins().uextend(I64, r)
                        }
                        Op::Less => {
                            let r = builder.ins().icmp_imm(IntCC::SignedLessThan, cmp_result, 0);
                            builder.ins().uextend(I64, r)
                        }
                        Op::LessEq => {
                            let r =
                                builder
                                    .ins()
                                    .icmp_imm(IntCC::SignedLessThanOrEqual, cmp_result, 0);
                            builder.ins().uextend(I64, r)
                        }
                        _ => unreachable!(),
                    }
                }
                Op::LogicAnd | Op::LogicOr => {
                    let nb_val2 = builder.ins().iconst(I32, op_nb as i64);
                    let x_nz = call_helper_ret(
                        context,
                        builder,
                        HelperSig::Reduce,
                        wide_fn_addrs::is_nonzero(),
                        &[x_ptr, nb_val],
                    );
                    let y_nz = call_helper_ret(
                        context,
                        builder,
                        HelperSig::Reduce,
                        wide_fn_addrs::is_nonzero(),
                        &[y_ptr, nb_val2],
                    );
                    if matches!(op, Op::LogicAnd) {
                        builder.ins().band(x_nz, y_nz)
                    } else {
                        builder.ins().bor(x_nz, y_nz)
                    }
                }
                _ => unreachable!(),
            };

            // 4-state for comparisons: if any mask is nonzero, result is X
            let mask_xz = wide_any_xz(context, builder, x_mask_xz, y_mask_xz, x_width, y_width);
            if let Some(is_xz) = mask_xz {
                let one = builder.ins().iconst(I64, 1);
                let payload = builder.ins().select(is_xz, context.zero, payload);
                let mask_xz = builder.ins().select(is_xz, one, context.zero);
                return Some((payload, Some(mask_xz)));
            }
            return Some((payload, None));
        }

        // Non-comparison binary op with wide result
        let result_nb = calc_native_bytes(width);
        let nb_val = builder.ins().iconst(I32, op_nb as i64);

        let payload = match op {
            Op::Add => {
                emit_wide_binary_op(context, builder, wide_fn_addrs::add(), x_ptr, y_ptr, op_nb)
            }
            Op::Sub => {
                emit_wide_binary_op(context, builder, wide_fn_addrs::sub(), x_ptr, y_ptr, op_nb)
            }
            Op::Mul => {
                emit_wide_binary_op(context, builder, wide_fn_addrs::mul(), x_ptr, y_ptr, op_nb)
            }
            Op::BitAnd => {
                emit_wide_binary_op(context, builder, wide_fn_addrs::band(), x_ptr, y_ptr, op_nb)
            }
            Op::BitOr => {
                emit_wide_binary_op(context, builder, wide_fn_addrs::bor(), x_ptr, y_ptr, op_nb)
            }
            Op::BitXor => {
                emit_wide_binary_op(context, builder, wide_fn_addrs::bxor(), x_ptr, y_ptr, op_nb)
            }
            Op::BitXnor => emit_wide_binary_op(
                context,
                builder,
                wide_fn_addrs::bxor_not(),
                x_ptr,
                y_ptr,
                op_nb,
            ),
            Op::LogicShiftL | Op::ArithShiftL => {
                // Shift amount: extract from y_ptr as u64
                let amount = builder.ins().load(I64, MemFlags::trusted(), y_ptr, 0);
                let dst = alloc_wide_slot(builder, op_nb);
                call_helper_void(
                    context,
                    builder,
                    HelperSig::BinaryOp,
                    wide_fn_addrs::shl(),
                    &[dst, x_ptr, amount, nb_val],
                );
                dst
            }
            Op::LogicShiftR => {
                let amount = builder.ins().load(I64, MemFlags::trusted(), y_ptr, 0);
                let dst = alloc_wide_slot(builder, op_nb);
                call_helper_void(
                    context,
                    builder,
                    HelperSig::BinaryOp,
                    wide_fn_addrs::lshr(),
                    &[dst, x_ptr, amount, nb_val],
                );
                dst
            }
            Op::ArithShiftR => {
                let amount = builder.ins().load(I64, MemFlags::trusted(), y_ptr, 0);
                let dst = alloc_wide_slot(builder, op_nb);
                let packed = wide_ops::pack_nb_width(op_nb, x_width);
                let packed_val = builder.ins().iconst(I32, packed as i64);
                call_helper_void(
                    context,
                    builder,
                    HelperSig::BinaryOp,
                    wide_fn_addrs::ashr(),
                    &[dst, x_ptr, amount, packed_val],
                );
                dst
            }
            Op::Pow => {
                // Binary exponentiation via helper calls
                // result = 1; while exp > 0 { if exp & 1: result *= base; base *= base; exp >>= 1; }
                // For simplicity, fall back to interpreter for wide Pow
                return None;
            }
            _ => return None,
        };

        // Apply width mask
        if result_nb == op_nb {
            emit_wide_apply_mask(context, builder, payload, op_nb, width);
        }

        // 4-state handling for arithmetic/bitwise ops
        let mask_xz = self.build_wide_4state_binary_mask(
            context,
            builder,
            op,
            &WideOperandPair {
                x_mask_xz,
                y_mask_xz,
                x_ptr,
                y_ptr,
                x_width,
                y_width,
                width,
                op_nb,
            },
        );

        Some((payload, mask_xz))
    }

    /// Build 4-state mask_xz for wide binary ops.
    #[cfg(not(target_family = "wasm"))]
    fn build_wide_4state_binary_mask(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
        op: &Op,
        operands: &WideOperandPair,
    ) -> Option<CraneliftValue> {
        let WideOperandPair {
            x_mask_xz,
            y_mask_xz,
            x_ptr,
            y_ptr,
            x_width,
            y_width,
            width,
            op_nb,
        } = *operands;
        if !context.use_4state {
            return None;
        }
        let x_mask_ptr = x_mask_xz.map(|m| {
            if is_wide_ptr(x_width) {
                m
            } else {
                ensure_wide_ptr_val(builder, m, x_width, op_nb)
            }
        });
        let y_mask_ptr = y_mask_xz.map(|m| {
            if is_wide_ptr(y_width) {
                m
            } else {
                ensure_wide_ptr_val(builder, m, y_width, op_nb)
            }
        });

        match op {
            Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
                // For bitwise ops with 4-state, compute mask using helper calls
                let (x_m, y_m) = match (x_mask_ptr, y_mask_ptr) {
                    (Some(x), Some(y)) => (x, y),
                    (Some(x), None) => (x, alloc_wide_zero(builder, op_nb)),
                    (None, Some(y)) => (alloc_wide_zero(builder, op_nb), y),
                    (None, None) => return None,
                };

                let result_mask = match op {
                    Op::BitAnd => {
                        // mask = (x_m & y_m) | (x_m & ~y_m & y_p) | (y_m & ~x_m & x_p)
                        let t1 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            x_m,
                            y_m,
                            op_nb,
                        );
                        let t2 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band_not(),
                            x_m,
                            y_m,
                            op_nb,
                        );
                        let t3 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            t2,
                            y_ptr,
                            op_nb,
                        );
                        let t4 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band_not(),
                            y_m,
                            x_m,
                            op_nb,
                        );
                        let t5 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            t4,
                            x_ptr,
                            op_nb,
                        );
                        let t6 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::bor(),
                            t1,
                            t3,
                            op_nb,
                        );
                        emit_wide_binary_op(context, builder, wide_fn_addrs::bor(), t6, t5, op_nb)
                    }
                    Op::BitOr => {
                        // mask = (x_m & y_m) | (x_m & ~y_m & ~y_p) | (y_m & ~x_m & ~x_p)
                        let t1 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            x_m,
                            y_m,
                            op_nb,
                        );
                        let ny_m =
                            emit_wide_unary_op(context, builder, wide_fn_addrs::bnot(), y_m, op_nb);
                        let ny_p = emit_wide_unary_op(
                            context,
                            builder,
                            wide_fn_addrs::bnot(),
                            y_ptr,
                            op_nb,
                        );
                        let t2 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            x_m,
                            ny_m,
                            op_nb,
                        );
                        let t3 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            t2,
                            ny_p,
                            op_nb,
                        );
                        let nx_m =
                            emit_wide_unary_op(context, builder, wide_fn_addrs::bnot(), x_m, op_nb);
                        let nx_p = emit_wide_unary_op(
                            context,
                            builder,
                            wide_fn_addrs::bnot(),
                            x_ptr,
                            op_nb,
                        );
                        let t4 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            y_m,
                            nx_m,
                            op_nb,
                        );
                        let t5 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::band(),
                            t4,
                            nx_p,
                            op_nb,
                        );
                        let t6 = emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::bor(),
                            t1,
                            t3,
                            op_nb,
                        );
                        emit_wide_binary_op(context, builder, wide_fn_addrs::bor(), t6, t5, op_nb)
                    }
                    Op::BitXor | Op::BitXnor => {
                        // mask = x_m | y_m
                        emit_wide_binary_op(context, builder, wide_fn_addrs::bor(), x_m, y_m, op_nb)
                    }
                    _ => unreachable!(),
                };
                Some(result_mask)
            }
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::LogicShiftL
            | Op::LogicShiftR
            | Op::ArithShiftL
            | Op::ArithShiftR => {
                // If any operand has X/Z, result mask = all-ones for width
                let is_xz = wide_any_xz(context, builder, x_mask_xz, y_mask_xz, x_width, y_width)?;
                let full_mask = emit_wide_fill_ones(context, builder, op_nb, width);
                let zero = alloc_wide_zero(builder, op_nb);
                Some(emit_wide_select(builder, is_xz, full_mask, zero, op_nb))
            }
            _ => None,
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn build_binary_wide_concat(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<(CraneliftValue, Option<CraneliftValue>)> {
        let ProtoExpression::Concatenation {
            elements, width, ..
        } = self
        else {
            unreachable!()
        };

        let nb = calc_native_bytes(*width);
        let mut acc = alloc_wide_zero(builder, nb);
        let mut acc_xz: Option<CraneliftValue> = if context.use_4state {
            Some(alloc_wide_zero(builder, nb))
        } else {
            None
        };

        let nb_val = builder.ins().iconst(I32, nb as i64);

        for (expr, repeat, elem_width) in elements {
            let (elem_payload, elem_mask_xz) = expr.build_binary(context, builder)?;
            let ew = *elem_width;

            // Ensure element is a wide pointer
            let elem_ptr = if is_wide_ptr(expr.width()) {
                elem_payload
            } else {
                ensure_wide_ptr_val(builder, elem_payload, expr.width(), nb)
            };
            let elem_xz_ptr = elem_mask_xz.map(|m| {
                if is_wide_ptr(expr.width()) {
                    m
                } else {
                    ensure_wide_ptr_val(builder, m, expr.width(), nb)
                }
            });

            for _ in 0..*repeat {
                // acc <<= ew
                let amount = builder.ins().iconst(I64, ew as i64);
                let new_acc = alloc_wide_slot(builder, nb);
                call_helper_void(
                    context,
                    builder,
                    HelperSig::BinaryOp,
                    wide_fn_addrs::shl(),
                    &[new_acc, acc, amount, nb_val],
                );
                // acc |= elem
                let result = emit_wide_binary_op(
                    context,
                    builder,
                    wide_fn_addrs::bor(),
                    new_acc,
                    elem_ptr,
                    nb,
                );
                acc = result;

                if let Some(acc_xz_val) = acc_xz {
                    let new_xz = alloc_wide_slot(builder, nb);
                    call_helper_void(
                        context,
                        builder,
                        HelperSig::BinaryOp,
                        wide_fn_addrs::shl(),
                        &[new_xz, acc_xz_val, amount, nb_val],
                    );
                    acc_xz = if let Some(elem_xz) = elem_xz_ptr {
                        Some(emit_wide_binary_op(
                            context,
                            builder,
                            wide_fn_addrs::bor(),
                            new_xz,
                            elem_xz,
                            nb,
                        ))
                    } else {
                        Some(new_xz)
                    };
                }
            }
        }

        // Apply width mask
        emit_wide_apply_mask(context, builder, acc, nb, *width);
        if let Some(xz) = acc_xz {
            emit_wide_apply_mask(context, builder, xz, nb, *width);
        }

        Some((acc, acc_xz))
    }

    #[cfg(not(target_family = "wasm"))]
    fn build_binary_wide_ternary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<(CraneliftValue, Option<CraneliftValue>)> {
        let ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            width,
            ..
        } = self
        else {
            unreachable!()
        };

        let nb = calc_native_bytes(*width);
        let (cond_payload, cond_mask_xz) = cond.build_binary(context, builder)?;
        let (true_payload, true_mask_xz) = true_expr.build_binary(context, builder)?;
        let (false_payload, false_mask_xz) = false_expr.build_binary(context, builder)?;

        // Condition is always narrow
        let cond_wide = cond.width() > 64;
        let effective_cond = if let Some(mask_xz) = cond_mask_xz {
            builder.ins().band_not(cond_payload, mask_xz)
        } else {
            cond_payload
        };
        let cond_nz = icmp_const(builder, IntCC::NotEqual, effective_cond, 0, cond_wide);

        // Ensure both branches are wide pointers
        let true_ptr = if is_wide_ptr(true_expr.width()) {
            true_payload
        } else {
            ensure_wide_ptr_val(builder, true_payload, true_expr.width(), nb)
        };
        let false_ptr = if is_wide_ptr(false_expr.width()) {
            false_payload
        } else {
            ensure_wide_ptr_val(builder, false_payload, false_expr.width(), nb)
        };

        let payload = emit_wide_select(builder, cond_nz, true_ptr, false_ptr, nb);

        // 4-state
        let mask_xz = if context.use_4state {
            let t_xz = true_mask_xz
                .map(|m| {
                    if is_wide_ptr(true_expr.width()) {
                        m
                    } else {
                        ensure_wide_ptr_val(builder, m, true_expr.width(), nb)
                    }
                })
                .unwrap_or_else(|| alloc_wide_zero(builder, nb));
            let f_xz = false_mask_xz
                .map(|m| {
                    if is_wide_ptr(false_expr.width()) {
                        m
                    } else {
                        ensure_wide_ptr_val(builder, m, false_expr.width(), nb)
                    }
                })
                .unwrap_or_else(|| alloc_wide_zero(builder, nb));
            Some(emit_wide_select(builder, cond_nz, t_xz, f_xz, nb))
        } else {
            None
        };

        Some((payload, mask_xz))
    }
}

/// Operand info for wide 4-state mask computation.
#[cfg(not(target_family = "wasm"))]
struct WideOperandPair {
    x_mask_xz: Option<CraneliftValue>,
    y_mask_xz: Option<CraneliftValue>,
    x_ptr: CraneliftValue,
    y_ptr: CraneliftValue,
    x_width: usize,
    y_width: usize,
    width: usize,
    op_nb: usize,
}

/// Check if either wide operand has nonzero mask_xz. Returns an I8 truth value.
#[cfg(not(target_family = "wasm"))]
fn wide_any_xz(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    x_mask_xz: Option<CraneliftValue>,
    y_mask_xz: Option<CraneliftValue>,
    x_width: usize,
    y_width: usize,
) -> Option<CraneliftValue> {
    if !context.use_4state {
        return None;
    }
    let x_has_xz = x_mask_xz.map(|m| {
        if is_wide_ptr(x_width) {
            emit_wide_is_nonzero(context, builder, m, calc_native_bytes(x_width))
        } else {
            let wide = x_width > 64;
            icmp_const(builder, IntCC::NotEqual, m, 0, wide)
        }
    });
    let y_has_xz = y_mask_xz.map(|m| {
        if is_wide_ptr(y_width) {
            emit_wide_is_nonzero(context, builder, m, calc_native_bytes(y_width))
        } else {
            let wide = y_width > 64;
            icmp_const(builder, IntCC::NotEqual, m, 0, wide)
        }
    });

    match (x_has_xz, y_has_xz) {
        (Some(x), Some(y)) => Some(builder.ins().bor(x, y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

#[cfg(not(target_family = "wasm"))]
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

#[cfg(not(target_family = "wasm"))]
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

    Ok(ret.expect("non-empty array must produce index expression"))
}

/// Build a ProtoDynamicBitSelect from a VarSelect that contains variable expressions.
pub fn build_dynamic_bit_select(
    context: &mut ConvContext,
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
    fn conv(context: &mut ConvContext, src: &air::Expression) -> Result<Self, SimulatorError> {
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
                            select.eval_value(&mut scope.analyzer_context, &comptime.r#type, false)
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
                        let width_shape = meta.r#type.width.clone();
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

                        Ok(ProtoExpression::Variable {
                            var_offset: element.current,
                            select: select_val,
                            dynamic_select,
                            width,
                            expr_context,
                        })
                    } else {
                        // Dynamic index: build linear index ProtoExpression directly
                        let scope = context.scope();
                        let meta = scope.variable_meta.get(id).unwrap();
                        let array_shape = meta.r#type.array.clone();
                        let dyn_info = meta.dynamic_index_info().unwrap();
                        let num_elements = meta.elements.len();
                        let (base_offset, _, stride, is_ff) = dyn_info;

                        let index_proto = build_linear_index_expr(context, &array_shape, index)?;

                        Ok(ProtoExpression::DynamicVariable {
                            base_offset: VarOffset::new(is_ff, base_offset),
                            stride,
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
                    let expr_context: ExpressionContext = (&call.comptime.expr_context).into();

                    Ok(ProtoExpression::Variable {
                        var_offset: element.current,
                        select: None,
                        dynamic_select: None,
                        width,
                        expr_context,
                    })
                }
                air::Factor::SystemFunctionCall(call) => match &call.kind {
                    air::SystemFunctionKind::Signed(input)
                    | air::SystemFunctionKind::Unsigned(input) => Conv::conv(context, &input.0),
                    _ => {
                        unreachable!("system function calls are resolved by the analyzer")
                    }
                },
                air::Factor::Anonymous(comptime) | air::Factor::Unknown(comptime) => {
                    Err(SimulatorError::unsupported_description(&comptime.token))
                }
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
                // Op::As is a type cast: just return the left operand unchanged.
                // The right operand is a type, not a runtime value.
                if matches!(op, Op::As) {
                    return Conv::conv(context, x.as_ref());
                }

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
                        return Ok(ProtoExpression::Value {
                            value: result,
                            width,
                            expr_context,
                        });
                    }
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
