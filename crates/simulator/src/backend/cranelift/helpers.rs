//! Cranelift codegen helpers shared by `expression.rs` / `statement.rs`:
//! wide 128-bit constants, masking, comparisons, wide-op helper calls.

use super::runtime::{
    Context as CraneliftContext, HelperSig, alloc_wide_slot, call_helper_ret, call_helper_void,
};
use crate::ir::{ProtoDynamicBitSelect, native_bytes as calc_native_bytes};
use crate::wide_ops;
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::{I32, I64};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::ir::Op;

/// `iconst` only accepts Imm64, so build I128 via `iconcat(lo, hi)`.
pub(crate) fn iconst_128(builder: &mut FunctionBuilder, val: u128) -> CraneliftValue {
    let lo = builder.ins().iconst(I64, val as u64 as i64);
    let hi = builder.ins().iconst(I64, (val >> 64) as u64 as i64);
    builder.ins().iconcat(lo, hi)
}

pub(crate) fn gen_mask_128(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    }
}

/// Mask for the bit range `[beg:end]` (inclusive both sides).
pub(crate) fn gen_mask_range_128(beg: usize, end: usize) -> u128 {
    gen_mask_128(beg - end + 1) << end
}

pub(crate) fn apply_mask_128(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    mask: u128,
) -> CraneliftValue {
    let mask_val = iconst_128(builder, mask);
    builder.ins().band(val, mask_val)
}

pub(crate) fn zero_for_width(
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

/// bxor with a constant: `bxor_imm` for I64, explicit const for I128.
pub(crate) fn bxor_const(
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

pub(crate) fn bor_const(
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

pub(crate) fn iadd_const(
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

pub(crate) fn icmp_const(
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

pub(crate) fn gen_mask_for_width(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else if width == 0 {
        0
    } else {
        (1u128 << width) - 1
    }
}

/// Clamped index → shift amount for dynamic bit select.
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

/// Width requires pointer-based wide representation (> 128 bit).
pub(crate) fn is_wide_ptr(width: usize) -> bool {
    width > 128
}

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

/// Promote a narrow value to a wide stack slot, or return as-is when
/// already a pointer.
pub(crate) fn ensure_wide_ptr_val(
    builder: &mut FunctionBuilder,
    val: CraneliftValue,
    src_width: usize,
    dst_nb: usize,
) -> CraneliftValue {
    if is_wide_ptr(src_width) {
        return val;
    }
    let ptr = alloc_wide_zero(builder, dst_nb);
    builder.ins().store(MemFlags::trusted(), val, ptr, 0);
    ptr
}

/// `extern "C"` wide-op addresses for `call_indirect`.  Two-step cast
/// (fn → *const () → usize) satisfies both the compiler and clippy.
pub(crate) mod wide_fn_addrs {
    use crate::wide_ops;

    macro_rules! fn_addr {
        ($f:expr) => {{
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

pub(crate) fn emit_wide_binary_op(
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

pub(crate) fn emit_wide_unary_op(
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

/// Wide constant in a stack slot, built from u64 digits.
pub(crate) fn emit_wide_const(
    builder: &mut FunctionBuilder,
    digits: &[u64],
    nb: usize,
) -> CraneliftValue {
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

/// Apply width mask to a wide buffer (clear bits ≥ width).
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

/// Wide mask_xz nonzero? Returns an I8 truth value.
pub(crate) fn emit_wide_is_nonzero(
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

/// Wide all-ones mask: ones in `[0..width)`, zeros above.
pub(crate) fn emit_wide_fill_ones(
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

/// Word-by-word select between two wide values.
pub(crate) fn emit_wide_select(
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

pub(crate) struct WideOperandPair {
    pub x_mask_xz: Option<CraneliftValue>,
    pub y_mask_xz: Option<CraneliftValue>,
    pub x_ptr: CraneliftValue,
    pub y_ptr: CraneliftValue,
    pub x_width: usize,
    pub y_width: usize,
    pub width: usize,
    pub op_nb: usize,
}

/// Either wide operand has nonzero mask_xz? Returns I8 truth value.
pub(crate) fn wide_any_xz(
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

pub(crate) fn expand_sign(
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

pub(crate) fn shift_mask_xz(
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
