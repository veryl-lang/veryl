//! Cranelift codegen helpers shared by `expression.rs` / `statement.rs`:
//! wide 128-bit constants, masking, comparisons, wide-op helper calls.

use super::runtime::{
    Context as CraneliftContext, HelperSig, alloc_wide_slot, call_helper_ret, call_helper_void,
};
use crate::ir::{ProtoDynamicBitSelect, ProtoExpression, native_bytes as calc_native_bytes};
use crate::wide_ops;
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::{I32, I64, I128};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlagsData};
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
        builder.ins().bxor_imm_u(val, imm as i64)
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
        builder.ins().band_imm_u(val, imm as i64)
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
        builder.ins().bor_imm_u(val, imm as i64)
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
        builder.ins().iadd_imm_s(val, imm)
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
        builder.ins().icmp_imm_s(cc, val, imm as i64)
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
    let shift = builder.ins().imul_imm_s(clamped, dyn_sel.elem_width as i64);
    Some(shift)
}

// ── Wide (>128-bit) helper utilities ────────────────────────────────

/// Register-vs-pointer boundary; defined in `ir::variable` so this backend and
/// `builds_wide_pointer` share one source of truth.
pub(crate) use crate::ir::variable::is_wide_ptr;

pub(crate) fn alloc_wide_zero(builder: &mut FunctionBuilder, nb: usize) -> CraneliftValue {
    let ptr = alloc_wide_slot(builder, nb);
    let zero = builder.ins().iconst(I64, 0);
    for i in 0..(nb / 8) {
        builder
            .ins()
            .store(MemFlagsData::trusted(), zero, ptr, (i * 8) as i32);
    }
    ptr
}

/// Zero-extend a wide-pointer value to span `nb` bytes.  A wide value's slot is
/// sized to its own width; a consumer that strides a wider `nb` (a concat/store
/// into a larger dst) would read past it into uninitialised stack.  Short →
/// copy into a zeroed `nb` slot; already `nb` or wider → passed through
/// (surplus high words ignored, never truncated).
pub(crate) fn widen_wide_ptr(
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    src_nb: usize,
    nb: usize,
) -> CraneliftValue {
    if src_nb >= nb {
        return ptr;
    }
    let slot = alloc_wide_zero(builder, nb);
    for i in 0..(src_nb / 8) {
        let off = (i * 8) as i32;
        let w = builder.ins().load(I64, MemFlagsData::trusted(), ptr, off);
        builder.ins().store(MemFlagsData::trusted(), w, slot, off);
    }
    slot
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
    builder.ins().store(MemFlagsData::trusted(), val, ptr, 0);
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
    pub fn resize() -> usize {
        fn_addr!(wide_ops::wide_resize)
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
    pub fn scmp_asym() -> usize {
        fn_addr!(wide_ops::wide_scmp_asym)
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

/// Buffers up to this many bytes get unrolled inline limb code in place of
/// a wide-helper call.  8 limbs covers the 512-bit packed lane buses whose
/// concat assembly makes shl/bor/apply_mask 74% of all wide helper calls
/// (measured on pe_core v8c); the win is the removed call + helper-loop
/// overhead per op.
pub(crate) const WIDE_INLINE_NB: usize = 64;

/// Unrolled bitwise op over an `nb`-byte buffer pair (`nb % 8 == 0`,
/// caller gates on [`WIDE_INLINE_NB`]).  Same limb semantics as
/// `wide_band`/`wide_bor`/`wide_bxor`/`wide_bxor_not`.
pub(crate) fn emit_wide_bitwise_inline(
    builder: &mut FunctionBuilder,
    op: Op,
    a: CraneliftValue,
    b: CraneliftValue,
    nb: usize,
) -> CraneliftValue {
    let dst = alloc_wide_slot(builder, nb);
    for i in 0..nb / 8 {
        let off = (i * 8) as i32;
        let la = builder.ins().load(I64, MemFlagsData::trusted(), a, off);
        let lb = builder.ins().load(I64, MemFlagsData::trusted(), b, off);
        let r = match op {
            Op::BitAnd => builder.ins().band(la, lb),
            Op::BitOr => builder.ins().bor(la, lb),
            Op::BitXor => builder.ins().bxor(la, lb),
            Op::BitXnor => {
                let x = builder.ins().bxor(la, lb);
                builder.ins().bnot(x)
            }
            _ => unreachable!("emit_wide_bitwise_inline: unsupported op"),
        };
        builder.ins().store(MemFlagsData::trusted(), r, dst, off);
    }
    dst
}

/// Unrolled constant-amount shift of an `nb`-byte buffer (`right` selects
/// logical right; left otherwise).  Limb-exact mirror of
/// `wide_shl`/`wide_lshr` with `word_shift`/`bit_shift` folded to
/// constants — the shift amounts of packed-lane concat assembly are
/// compile-time constants, so the helper's generic loop is pure overhead.
pub(crate) fn emit_wide_shift_const_inline(
    builder: &mut FunctionBuilder,
    x: CraneliftValue,
    amount: u64,
    nb: usize,
    right: bool,
) -> CraneliftValue {
    let dst = alloc_wide_slot(builder, nb);
    let n = nb / 8;
    let word_shift = (amount / 64) as usize;
    let bit_shift = (amount % 64) as u32;
    for i in 0..n {
        let off = (i * 8) as i32;
        let in_range = if right {
            word_shift < n - i
        } else {
            word_shift <= i
        };
        let r = if !in_range {
            builder.ins().iconst(I64, 0)
        } else if right {
            let src = i + word_shift;
            let lo = builder
                .ins()
                .load(I64, MemFlagsData::trusted(), x, (src * 8) as i32);
            if bit_shift == 0 {
                lo
            } else {
                let hi = if src + 1 < n {
                    builder
                        .ins()
                        .load(I64, MemFlagsData::trusted(), x, ((src + 1) * 8) as i32)
                } else {
                    builder.ins().iconst(I64, 0)
                };
                let a = builder.ins().ushr_imm_s(lo, bit_shift as i64);
                let b = builder.ins().ishl_imm_s(hi, (64 - bit_shift) as i64);
                builder.ins().bor(a, b)
            }
        } else {
            let src = i - word_shift;
            let lo = builder
                .ins()
                .load(I64, MemFlagsData::trusted(), x, (src * 8) as i32);
            if bit_shift == 0 {
                lo
            } else {
                let hi = if src > 0 {
                    builder
                        .ins()
                        .load(I64, MemFlagsData::trusted(), x, ((src - 1) * 8) as i32)
                } else {
                    builder.ins().iconst(I64, 0)
                };
                let a = builder.ins().ishl_imm_s(lo, bit_shift as i64);
                let b = builder.ins().ushr_imm_s(hi, (64 - bit_shift) as i64);
                builder.ins().bor(a, b)
            }
        };
        builder.ins().store(MemFlagsData::trusted(), r, dst, off);
    }
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
            .store(MemFlagsData::trusted(), v, ptr, (i * 8) as i32);
    }
    ptr
}

/// Whether to elide provably-no-op wide width-masks (default on;
/// `VERYL_WIDE_MASK_ELIDE=0` disables for A/B measurement).
fn wide_mask_elide_on() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_WIDE_MASK_ELIDE").as_deref() != Ok("0"))
}

/// Apply width mask to a wide buffer (clear bits ≥ width).
pub(crate) fn emit_wide_apply_mask(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    nb: usize,
    width: usize,
) {
    // When `width` fills the whole buffer there are no bits ≥ width to clear, so
    // `wide_apply_mask` is a proven no-op (partial-word mask skipped, zero-fill
    // loop empty). Word-aligned wide values (e.g. pe_core's 512-bit v8c lanes)
    // hit this, so eliding the call drops a share of the per-cycle wide helper
    // calls. Sound: identical result, one fewer call + comb-buffer touch.
    // `VERYL_WIDE_MASK_ELIDE=0` disables it for A/B measurement.
    if width >= nb * 8 && wide_mask_elide_on() {
        return;
    }
    // Unrolled in-place mask: same limb semantics as `wide_apply_mask`
    // (partial limb masked, higher limbs zeroed).  Only the limbs at or
    // above `width` are touched, so the inline is gated on THAT count, not
    // on `nb` — a 1953-bit value in a 2048-bit buffer costs two stores.
    let touched = (nb / 8).saturating_sub(width / 64);
    if touched <= 8 && nb.is_multiple_of(8) && width > 0 {
        let n = nb / 8;
        let full_words = width / 64;
        let remaining = width % 64;
        if remaining > 0 && full_words < n {
            let off = (full_words * 8) as i32;
            let v = builder.ins().load(I64, MemFlagsData::trusted(), ptr, off);
            let m = builder
                .ins()
                .band_imm_s(v, ((1u64 << remaining) - 1) as i64);
            builder.ins().store(MemFlagsData::trusted(), m, ptr, off);
        }
        let zero_from = full_words + usize::from(remaining > 0);
        if zero_from < n {
            let z = builder.ins().iconst(I64, 0);
            for i in zero_from..n {
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), z, ptr, (i * 8) as i32);
            }
        }
        return;
    }
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
    builder.ins().icmp_imm_s(IntCC::NotEqual, result, 0)
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

/// Static narrow bit-select read of a wide (>128-bit) value: extract bits
/// `[end ..= beg]` (result `width` = `beg - end + 1` ≤ 64) from the wide value
/// at `ptr` into a single I64.  `beg`/`end` are compile-time constants, so the
/// word index and intra-word shift are constants and the selection touches at
/// most two adjacent 64-bit words.  Returns the masked I64 value.
pub(crate) fn emit_wide_bit_select_read_narrow(
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    beg: usize,
    end: usize,
    width: usize,
) -> CraneliftValue {
    debug_assert!(beg >= end && width <= 64 && width > 0);
    let flags = MemFlagsData::trusted();
    let word = end / 64;
    let bit = (end % 64) as i64;
    let lo = builder.ins().load(I64, flags, ptr, (word * 8) as i32);
    let mut result = if bit == 0 {
        lo
    } else {
        builder.ins().ushr_imm_u(lo, bit)
    };
    // Selection straddles into the next word? (only possible when bit > 0)
    if (end % 64) + width > 64 {
        let hi = builder.ins().load(I64, flags, ptr, ((word + 1) * 8) as i32);
        let hi_part = builder.ins().ishl_imm_u(hi, 64 - bit);
        result = builder.ins().bor(result, hi_part);
    }
    // Mask to the declared result width (width == 64 needs no mask).
    if width < 64 {
        let mask = ((1u64 << width) - 1) as i64;
        result = builder.ins().band_imm_u(result, mask);
    }
    result
}

/// Read a 64-bit window of a wide value at an arbitrary compile-time bit
/// offset: bits `[bit_start ..= bit_start + 63]` of the value at `ptr`.  A word
/// at/beyond `src_words` folds to a zero const, so bits past the source read as
/// zero with no runtime bounds check.
fn read_shifted_word(
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    bit_start: usize,
    src_words: usize,
) -> CraneliftValue {
    let flags = MemFlagsData::trusted();
    let word = bit_start / 64;
    let bit = (bit_start % 64) as i64;
    let lo = if word < src_words {
        builder.ins().load(I64, flags, ptr, (word * 8) as i32)
    } else {
        builder.ins().iconst(I64, 0)
    };
    let mut result = if bit == 0 {
        lo
    } else {
        builder.ins().ushr_imm_u(lo, bit)
    };
    if bit != 0 {
        let hi = if word + 1 < src_words {
            builder.ins().load(I64, flags, ptr, ((word + 1) * 8) as i32)
        } else {
            builder.ins().iconst(I64, 0)
        };
        let hi_part = builder.ins().ishl_imm_u(hi, 64 - bit);
        result = builder.ins().bor(result, hi_part);
    }
    result
}

/// Extract bits `[end ..= beg]` (`width` in `65..=128`) of the wide value at
/// `ptr` into an I128 register — not a pointer, since the value form must match
/// `is_wide_ptr(width)` (register for `width <= 128`).
pub(crate) fn emit_wide_bit_select_read_i128(
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    src_full_width: usize,
    beg: usize,
    end: usize,
    width: usize,
) -> CraneliftValue {
    debug_assert!(beg >= end && width == beg - end + 1 && width > 64 && width <= 128);
    let src_words = calc_native_bytes(src_full_width) / 8;
    let w0 = read_shifted_word(builder, ptr, end, src_words);
    let mut w1 = read_shifted_word(builder, ptr, end + 64, src_words);
    let hi_bits = width - 64;
    if hi_bits < 64 {
        w1 = builder.ins().band_imm_u(w1, ((1u64 << hi_bits) - 1) as i64);
    }
    builder.ins().iconcat(w0, w1)
}

/// Extract bits `[end ..= beg]` (`width` > 128) of the wide value at `ptr` into
/// a fresh slot and return the pointer.  `src_full_width` bounds the source word
/// count; the caller treats the result as a wide pointer per `builds_wide_pointer`.
pub(crate) fn emit_wide_bit_select_read_wide(
    builder: &mut FunctionBuilder,
    ptr: CraneliftValue,
    src_full_width: usize,
    beg: usize,
    end: usize,
    width: usize,
) -> CraneliftValue {
    debug_assert!(beg >= end && width == beg - end + 1 && width > 128);
    let flags = MemFlagsData::trusted();
    let src_words = calc_native_bytes(src_full_width) / 8;
    let result_nb = calc_native_bytes(width);
    let result_words = result_nb / 8;
    let dst = alloc_wide_slot(builder, result_nb);
    for j in 0..result_words {
        let mut w = read_shifted_word(builder, ptr, end + j * 64, src_words);
        // Only the top result word can be partial; lower words are full.
        let valid_bits = width - j * 64;
        if valid_bits < 64 {
            let mask = ((1u64 << valid_bits) - 1) as i64;
            w = builder.ins().band_imm_u(w, mask);
        }
        builder.ins().store(flags, w, dst, (j * 8) as i32);
    }
    dst
}

/// Static bit-select `[end ..= beg]` of a wide (>128-bit) source, dispatched to
/// the value form `is_wide_ptr(width)` expects.  The one place the
/// width→representation split lives; shared by the Variable and DynamicVariable
/// select paths.
pub(crate) fn emit_wide_bit_select_read(
    builder: &mut FunctionBuilder,
    base: CraneliftValue,
    src_full_width: usize,
    beg: usize,
    end: usize,
    width: usize,
) -> CraneliftValue {
    if width <= 64 {
        emit_wide_bit_select_read_narrow(builder, base, beg, end, width)
    } else if is_wide_ptr(width) {
        emit_wide_bit_select_read_wide(builder, base, src_full_width, beg, end, width)
    } else {
        emit_wide_bit_select_read_i128(builder, base, src_full_width, beg, end, width)
    }
}

/// `rhs_select`: `dst = (src >> end) & mask(width)` for a wide source, into a
/// fresh wide slot.  `end` is a compile-time constant.
pub(crate) fn emit_wide_shift_right_mask(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    src_ptr: CraneliftValue,
    end: usize,
    width: usize,
    nb: usize,
) -> CraneliftValue {
    let dst = alloc_wide_slot(builder, nb);
    let amount = builder.ins().iconst(I64, end as i64);
    let nb_val = builder.ins().iconst(I32, nb as i64);
    call_helper_void(
        context,
        builder,
        HelperSig::BinaryOp,
        wide_fn_addrs::lshr(),
        &[dst, src_ptr, amount, nb_val],
    );
    emit_wide_apply_mask(context, builder, dst, nb, width);
    dst
}

/// `select` (wide-dst bit-select WRITE / RMW), 2-state:
/// `new = (old & ~rangemask) | ((src << end) & rangemask)` where `rangemask`
/// covers bits `[end ..= end + width - 1]`.  Returns a fresh wide slot holding
/// the full post-RMW value.  `old_ptr` points at the current dst value.
pub(crate) fn emit_wide_select_rmw(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    old_ptr: CraneliftValue,
    src_ptr: CraneliftValue,
    end: usize,
    width: usize,
    nb: usize,
) -> CraneliftValue {
    if nb <= WIDE_INLINE_NB && nb.is_multiple_of(8) {
        // `end`/`width` are compile-time constants, so the range mask and
        // the shift split fold away: limbs outside the range copy `old`,
        // limbs fully inside take the shifted source, boundary limbs
        // blend — a lane store into a packed bus becomes 1-2 computed
        // limbs instead of six wide-helper calls over every limb.
        let flags = MemFlagsData::trusted();
        let dst = alloc_wide_slot(builder, nb);
        let n = nb / 8;
        let lo_limb = end / 64;
        let hi_limb = (end + width - 1) / 64;
        let ws = end / 64;
        let bs = (end % 64) as u32;
        for i in 0..n {
            let off = (i * 8) as i32;
            let r = if i < lo_limb || i > hi_limb {
                builder.ins().load(I64, flags, old_ptr, off)
            } else {
                // Bits of [end, end+width) covering limb i.
                let lo_bit = (i * 64).max(end);
                let hi_bit = ((i + 1) * 64).min(end + width);
                let m = if hi_bit - lo_bit >= 64 {
                    u64::MAX
                } else {
                    ((1u64 << (hi_bit - lo_bit)) - 1) << (lo_bit % 64)
                };
                // (src << end) limb i, with out-of-range source limbs zero.
                let src_i = i as isize - ws as isize;
                let lo = if src_i >= 0 {
                    builder.ins().load(I64, flags, src_ptr, (src_i * 8) as i32)
                } else {
                    builder.ins().iconst(I64, 0)
                };
                let shifted = if bs == 0 {
                    lo
                } else {
                    let hi = if src_i > 0 {
                        builder
                            .ins()
                            .load(I64, flags, src_ptr, ((src_i - 1) * 8) as i32)
                    } else {
                        builder.ins().iconst(I64, 0)
                    };
                    let a = builder.ins().ishl_imm_s(lo, bs as i64);
                    let b = builder.ins().ushr_imm_s(hi, (64 - bs) as i64);
                    builder.ins().bor(a, b)
                };
                if m == u64::MAX {
                    shifted
                } else {
                    let sm = builder.ins().band_imm_s(shifted, m as i64);
                    let old = builder.ins().load(I64, flags, old_ptr, off);
                    let om = builder.ins().band_imm_s(old, !m as i64);
                    builder.ins().bor(om, sm)
                }
            };
            builder.ins().store(flags, r, dst, off);
        }
        return dst;
    }
    let amount = builder.ins().iconst(I64, end as i64);
    let nb_val = builder.ins().iconst(I32, nb as i64);
    // rangemask = fill_ones(width) << end
    let rmask = emit_wide_fill_ones(context, builder, nb, width);
    call_helper_void(
        context,
        builder,
        HelperSig::BinaryOp,
        wide_fn_addrs::shl(),
        &[rmask, rmask, amount, nb_val],
    );
    // src_in_range = (src << end) & rangemask
    let src_sh = alloc_wide_slot(builder, nb);
    call_helper_void(
        context,
        builder,
        HelperSig::BinaryOp,
        wide_fn_addrs::shl(),
        &[src_sh, src_ptr, amount, nb_val],
    );
    call_helper_void(
        context,
        builder,
        HelperSig::BinaryOp,
        wide_fn_addrs::band(),
        &[src_sh, src_sh, rmask, nb_val],
    );
    // new = (old & ~rangemask) | src_in_range
    let new = alloc_wide_slot(builder, nb);
    call_helper_void(
        context,
        builder,
        HelperSig::BinaryOp,
        wide_fn_addrs::band_not(),
        &[new, old_ptr, rmask, nb_val],
    );
    call_helper_void(
        context,
        builder,
        HelperSig::BinaryOp,
        wide_fn_addrs::bor(),
        &[new, new, src_sh, nb_val],
    );
    new
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
    let flags = MemFlagsData::trusted();
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
    /// `returns_wide_pointer(x)`/`(y)`: the mask marshaling gates on these, not
    /// on `is_wide_ptr(x_width)` (an inflated width can accompany a scalar).
    pub x_is_ptr: bool,
    pub y_is_ptr: bool,
    pub width: usize,
    pub op_nb: usize,
}

/// Either wide operand has nonzero mask_xz? Returns I8 truth value.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wide_any_xz(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    x_mask_xz: Option<CraneliftValue>,
    y_mask_xz: Option<CraneliftValue>,
    x_is_ptr: bool,
    y_is_ptr: bool,
    x_width: usize,
    y_width: usize,
) -> Option<CraneliftValue> {
    if !context.use_4state {
        return None;
    }
    // A scalar mask can carry an inflated wide `width` (`c + (a==b)`), so gate on
    // `x_is_ptr`, and pick i64-vs-i128 from the value's type, not the width.
    let x_has_xz = x_mask_xz.map(|m| {
        if x_is_ptr {
            emit_wide_is_nonzero(context, builder, m, calc_native_bytes(x_width))
        } else {
            let wide = builder.func.dfg.value_type(m) == I128;
            icmp_const(builder, IntCC::NotEqual, m, 0, wide)
        }
    });
    let y_has_xz = y_mask_xz.map(|m| {
        if y_is_ptr {
            emit_wide_is_nonzero(context, builder, m, calc_native_bytes(y_width))
        } else {
            let wide = builder.func.dfg.value_type(m) == I128;
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

/// Cranelift-side refinement of `ProtoExpression::builds_wide_pointer`:
/// `build_binary_wide_binary` hands back a masked SCALAR for a
/// non-comparison Binary whose own evaluation width is ≤128 even when an
/// operand lives in wide-pointer storage (a narrowing `as` cast keeps its
/// operand in the source domain), so consumers must not dereference it.
/// Every other node kind matches the shared predicate, which the AOT-C
/// emitter keeps using as-is: its wide builder always produces a buffer.
pub(crate) fn returns_wide_pointer(expr: &ProtoExpression) -> bool {
    match expr {
        ProtoExpression::Binary { expr_context, .. } => {
            expr.builds_wide_pointer() && expr_context.width > 128
        }
        // The funnel-load path for a dynamic element read of a wide
        // variable (window ≤ 64, no combined static select) hands back a
        // narrow scalar; the shared predicate still reports a pointer
        // because the AOT-C emitter keeps that shape on the interpreter.
        ProtoExpression::Variable {
            dynamic_select: Some(ds),
            select: None,
            var_full_width,
            ..
        } if is_wide_ptr(*var_full_width) && ds.window <= 64 => false,
        _ => expr.builds_wide_pointer(),
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
        let dst_wide = dst_width > 64;
        // narrow → wide: uextend payload to I128 before downstream
        // `bor_const(.., wide=true)` builds an I128 mask.  Gate on the
        // ACTUAL value type, not the logical widths: the caller may have
        // widened a narrow operand already (e.g. the binary-op type
        // unification when the other operand is wide).
        if dst_wide && builder.func.dfg.value_type(payload) != I128 {
            payload = builder.ins().uextend(I128, payload);
        }
        if dst_wide
            && let Some(m) = mask_xz
            && builder.func.dfg.value_type(m) != I128
        {
            mask_xz = Some(builder.ins().uextend(I128, m));
        }
        let mask = gen_mask_for_width(dst_width) ^ gen_mask_for_width(src_width);
        let msb = builder.ins().ushr_imm_u(payload, (src_width - 1) as i64);
        let ext = bor_const(builder, payload, mask, dst_wide);
        payload = builder.ins().select(msb, ext, payload);
        if let Some(x) = mask_xz {
            let msb_xz = builder.ins().ushr_imm_u(x, (src_width - 1) as i64);
            let ext_xz = bor_const(builder, x, mask, dst_wide);
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
                // Operands wider than the native container are reduced to
                // `native_bits` bits, so clamp the sign-extension width;
                // `native_bits - x_width` would otherwise underflow.
                let eff_width = x_width.min(native_bits);
                let shl_amount = (native_bits - eff_width) as i64;
                let shifted_up = builder.ins().ishl_imm_u(mask_xz, shl_amount);
                let sign_extended = builder.ins().sshr_imm_u(shifted_up, shl_amount);
                builder.ins().sshr(sign_extended, y_payload)
            } else {
                builder.ins().ushr(mask_xz, y_payload)
            }
        }
        _ => unreachable!(),
    }
}
