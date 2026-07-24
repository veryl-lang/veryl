//! Cranelift codegen impl blocks for `ProtoStatement` and friends,
//! sibling to the IR-side impls in `crate::ir::statement`.

use super::helpers::*;
use super::runtime::{
    Context as CraneliftContext, emit_inline_write_log_push, emit_inline_write_log_push_wide,
};
use crate::ir::variable::native_bytes as calc_native_bytes;
use crate::ir::{
    ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoCaseStatement, ProtoExpression,
    ProtoForBound, ProtoForRange, ProtoForStatement, ProtoIfStatement, ProtoStatement,
};
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::{I32, I64, I128};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlagsData};
use veryl_analyzer::ir as air;
use veryl_analyzer::value::ValueU64;

/// Push an FF write-log entry for a post-RMW `payload` (+ optional 4-state
/// `mask_xz`). Narrow FFs (`nb <= 8`) push one I64 entry; wide FFs (`nb > 8`,
/// I128 payload) split into per-8-byte words — `emit_inline_write_log_push`
/// takes an I64, so passing a wide value directly panics cranelift.
fn emit_ff_log_push(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    log_current_offset: i32,
    nb: usize,
    payload: CraneliftValue,
    mask_xz: Option<CraneliftValue>,
) {
    let nb_i32 = nb as i32;
    if nb <= 8 {
        let offset_val = builder.ins().iconst(I32, log_current_offset as i64);
        let width_class_val = builder.ins().iconst(I32, nb as i64);
        emit_inline_write_log_push(context, builder, offset_val, payload, width_class_val);
        if context.use_4state
            && let Some(m) = mask_xz
        {
            let mask_offset_val = builder
                .ins()
                .iconst(I32, (log_current_offset + nb_i32) as i64);
            emit_inline_write_log_push(context, builder, mask_offset_val, m, width_class_val);
        }
    } else {
        // Wide FF: split the I128 payload into per-8-byte words, one narrow
        // entry each (commit-side `ff_commit_from_log` writes 8 bytes per
        // width_class=8 entry).
        let n_words = nb / 8;
        let width_class_val = builder.ins().iconst(I32, 8);
        for i in 0..n_words {
            let shifted = if i == 0 {
                payload
            } else {
                builder.ins().ushr_imm_u(payload, (i * 64) as i64)
            };
            let word = builder.ins().ireduce(I64, shifted);
            let off = log_current_offset + (i * 8) as i32;
            let entry_offset_val = builder.ins().iconst(I32, off as i64);
            emit_inline_write_log_push(context, builder, entry_offset_val, word, width_class_val);
        }
        if context.use_4state
            && let Some(m) = mask_xz
        {
            for i in 0..n_words {
                let shifted = if i == 0 {
                    m
                } else {
                    builder.ins().ushr_imm_u(m, (i * 64) as i64)
                };
                let word = builder.ins().ireduce(I64, shifted);
                let off = log_current_offset + nb_i32 + (i * 8) as i32;
                let entry_offset_val = builder.ins().iconst(I32, off as i64);
                emit_inline_write_log_push(
                    context,
                    builder,
                    entry_offset_val,
                    word,
                    width_class_val,
                );
            }
        }
    }
}

impl ProtoAssignDynamicStatement {
    pub fn can_build_binary(&self) -> bool {
        if !self.expr.can_build_binary() || !self.dst_index_expr.can_build_binary() {
            return false;
        }
        // Sign-extension of a bare narrow signed RHS at the store is
        // handled only by the interpreter for dynamic-index destinations.
        if self.rhs_select.is_none() && self.expr.store_sign_extend_from(self.dst_width).is_some() {
            return false;
        }
        if let Some(dyn_sel) = &self.dynamic_select {
            let full_width = dyn_sel.elem_width * dyn_sel.num_elements;
            if full_width > 128 || !dyn_sel.index_expr.can_build_binary() {
                return false;
            }
            return full_width <= 64;
        }
        // build_binary's rhs_select slicing assumes a ≤64-bit scalar
        // payload; wider sources stay on the interpreter (rare).
        if self.rhs_select.is_some() && self.expr.width() > 64 {
            return false;
        }
        // Wide (>128-bit) comb-base dynamic-indexed store: see
        // `build_binary_dynamic_wide`.
        if self.dst_width > 128 && !self.dst_base.is_ff() && self.rhs_select.is_none() {
            return true;
        }
        // Wide (>64-bit) FF-base full-element dynamic store:
        // `build_binary_dynamic_wide_ff`.  A 4-state dst bails there (→ module
        // falls back), so the gate here is optimistic.
        if self.dst_width > 64
            && self.dst_base.is_ff()
            && self.select.is_none()
            && self.rhs_select.is_none()
            && self.dynamic_select.is_none()
        {
            return true;
        }
        self.dst_width <= 64
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        // The narrow path below assumes a native (≤8-byte) dst; route the
        // wide (>128-bit) comb-base case to its own emitter.
        if self.dst_width > 128 && !self.dst_base.is_ff() {
            return self.build_binary_dynamic_wide(context, builder);
        }
        // Wide (>64-bit) FF-base full-element dynamic write: see
        // `build_binary_dynamic_wide_ff`.
        if self.dst_width > 64
            && self.dst_base.is_ff()
            && self.select.is_none()
            && self.rhs_select.is_none()
            && self.dynamic_select.is_none()
        {
            return self.build_binary_dynamic_wide_ff(context, builder);
        }
        // Plain store re-masks the payload to dst_width (istoreN truncation
        // or the non-native band below), so the producer-side root mask is
        // redundant; select/dynamic_select shift the payload and need it.
        // Unlike the static `ProtoAssignStatement` path, no `dst_width <= 64`
        // guard is needed: a wide non-native dst bails (`return None`) below
        // and this dynamic path has no load-cache forwarding that could
        // observe an unmasked payload.
        let (mut payload, mut mask_xz) = if self.select.is_none() && self.dynamic_select.is_none() {
            self.expr.build_binary_root(context, builder)?
        } else {
            self.expr.build_binary(context, builder)?
        };
        let nb = calc_native_bytes(self.dst_width);
        let nb_i32 = nb as i32;

        if let Some((beg, end)) = self.rhs_select {
            let mask = ValueU64::gen_mask(beg - end + 1);

            payload = builder.ins().ushr_imm_u(payload, end as i64);
            payload = builder.ins().band_imm_u(payload, mask as i64);

            if let Some(mxz) = mask_xz {
                let mxz = builder.ins().ushr_imm_u(mxz, end as i64);
                let mxz = builder.ins().band_imm_u(mxz, mask as i64);
                mask_xz = Some(mxz);
            }
        }

        // Compute dynamic address
        let (idx_payload, _idx_mask_xz) = self.dst_index_expr.build_binary(context, builder)?;

        let max_idx = builder
            .ins()
            .iconst(I64, (self.dst_num_elements as i64).saturating_sub(1));
        let in_bounds = builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, idx_payload, max_idx);
        let clamped = builder.ins().select(in_bounds, idx_payload, max_idx);

        let stride_val = builder.ins().iconst(I64, self.dst_stride as i64);
        let byte_offset = builder.ins().imul(clamped, stride_val);

        let base_addr = if self.dst_base.is_ff() {
            context.ff_values
        } else {
            context.comb_values
        };
        let static_offset = builder.ins().iconst(I64, self.dst_base.raw() as i64);
        let addr = builder.ins().iadd(base_addr, static_offset);
        let addr = builder.ins().iadd(addr, byte_offset);

        // FF write log push: log offset = (canonical current base) +
        // runtime byte_offset, regardless of whether dst_base points to
        // the current slot (packed layout) or the next slot (dual-slot
        // multi-RMW).  The canonical `dst_ff_current_base_offset` always
        // reflects the current slot origin.
        let emit_log = self.dst_base.is_ff() && self.dst_width <= 64;
        let is_packed_ff_dyn = emit_log && (self.dst_base.raw() == self.dst_ff_current_base_offset);
        let log_offset_i32 = if emit_log {
            let log_base = builder
                .ins()
                .iconst(I64, self.dst_ff_current_base_offset as i64);
            let log_offset_64 = builder.ins().iadd(log_base, byte_offset);
            Some(builder.ins().ireduce(I32, log_offset_64))
        } else {
            None
        };

        let load_mem_flag = MemFlagsData::trusted();
        let store_mem_flag = MemFlagsData::trusted();

        // Helpers covering the nb ∈ {1, 2, 4, 8} storage widths.
        // Use uload8/uload16/uload32 (fused load-and-zero-extend) for narrow
        // widths so each load lowers to one x86 movzbq instead of two.
        let load_native_to_i64 =
            |builder: &mut FunctionBuilder, addr: cranelift::prelude::Value, off: i32| match nb {
                1 => builder.ins().uload8(I64, load_mem_flag, addr, off),
                2 => builder.ins().uload16(I64, load_mem_flag, addr, off),
                4 => builder.ins().uload32(load_mem_flag, addr, off),
                _ => builder.ins().load(I64, load_mem_flag, addr, off),
            };
        let store_i64_to_native = |builder: &mut FunctionBuilder,
                                   v: cranelift::prelude::Value,
                                   addr: cranelift::prelude::Value,
                                   off: i32| match nb {
            1 => {
                builder.ins().istore8(store_mem_flag, v, addr, off);
            }
            2 => {
                builder.ins().istore16(store_mem_flag, v, addr, off);
            }
            4 => {
                builder.ins().istore32(store_mem_flag, v, addr, off);
            }
            _ => {
                builder.ins().store(store_mem_flag, v, addr, off);
            }
        };

        let emit_log_push = |context: &mut CraneliftContext,
                             builder: &mut FunctionBuilder,
                             payload: cranelift::prelude::Value,
                             mask: Option<cranelift::prelude::Value>| {
            if let Some(offset_val) = log_offset_i32 {
                let width_class_val = builder.ins().iconst(I32, nb as i64);
                emit_inline_write_log_push(context, builder, offset_val, payload, width_class_val);
                if context.use_4state
                    && let Some(mask_v) = mask
                {
                    let mask_offset_val = builder.ins().iadd_imm_s(offset_val, nb as i64);
                    emit_inline_write_log_push(
                        context,
                        builder,
                        mask_offset_val,
                        mask_v,
                        width_class_val,
                    );
                }
            }
        };
        if let Some(dyn_sel) = &self.dynamic_select {
            let shift = build_dynamic_select_shift(dyn_sel, context, builder)?;

            let payload = builder.ins().ishl(payload, shift);

            let elem_mask = gen_mask_for_width(dyn_sel.window);
            let mask_val = builder.ins().iconst(I64, elem_mask as i64);
            let dyn_mask = builder.ins().ishl(mask_val, shift);
            let not_mask = builder.ins().bnot(dyn_mask);

            let org = load_native_to_i64(builder, addr, 0);
            let org = builder.ins().band(org, not_mask);
            let result = builder.ins().bor(payload, org);
            if !is_packed_ff_dyn {
                store_i64_to_native(builder, result, addr, 0);
            }
            let mask_result = if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl(mask_xz, shift);
                let org = load_native_to_i64(builder, addr, nb_i32);
                let org = builder.ins().band(org, not_mask);
                let result_m = builder.ins().bor(mask_xz, org);
                if !is_packed_ff_dyn {
                    store_i64_to_native(builder, result_m, addr, nb_i32);
                }
                Some(result_m)
            } else {
                None
            };
            emit_log_push(context, builder, result, mask_result);
        } else if let Some((beg, end)) = self.select {
            let mask = ValueU64::gen_mask_range(beg, end);

            // Clip the shifted payload to the [beg:end] window: the RHS is
            // evaluated at the assignment width (`s.f = ~x`), so bits above
            // `beg` would else leak into adjacent fields / the storage padding.
            let payload = builder.ins().ishl_imm_u(payload, end as i64);
            let payload = builder.ins().band_imm_u(payload, mask as i64);
            let org = load_native_to_i64(builder, addr, 0);
            let org = builder.ins().band_imm_u(org, !mask as i64);
            let result = builder.ins().bor(payload, org);
            if !is_packed_ff_dyn {
                store_i64_to_native(builder, result, addr, 0);
            }
            let mask_result = if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm_u(mask_xz, end as i64);
                let mask_xz = builder.ins().band_imm_u(mask_xz, mask as i64);
                let org = load_native_to_i64(builder, addr, nb_i32);
                let org = builder.ins().band_imm_u(org, !mask as i64);
                let result_m = builder.ins().bor(mask_xz, org);
                if !is_packed_ff_dyn {
                    store_i64_to_native(builder, result_m, addr, nb_i32);
                }
                Some(result_m)
            } else {
                None
            };
            emit_log_push(context, builder, result, mask_result);
        } else {
            // Mask payload to dst_width when not a clean native width;
            // matches the loaded-width semantics that ff_commit_from_log
            // expects from the log payload.
            let (payload_to_store, payload_for_log) = match self.dst_width {
                8 | 16 | 32 | 64 => (payload, payload),
                _ => {
                    if self.dst_width >= 64 {
                        return None;
                    }
                    let mask = (1u64 << self.dst_width) - 1;
                    let masked = builder.ins().band_imm_u(payload, mask as i64);
                    (masked, masked)
                }
            };
            let mask_xz_for_log = if let Some(mask_xz_v) = mask_xz {
                let m = if !matches!(self.dst_width, 8 | 16 | 32 | 64) {
                    let mask = (1u64 << self.dst_width) - 1;
                    builder.ins().band_imm_u(mask_xz_v, mask as i64)
                } else {
                    mask_xz_v
                };
                Some(m)
            } else {
                None
            };
            if !is_packed_ff_dyn {
                match nb {
                    1 => {
                        builder
                            .ins()
                            .istore8(store_mem_flag, payload_to_store, addr, 0);
                    }
                    2 => {
                        builder
                            .ins()
                            .istore16(store_mem_flag, payload_to_store, addr, 0);
                    }
                    4 => {
                        builder
                            .ins()
                            .istore32(store_mem_flag, payload_to_store, addr, 0);
                    }
                    _ => {
                        builder
                            .ins()
                            .store(store_mem_flag, payload_to_store, addr, 0);
                    }
                }
                if let Some(mask_v) = mask_xz_for_log {
                    match nb {
                        1 => {
                            builder.ins().istore8(store_mem_flag, mask_v, addr, nb_i32);
                        }
                        2 => {
                            builder.ins().istore16(store_mem_flag, mask_v, addr, nb_i32);
                        }
                        4 => {
                            builder.ins().istore32(store_mem_flag, mask_v, addr, nb_i32);
                        }
                        _ => {
                            builder.ins().store(store_mem_flag, mask_v, addr, nb_i32);
                        }
                    }
                }
            }
            // Simple-else log push: payload is the I64 value whose lower
            // `nb*8` bits match what istoreN deposited.  4-state appends
            // mask_xz at `offset + nb`.
            emit_log_push(context, builder, payload_for_log, mask_xz_for_log);
        }

        Some(())
    }

    /// Runtime byte-address of the dynamically-indexed element:
    /// `comb_values + dst_base + dst_stride * clamp(idx, num_elements-1)`.
    fn dynamic_elem_addr(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<cranelift::prelude::Value> {
        let (idx_payload, _) = self.dst_index_expr.build_binary(context, builder)?;
        let max_idx = builder
            .ins()
            .iconst(I64, (self.dst_num_elements as i64).saturating_sub(1));
        let in_bounds = builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, idx_payload, max_idx);
        let clamped = builder.ins().select(in_bounds, idx_payload, max_idx);
        let stride_val = builder.ins().iconst(I64, self.dst_stride as i64);
        let byte_offset = builder.ins().imul(clamped, stride_val);
        let static_offset = builder.ins().iconst(I64, self.dst_base.raw() as i64);
        let addr = builder.ins().iadd(context.comb_values, static_offset);
        Some(builder.ins().iadd(addr, byte_offset))
    }

    /// Wide (>128-bit) comb-base dynamic-indexed store (`var arr[idx] <= wide`).
    /// Stores byte-for-byte to the comb buffer (no write-log) and masks to
    /// `dst_width`; a ≤64-bit `select` is a scalar field RMW. Mirrors the AOT-C
    /// wide AssignDynamic path. 4-state / `rhs_select` / `dynamic_select` bail.
    fn build_binary_dynamic_wide(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        if context.use_4state || self.rhs_select.is_some() || self.dynamic_select.is_some() {
            return None;
        }
        if self.dst_base.is_ff() || self.dst_num_elements == 0 {
            return None;
        }
        let nb = calc_native_bytes(self.dst_width);
        let n_words = nb / 8;
        let flags = MemFlagsData::trusted();

        // Narrow-field (≤64-bit) bit-select: scalar 1-2 word RMW at the runtime
        // element address (the helper clamps the field to `dst_width`).
        if let Some((beg, end)) = self.select
            && (beg - end + 1) <= 64
        {
            let (raw, _mask_xz) = self.expr.build_binary(context, builder)?;
            // A `builds_wide_pointer` expr returns a pointer; the field is its
            // low word. A scalar IS that word. (cf. `wide_shift_amount`.)
            let sv = if returns_wide_pointer(&self.expr) {
                builder.ins().load(I64, MemFlagsData::trusted(), raw, 0)
            } else {
                raw
            };
            let addr = self.dynamic_elem_addr(context, builder)?;
            emit_wide_narrow_field_store(builder, addr, 0, beg, end, self.dst_width, sv);
            return Some(());
        }

        // Build the wide RHS into a pointer (a register result is force-stored
        // into a fresh slot; `builds_wide_pointer()` is the gate, not `width`).
        let (payload, _mask_xz) = self.expr.build_binary(context, builder)?;
        let src_ptr = if returns_wide_pointer(&self.expr) {
            // The slot is sized to the expr's width, which may be NARROWER than
            // the dst element; the copy below reads `nb` (dst) bytes, so a
            // narrower source would read past it into uninitialised stack. When
            // sizes differ, marshal into a zeroed dst-sized slot copying only
            // min(src_nb, nb) bytes. Mirrors AOT-C's `emit_wide_operand`.
            let src_nb = calc_native_bytes(self.expr.width());
            if src_nb == nb {
                payload
            } else {
                let slot = alloc_wide_zero(builder, nb);
                let copy_words = src_nb.min(nb) / 8;
                for i in 0..copy_words {
                    let off = (i * 8) as i32;
                    let w = builder
                        .ins()
                        .load(I64, MemFlagsData::trusted(), payload, off);
                    builder.ins().store(MemFlagsData::trusted(), w, slot, off);
                }
                slot
            }
        } else {
            let slot = alloc_wide_zero(builder, nb);
            builder
                .ins()
                .store(MemFlagsData::trusted(), payload, slot, 0);
            slot
        };

        let addr = self.dynamic_elem_addr(context, builder)?;

        // Wide (>64-bit) bit-select: RMW the [end..=beg] range; else full value.
        let store_ptr = if let Some((beg, end)) = self.select {
            emit_wide_select_rmw(context, builder, addr, src_ptr, end, beg - end + 1, nb)
        } else {
            src_ptr
        };

        // Word-copy into the element, then mask the DESTINATION (not the
        // source, which may alias a flat-buffer read) to dst_width.
        for i in 0..n_words {
            let off = (i * 8) as i32;
            let val = builder.ins().load(I64, flags, store_ptr, off);
            builder.ins().store(flags, val, addr, off);
        }
        emit_wide_apply_mask(context, builder, addr, nb, self.dst_width);

        Some(())
    }

    /// Wide (>64-bit) FF-base full-element dynamic-indexed write (dcache
    /// line-wide RAM RMW) — the FF analogue of `build_binary_dynamic_wide`.
    /// Pushes a wide write-log entry at the element's current slot to commit,
    /// and (matching the dynamic interpret path) also stores into `dst_base` so
    /// in-event readers forward.  Full-element 2-state only; select /
    /// dynamic_select / rhs_select / 4-state bail.
    fn build_binary_dynamic_wide_ff(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        if context.use_4state
            || self.select.is_some()
            || self.rhs_select.is_some()
            || self.dynamic_select.is_some()
        {
            return None;
        }
        if !self.dst_base.is_ff() || self.dst_num_elements == 0 {
            return None;
        }
        let nb = calc_native_bytes(self.dst_width);
        let n_words = nb / 8;
        let flags = MemFlagsData::trusted();

        // Materialize the RHS into an nb-sized slot, zero-extending a narrower
        // wide source.
        let (payload, _mask_xz) = self.expr.build_binary(context, builder)?;
        let src_ptr = if returns_wide_pointer(&self.expr) {
            let src_nb = calc_native_bytes(self.expr.width());
            if src_nb == nb {
                payload
            } else {
                let slot = alloc_wide_zero(builder, nb);
                let copy_words = src_nb.min(nb) / 8;
                for i in 0..copy_words {
                    let off = (i * 8) as i32;
                    let w = builder.ins().load(I64, flags, payload, off);
                    builder.ins().store(flags, w, slot, off);
                }
                slot
            }
        } else {
            let slot = alloc_wide_zero(builder, nb);
            builder.ins().store(flags, payload, slot, 0);
            slot
        };
        // Mask the source to dst_width (the source may alias a flat read).
        emit_wide_apply_mask(context, builder, src_ptr, nb, self.dst_width);

        let (idx_payload, _) = self.dst_index_expr.build_binary(context, builder)?;
        let max_idx = builder
            .ins()
            .iconst(I64, (self.dst_num_elements as i64).saturating_sub(1));
        let in_bounds = builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, idx_payload, max_idx);
        let clamped = builder.ins().select(in_bounds, idx_payload, max_idx);
        let stride_val = builder.ins().iconst(I64, self.dst_stride as i64);
        let byte_offset = builder.ins().imul(clamped, stride_val);

        // Packed: skip the in-place store; the wide log push below delivers it
        // read-OLD (NBA). Not "idempotent with the log" — it landed mid-event, so a
        // same-event reader saw read-NEW. Unpacked keeps it for multi-RMW forwarding.
        let ff_is_packed = self.dst_base.raw() == self.dst_ff_current_base_offset;
        if !ff_is_packed {
            let base = builder.ins().iconst(I64, self.dst_base.raw() as i64);
            let addr = builder.ins().iadd(context.ff_values, base);
            let addr = builder.ins().iadd(addr, byte_offset);
            for i in 0..n_words {
                let off = (i * 8) as i32;
                let val = builder.ins().load(I64, flags, src_ptr, off);
                builder.ins().store(flags, val, addr, off);
            }
        }

        // Wide log push at the element's current slot.
        let cur_base = builder
            .ins()
            .iconst(I64, self.dst_ff_current_base_offset as i64);
        let log_base = builder.ins().iadd(cur_base, byte_offset);
        let log_base_i32 = builder.ins().ireduce(I32, log_base);
        emit_wide_log_chunks_dyn(context, builder, src_ptr, log_base_i32, nb);

        Some(())
    }
}
impl ProtoForStatement {
    pub fn can_build_binary(&self) -> bool {
        // A bound is JIT-able when it is a compile-time constant, or a runtime
        // expression that is itself JIT-able and fits an i64 counter (≤64 bits,
        // so its payload materialises as a scalar we can compare against).
        let bound_ok = |b: &ProtoForBound| match b {
            ProtoForBound::Const(_) => true,
            ProtoForBound::Dynamic(e) => e.can_build_binary() && e.width() <= 64,
        };
        let bounds_jittable = match &self.range {
            // Forward/Reverse advance by a fixed const step (≥1), so they always
            // make progress and terminate — safe with runtime bounds.
            ProtoForRange::Forward { start, end, .. }
            | ProtoForRange::Reverse { start, end, .. } => bound_ok(start) && bound_ok(end),
            // Stepped advances via an arbitrary op (e.g. `*= 2`), which can
            // stall (`0 * 2 == 0`) and spin forever.  The interpreter breaks on
            // a non-progressing step; const-bound stalls are rejected at
            // analysis, but runtime bounds have no such guard — so only JIT
            // const-bound Stepped loops (matching the pre-existing behaviour).
            ProtoForRange::Stepped { start, end, .. } => {
                matches!(start, ProtoForBound::Const(_)) && matches!(end, ProtoForBound::Const(_))
            }
        };
        bounds_jittable && self.body.iter().all(|s| s.can_build_binary())
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
        _is_last: bool,
    ) -> Option<()> {
        let header_block = builder.create_block();
        let body_block = builder.create_block();
        let exit_block = builder.create_block();

        let base_addr = if self.var_offset.is_ff() {
            context.ff_values
        } else {
            context.comb_values
        };
        let var_mem_offset = self.var_offset.raw() as i32;
        let nb = self.var_native_bytes;

        let (start_bound, end_bound, inclusive, step_val, is_reverse) = match &self.range {
            ProtoForRange::Forward {
                start,
                end,
                inclusive,
                step,
            } => (start, end, *inclusive, *step, false),
            ProtoForRange::Reverse {
                start,
                end,
                inclusive,
                step,
            } => (start, end, *inclusive, *step, true),
            ProtoForRange::Stepped {
                start,
                end,
                inclusive,
                step,
                ..
            } => (start, end, *inclusive, *step, false),
        };

        // Evaluate the loop bounds once, here in the entry block, matching the
        // interpreter which reads `r.start`/`r.end` a single time before
        // looping (`Statement::For` in `ir::statement`).  Const bounds fold to
        // `iconst`; dynamic bounds evaluate their expression to an i64.
        // `inclusive` bumps the end by one, as the const path did with `e += 1`.
        let start_v = Self::bound_value(start_bound, false, context, builder)?;
        let end_v = Self::bound_value(end_bound, inclusive, context, builder)?;

        // Reverse mirrors the emitted SV `for (int i = hi - 1; i >= lo;
        // i -= step)`. The signed `>= lo` guard makes underflow past `lo`
        // terminate, matching the signed `int i`.
        let init_i = if is_reverse {
            builder.ins().iadd_imm_s(end_v, -1)
        } else {
            start_v
        };

        // Carry the counter in a block param (a register), mirroring the
        // interpreter's local `i`: `var_mem` is written only at the top of the
        // body. So after the loop `var_mem` holds the last body value (or its
        // prior value for a zero-iteration loop), matching the interpreter,
        // not the exit value the counter reaches when the guard fails.
        builder.append_block_param(header_block, I64);
        builder.append_block_param(body_block, I64);

        builder.ins().jump(header_block, &[BlockArg::Value(init_i)]);

        context.load_cache.clear();
        builder.switch_to_block(header_block);

        let i_val = builder.block_params(header_block)[0];

        let cond = if is_reverse {
            // Sign-extend the counter from its native width so an underflow
            // below `lo` compares as negative, matching the signed `int i`.
            let i_signed = if nb <= 4 {
                let r = builder.ins().ireduce(I32, i_val);
                builder.ins().sextend(I64, r)
            } else {
                i_val
            };
            builder
                .ins()
                .icmp(IntCC::SignedGreaterThanOrEqual, i_signed, start_v)
        } else {
            builder.ins().icmp(IntCC::UnsignedLessThan, i_val, end_v)
        };
        builder
            .ins()
            .brif(cond, body_block, &[BlockArg::Value(i_val)], exit_block, &[]);

        context.load_cache.clear();
        let prev_store_elim = context.store_elim_enabled;
        context.store_elim_enabled = false;
        builder.switch_to_block(body_block);

        let i_cur = builder.block_params(body_block)[0];

        // Publish the counter to `var_mem` so the body reads the current `i`.
        Self::store_counter(context, builder, i_cur, base_addr, var_mem_offset, nb);
        context.load_cache.clear();

        for s in &self.body {
            s.build_binary(context, builder, false)?;
        }

        let step_c = builder.ins().iconst(I64, step_val as i64);
        let new_i = if is_reverse {
            builder.ins().isub(i_cur, step_c)
        } else {
            match &self.range {
                ProtoForRange::Stepped { op, .. } => match op {
                    // Mirror `Op::eval` (op.rs): the counter is unsigned, so
                    // right shifts and div/rem are unsigned. Bail (None) for
                    // any unmodeled op instead of silently doing `i + step`.
                    air::Op::Add => builder.ins().iadd(i_cur, step_c),
                    air::Op::Sub => builder.ins().isub(i_cur, step_c),
                    air::Op::Mul => builder.ins().imul(i_cur, step_c),
                    air::Op::Div => builder.ins().udiv(i_cur, step_c),
                    air::Op::Rem => builder.ins().urem(i_cur, step_c),
                    air::Op::BitAnd => builder.ins().band(i_cur, step_c),
                    air::Op::BitOr => builder.ins().bor(i_cur, step_c),
                    air::Op::BitXor => builder.ins().bxor(i_cur, step_c),
                    air::Op::LogicShiftL | air::Op::ArithShiftL => {
                        builder.ins().ishl(i_cur, step_c)
                    }
                    air::Op::LogicShiftR | air::Op::ArithShiftR => {
                        builder.ins().ushr(i_cur, step_c)
                    }
                    _ => return None,
                },
                _ => builder.ins().iadd(i_cur, step_c),
            }
        };

        builder.ins().jump(header_block, &[BlockArg::Value(new_i)]);

        context.load_cache.clear();
        context.store_elim_enabled = prev_store_elim;
        builder.switch_to_block(exit_block);

        Some(())
    }

    /// Materialise a loop bound as an i64 cranelift value.  Const bounds fold
    /// to `iconst`; dynamic bounds evaluate their (JIT-able, ≤64-bit) expression
    /// and normalise its payload to I64.  `add_one` applies the `inclusive`
    /// end-bound bump.  Callers gate on `can_build_binary`, so the payload is a
    /// scalar; the `None` arms are defensive (a type mismatch bails to the
    /// interpreter instead of panicking cranelift).
    fn bound_value(
        bound: &ProtoForBound,
        add_one: bool,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<CraneliftValue> {
        let raw = match bound {
            ProtoForBound::Const(c) => builder.ins().iconst(I64, *c as i64),
            ProtoForBound::Dynamic(expr) => {
                let (payload, _mask) = expr.build_binary(context, builder)?;
                let ty = builder.func.dfg.value_type(payload);
                if ty == I64 {
                    payload
                } else if ty == I128 {
                    builder.ins().ireduce(I64, payload)
                } else if ty.is_int() && ty.bits() < 64 {
                    builder.ins().uextend(I64, payload)
                } else {
                    return None;
                }
            }
        };
        Some(if add_one {
            builder.ins().iadd_imm_s(raw, 1)
        } else {
            raw
        })
    }

    fn store_counter(
        context: &CraneliftContext,
        builder: &mut FunctionBuilder,
        val_i64: cranelift::prelude::Value,
        base_addr: cranelift::prelude::Value,
        offset: i32,
        nb: usize,
    ) {
        if nb <= 4 {
            let v32 = builder.ins().ireduce(I32, val_i64);
            builder
                .ins()
                .store(MemFlagsData::trusted(), v32, base_addr, offset);
            if context.use_4state {
                let zero = builder.ins().iconst(I32, 0);
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), zero, base_addr, offset + nb as i32);
            }
        } else if nb <= 8 {
            builder
                .ins()
                .store(MemFlagsData::trusted(), val_i64, base_addr, offset);
            if context.use_4state {
                let zero = builder.ins().iconst(I64, 0);
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), zero, base_addr, offset + nb as i32);
            }
        } else {
            let v128 = builder.ins().uextend(I128, val_i64);
            builder
                .ins()
                .store(MemFlagsData::trusted(), v128, base_addr, offset);
            if context.use_4state {
                builder.ins().store(
                    MemFlagsData::trusted(),
                    context.zero_128,
                    base_addr,
                    offset + nb as i32,
                );
            }
        }
    }
}

impl ProtoStatement {
    pub fn can_build_binary(&self) -> bool {
        match self {
            ProtoStatement::Assign(x) => x.can_build_binary(),
            ProtoStatement::AssignDynamic(x) => x.can_build_binary(),
            ProtoStatement::If(x) => x.can_build_binary(),
            ProtoStatement::Case(x) => x.can_build_binary(),
            ProtoStatement::For(x) => x.can_build_binary(),
            ProtoStatement::SystemFunctionCall(_) => false,
            ProtoStatement::CompiledBlock(_) => false,
            ProtoStatement::SequentialBlock(body) => body.iter().all(|s| s.can_build_binary()),
            ProtoStatement::TbMethodCall { .. } => false,
            ProtoStatement::Break => false,
        }
    }
    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) -> Option<()> {
        match self {
            ProtoStatement::Assign(x) => x.build_binary(context, builder),
            ProtoStatement::AssignDynamic(x) => {
                let result = x.build_binary(context, builder);
                // Dynamic assigns write to runtime-computed addresses; invalidate all cached loads
                context.load_cache.clear();
                result
            }
            ProtoStatement::If(x) => x.build_binary(context, builder, is_last),
            ProtoStatement::Case(x) => x.build_binary(context, builder, is_last),
            ProtoStatement::For(x) => x.build_binary(context, builder, is_last),
            ProtoStatement::SystemFunctionCall(_) => None,
            ProtoStatement::CompiledBlock(_) => None,
            ProtoStatement::SequentialBlock(body) => {
                for (i, s) in body.iter().enumerate() {
                    s.build_binary(context, builder, is_last && i == body.len() - 1)?;
                }
                Some(())
            }
            ProtoStatement::TbMethodCall { .. } => None,
            ProtoStatement::Break => None,
        }
    }
}

impl ProtoAssignStatement {
    pub fn can_build_binary(&self) -> bool {
        if !self.expr.can_build_binary() {
            return false;
        }
        // The wide (>128-bit) flat-buffer store path can't sign-extend a
        // narrow signed RHS; run on the interpreter instead.
        if self.dst_width > 128
            && self.rhs_select.is_none()
            && self.expr.store_sign_extend_from(self.dst_width).is_some()
        {
            return false;
        }
        if let Some(dyn_sel) = &self.dynamic_select
            && (dyn_sel.elem_width * dyn_sel.num_elements > 128
                || !dyn_sel.index_expr.can_build_binary())
        {
            return false;
        }
        true
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        // Wide path: >128-bit destination
        if self.dst_width > 128 {
            return self.build_binary_wide(context, builder);
        }

        let known_bits = self.expr.effective_bits();
        let wide = self.dst_width > 64;

        // Plain store re-masks the payload to dst_width (istoreN truncation,
        // fwd_p, or the non-native band below), so the producer-side root
        // mask is redundant; select/dynamic_select shift it and need it.
        // Restricted to dst_width <= 64: wider dsts forward the unmasked
        // payload to the load cache (fwd_p only masks when dst_width < 64).
        let plain_root =
            self.select.is_none() && self.dynamic_select.is_none() && self.dst_width <= 64;
        let (mut payload, mut mask_xz) = if plain_root {
            self.expr.build_binary_root(context, builder)?
        } else {
            self.expr.build_binary(context, builder)?
        };
        let nb = calc_native_bytes(self.dst_width);
        let nb_i32 = nb as i32;

        // Apply rhs_select before the dst-width coercion, at the source's
        // natural representation: shifting after the I128→I64 truncation
        // would lose windows above bit 63, and a >128-bit source is a
        // pointer needing a word-window read.  A wide-pointer source with
        // no rhs_select is the implicit truncation `narrow = wide`, read
        // with window (dst_width-1, 0).
        let rhs_window = self.rhs_select.or_else(|| {
            if returns_wide_pointer(&self.expr) {
                Some((self.dst_width - 1, 0))
            } else {
                None
            }
        });
        if let Some((beg, end)) = rhs_window {
            let select_width = beg - end + 1;
            if returns_wide_pointer(&self.expr) {
                // Producers clamp windows to the source width, so these
                // reads stay inside its allocation.
                let read = |builder: &mut FunctionBuilder, ptr| {
                    if select_width <= 64 {
                        emit_wide_bit_select_read_narrow(builder, ptr, beg, end, select_width)
                    } else {
                        // 65-128-bit window: combine two ≤64-bit reads.
                        let lo = emit_wide_bit_select_read_narrow(builder, ptr, end + 63, end, 64);
                        let hi = emit_wide_bit_select_read_narrow(
                            builder,
                            ptr,
                            beg,
                            end + 64,
                            select_width - 64,
                        );
                        let lo = builder.ins().uextend(I128, lo);
                        let hi = builder.ins().uextend(I128, hi);
                        let hi = builder.ins().ishl_imm_u(hi, 64);
                        builder.ins().bor(lo, hi)
                    }
                };
                payload = read(builder, payload);
                if let Some(mxz) = mask_xz {
                    mask_xz = Some(read(builder, mxz));
                }
            } else {
                let slice = |builder: &mut FunctionBuilder, v| {
                    let bits = builder.func.dfg.value_type(v).bits() as usize;
                    if end >= bits {
                        // Window entirely above the source value: zero.
                        if bits > 64 {
                            iconst_128(builder, 0)
                        } else {
                            builder.ins().iconst(I64, 0)
                        }
                    } else {
                        let shifted = if end > 0 {
                            builder.ins().ushr_imm_u(v, end as i64)
                        } else {
                            v
                        };
                        band_const(
                            builder,
                            shifted,
                            gen_mask_for_width(select_width),
                            bits > 64,
                        )
                    }
                };
                payload = slice(builder, payload);
                if let Some(mxz) = mask_xz {
                    mask_xz = Some(slice(builder, mxz));
                }
            }
        }

        let known_bits = if let Some((beg, end)) = rhs_window {
            beg - end + 1
        } else {
            known_bits
        };

        // Coerce the (now always scalar) value to the dst representation.
        // Keyed on the value type, not expr.width(): a window read of a
        // wide source yields I64/I128 regardless of the declared width.
        if wide && builder.func.dfg.value_type(payload) != I128 {
            payload = builder.ins().uextend(I128, payload);
            if let Some(mxz) = mask_xz
                && builder.func.dfg.value_type(mxz) != I128
            {
                mask_xz = Some(builder.ins().uextend(I128, mxz));
            }
        } else if !wide && builder.func.dfg.value_type(payload) == I128 {
            // Narrow dst with a wide (I128) expression value: truncate to I64.
            payload = builder.ins().ireduce(I64, payload);
            if let Some(mxz) = mask_xz
                && builder.func.dfg.value_type(mxz) == I128
            {
                mask_xz = Some(builder.ins().ireduce(I64, mxz));
            }
        }

        // A bare narrow signed RHS sign-extends to the assignment width
        // (store_sign_extend_from; matches the interpreter and emitted SV).
        // Extend in-register, then re-mask so bits above dst_width stay clear.
        // A sign-extended mask_xz replicates an X sign bit, like Value::expand.
        if rhs_window.is_none()
            && let Some(from_w) = self.expr.store_sign_extend_from(self.dst_width)
        {
            let reg_bits: usize = if wide { 128 } else { 64 };
            let sh = (reg_bits - from_w) as i64;
            let mask = gen_mask_for_width(self.dst_width);
            let sext = |builder: &mut FunctionBuilder, v| {
                let v = builder.ins().ishl_imm_u(v, sh);
                let v = builder.ins().sshr_imm_u(v, sh);
                if self.dst_width < reg_bits {
                    band_const(builder, v, mask, wide)
                } else {
                    v
                }
            };
            payload = sext(builder, payload);
            if let Some(m) = mask_xz {
                mask_xz = Some(sext(builder, m));
            }
        }

        let load_mem_flag = MemFlagsData::trusted();
        let store_mem_flag = MemFlagsData::trusted();

        let base_addr = if self.dst.is_ff() {
            context.ff_values
        } else {
            context.comb_values
        };

        let dst_offset = self.dst.raw() as i32;
        let cache_key = self.dst;

        // FF write log push.  log_current_offset is always the canonical
        // FF current byte offset, regardless of whether `dst` points to
        // the current slot (packed layout, `dst.raw() == ff_current`) or
        // the next slot (dual-slot multi-RMW path, `dst.raw() == next`).
        // Use the canonical offset directly to make the layout choice
        // transparent to ff_commit_from_log.
        //
        // For packed FFs (`is_packed_ff`), the direct store and cache
        // insert are skipped — log-push-only path.  Multi-RMW FFs keep
        // the dual-slot dst=Ff(next) with cache forwarding for in-block
        // chain semantics.
        //
        // Wide FFs (65-128 bit, nb=16) emit a pair of 8-byte log entries
        // for the payload (and another pair for the 4-state mask).  See
        // the log push block below.
        let emit_log = self.dst.is_ff();
        let is_packed_ff = emit_log && (self.dst.raw() == self.dst_ff_current_offset);
        let log_current_offset = self.dst_ff_current_offset as i32;

        // Helpers covering nb ∈ {1, 2, 4, 8, 16} for the
        // dynamic_select / select / fallback paths below.  Use the fused
        // uload8/16/32 ops so narrow loads lower to a single movzbq.
        let load_native_to_native = |builder: &mut FunctionBuilder, off: i32| match nb {
            1 => builder.ins().uload8(I64, load_mem_flag, base_addr, off),
            2 => builder.ins().uload16(I64, load_mem_flag, base_addr, off),
            4 => builder.ins().uload32(load_mem_flag, base_addr, off),
            8 => builder.ins().load(I64, load_mem_flag, base_addr, off),
            _ => builder.ins().load(I128, load_mem_flag, base_addr, off),
        };
        let store_native_to_native =
            |builder: &mut FunctionBuilder, v: cranelift::prelude::Value, off: i32| match nb {
                1 => {
                    builder.ins().istore8(store_mem_flag, v, base_addr, off);
                }
                2 => {
                    builder.ins().istore16(store_mem_flag, v, base_addr, off);
                }
                4 => {
                    builder.ins().istore32(store_mem_flag, v, base_addr, off);
                }
                _ => {
                    builder.ins().store(store_mem_flag, v, base_addr, off);
                }
            };

        if let Some(dyn_sel) = &self.dynamic_select {
            let shift = build_dynamic_select_shift(dyn_sel, context, builder)?;

            let payload = builder.ins().ishl(payload, shift);

            // Dynamic mask: elem_mask << shift, then invert
            let elem_mask = gen_mask_for_width(dyn_sel.window);
            let mask_val = if wide {
                iconst_128(builder, elem_mask)
            } else {
                builder.ins().iconst(I64, elem_mask as i64)
            };
            let dyn_mask = builder.ins().ishl(mask_val, shift);
            let not_mask = builder.ins().bnot(dyn_mask);

            let (org_payload, org_mask_xz) = if !context.disable_load_cache
                && let Some(&(cached_p, cached_m)) = context.load_cache.get(&cache_key)
            {
                (cached_p, cached_m)
            } else {
                let p = load_native_to_native(builder, dst_offset);
                let m = if context.use_4state {
                    Some(load_native_to_native(builder, dst_offset + nb_i32))
                } else {
                    None
                };
                (p, m)
            };

            let org = builder.ins().band(org_payload, not_mask);
            let result = builder.ins().bor(payload, org);
            if !is_packed_ff {
                store_native_to_native(builder, result, dst_offset);
            }

            let result_mask_xz = if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl(mask_xz, shift);
                let z = if wide { context.zero_128 } else { context.zero };
                let org_m = org_mask_xz.unwrap_or(z);
                let org_m = builder.ins().band(org_m, not_mask);
                let result_m = builder.ins().bor(mask_xz, org_m);
                if !is_packed_ff {
                    store_native_to_native(builder, result_m, dst_offset + nb_i32);
                }
                Some(result_m)
            } else {
                None
            };

            // Mask the forwarded value to dst_width (see the plain store path);
            // wide non-native (64<w<128) needs it, full native widths are exempt.
            let needs_fwd_mask = self.dst_width != 64 && self.dst_width < 128;
            let fwd = if needs_fwd_mask {
                let m = gen_mask_for_width(self.dst_width);
                band_const(builder, result, m, wide)
            } else {
                result
            };
            let fwd_m = if needs_fwd_mask {
                result_mask_xz.map(|v| {
                    let m = gen_mask_for_width(self.dst_width);
                    band_const(builder, v, m, wide)
                })
            } else {
                result_mask_xz
            };
            if !is_packed_ff {
                context.load_cache.insert(cache_key, (fwd, fwd_m));
            }

            // dynamic_select RMW log push.  dyn_sel selects bit ranges
            // within a packed FF dst whose byte offset stays static.  Wide
            // (nb > 8) dsts must split the I128 `fwd` into per-word entries —
            // `emit_ff_log_push` handles both.  4-state pushes the mask_xz
            // portion at `log_current_offset + nb` (matches the storage layout
            // `[payload][mask]` produced by write_native_value).
            if emit_log {
                emit_ff_log_push(context, builder, log_current_offset, nb, fwd, fwd_m);
            }
        } else if let Some((beg, end)) = self.select {
            // Absolute-position masks for the [beg:end] window.  The shifted
            // payload is clipped to it (`pos_mask`) since the RHS, evaluated at
            // the assignment width (`s.f = ~x`), can carry bits above the field.
            let (pos_mask, not_mask) = if wide {
                let mask = gen_mask_range_128(beg, end);
                (iconst_128(builder, mask), iconst_128(builder, !mask))
            } else {
                let mask = ValueU64::gen_mask_range(beg, end);
                (
                    builder.ins().iconst(I64, mask as i64),
                    builder.ins().iconst(I64, !mask as i64),
                )
            };

            // Read-modify-write with native width
            let payload = builder.ins().ishl_imm_u(payload, end as i64);
            let payload = builder.ins().band(payload, pos_mask);

            // Use cached value if available, otherwise load from memory
            let (org_payload, org_mask_xz) = if !context.disable_load_cache
                && let Some(&(cached_p, cached_m)) = context.load_cache.get(&cache_key)
            {
                (cached_p, cached_m)
            } else {
                let p = load_native_to_native(builder, dst_offset);
                let m = if context.use_4state {
                    Some(load_native_to_native(builder, dst_offset + nb_i32))
                } else {
                    None
                };
                (p, m)
            };

            let org = builder.ins().band(org_payload, not_mask);
            let result = builder.ins().bor(payload, org);
            if !is_packed_ff {
                store_native_to_native(builder, result, dst_offset);
            }

            let result_mask_xz = if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm_u(mask_xz, end as i64);
                let mask_xz = builder.ins().band(mask_xz, pos_mask);
                let z = if wide { context.zero_128 } else { context.zero };
                let org_m = org_mask_xz.unwrap_or(z);
                let org_m = builder.ins().band(org_m, not_mask);
                let result_m = builder.ins().bor(mask_xz, org_m);
                if !is_packed_ff {
                    store_native_to_native(builder, result_m, dst_offset + nb_i32);
                }
                Some(result_m)
            } else {
                None
            };

            // Mask the forwarded value to dst_width (see the plain store path);
            // wide non-native (64<w<128) needs it, full native widths are exempt.
            let needs_fwd_mask = self.dst_width != 64 && self.dst_width < 128;
            let fwd = if needs_fwd_mask {
                let m = gen_mask_for_width(self.dst_width);
                band_const(builder, result, m, wide)
            } else {
                result
            };
            let fwd_m = if needs_fwd_mask {
                result_mask_xz.map(|v| {
                    let m = gen_mask_for_width(self.dst_width);
                    band_const(builder, v, m, wide)
                })
            } else {
                result_mask_xz
            };
            if !is_packed_ff {
                context.load_cache.insert(cache_key, (fwd, fwd_m));
            }

            // Select RMW log push.  `fwd` is the post-RMW payload masked
            // to dst_width — pushed as the cycle's update to the FF
            // current slot via `ff_commit_from_log`.  Wide (nb > 8) dsts split
            // the I128 `fwd` into per-word entries; 4-state appends a mask_xz
            // entry at `log_current_offset + nb`.  `emit_ff_log_push` handles
            // both.
            if emit_log {
                emit_ff_log_push(context, builder, log_current_offset, nb, fwd, fwd_m);
            }
        } else {
            // Store elimination relies on load_cache forwarding to
            // serve subsequent reads; skip only when the cache is live.
            // Packed FF also skips the direct store and cache insert
            // — the FF current slot is owned by ff_commit_from_log
            // replay, and an intra-cycle write would corrupt OLD-value
            // reads (NBA violation).
            let skip_store = (context.store_elim_enabled
                && !context.disable_load_cache
                && context.store_elim_offsets.contains(&cache_key))
                || is_packed_ff;

            // Writer-side mask is redundant when the post-rhs_select
            // payload is provably narrow-clean.  Two cases:
            //   - rhs_select absent: RHS expr is clean to dst_width
            //     (see ProtoExpression::is_clean_to_width).
            //   - rhs_select present: the rhs_select block above has
            //     already band_const'd payload to select_width, so the
            //     fwd mask is redundant when select_width <= dst_width.
            let post_rhs_clean_bits = if let Some((beg, end)) = self.rhs_select {
                Some(beg - end + 1)
            } else if self.expr.is_clean_to_width(self.dst_width) {
                Some(self.dst_width)
            } else {
                None
            };
            let skip_writer_mask = post_rhs_clean_bits
                .map(|w| w <= self.dst_width)
                .unwrap_or(false);

            if !skip_store {
                let needs_trunc = known_bits > self.dst_width;

                match self.dst_width {
                    8 => {
                        builder
                            .ins()
                            .istore8(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().istore8(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    16 => {
                        builder
                            .ins()
                            .istore16(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().istore16(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    32 => {
                        builder
                            .ins()
                            .istore32(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().istore32(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    64 => {
                        builder
                            .ins()
                            .store(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().store(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    128 => {
                        builder
                            .ins()
                            .store(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().store(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    _ => {
                        if self.dst_width > 128 {
                            return None;
                        }
                        // Mask is needed in general: effective_bits() reports
                        // declared width and misses carry-out from Add and
                        // friends.  Skip only when is_clean_to_width proves
                        // the RHS is already narrow.
                        let _ = needs_trunc;
                        let mask = gen_mask_for_width(self.dst_width);
                        let payload = if skip_writer_mask {
                            payload
                        } else {
                            band_const(builder, payload, mask, wide)
                        };
                        store_native_to_native(builder, payload, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            let mask = gen_mask_for_width(self.dst_width);
                            let mask_xz = if skip_writer_mask {
                                mask_xz
                            } else {
                                band_const(builder, mask_xz, mask, wide)
                            };
                            store_native_to_native(builder, mask_xz, dst_offset + nb_i32);
                        }
                    }
                }
            }

            // Mask the cached value to dst_width like the store arm masks
            // memory, else a later read forwards unmasked upper bits.  Wide
            // non-native (64<w<128) needs it too; full native widths (64/128)
            // store the whole slot and are exempt.
            let needs_fwd_mask = !skip_writer_mask && self.dst_width != 64 && self.dst_width < 128;
            let fwd_p = if needs_fwd_mask {
                let m = gen_mask_for_width(self.dst_width);
                band_const(builder, payload, m, wide)
            } else {
                payload
            };
            let fwd_m = if needs_fwd_mask {
                mask_xz.map(|v| {
                    let m = gen_mask_for_width(self.dst_width);
                    band_const(builder, v, m, wide)
                })
            } else {
                mask_xz
            };
            if !is_packed_ff {
                context.load_cache.insert(cache_key, (fwd_p, fwd_m));
            }

            // Write-log push (dual-write for unpacked FFs).  fwd_p is
            // the width-masked payload that matches what istoreN would
            // actually deposit.  Narrow FFs emit one entry per payload
            // (+ one mask entry for 4-state).  Wide FFs (nb > 8) split
            // the I128 payload into low/high u64 words, emitting one
            // entry per word — the commit-side consumer in
            // `ff_commit_from_log` writes width_class=8 bytes per entry.
            if emit_log {
                emit_ff_log_push(context, builder, log_current_offset, nb, fwd_p, fwd_m);
            }
        }

        Some(())
    }

    /// Wide (>128-bit) store: copy from expression pointer to destination memory.
    fn build_binary_wide(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        use super::helpers::emit_wide_apply_mask;

        // Wide-dst bit-select WRITE (RMW) in 4-state mode needs mask-aware
        // read-modify-write semantics; keep that on the interpreter (rare).
        // 2-state dst-select and any rhs_select are handled below.
        if self.select.is_some() && context.use_4state {
            return None;
        }

        // Fast path: ≤64-bit bit-select into a wide COMB value
        // (`wide_comb[hi:lo] <= narrow`). Collapse to a scalar 1-2 word RMW
        // instead of the full-width wide-op sequence — the dominant comb cost on
        // `--backend cranelift`. FF / `rhs_select` / `dynamic_select` / 4-state
        // keep the general wide path below.
        if !self.dst.is_ff()
            && self.dynamic_select.is_none()
            && self.rhs_select.is_none()
            && let Some((beg, end)) = self.select
            && (beg - end + 1) <= 64
        {
            let (raw, _mask_xz) = self.expr.build_binary(context, builder)?;
            // A `builds_wide_pointer` expr returns a pointer; the field is its
            // low word. A scalar IS that word. (cf. `wide_shift_amount`.)
            let sv = if returns_wide_pointer(&self.expr) {
                builder.ins().load(I64, MemFlagsData::trusted(), raw, 0)
            } else {
                raw
            };
            emit_wide_narrow_field_store(
                builder,
                context.comb_values,
                self.dst.raw() as i32,
                beg,
                end,
                self.dst_width,
                sv,
            );
            return Some(());
        }

        let expr_width = self.expr.width();
        let (payload, mask_xz) = self.expr.build_binary(context, builder)?;
        let nb = calc_native_bytes(self.dst_width);
        let n_words = nb / 8;
        let flags = MemFlagsData::trusted();

        let base_addr = if self.dst.is_ff() {
            context.ff_values
        } else {
            context.comb_values
        };
        let dst_offset = self.dst.raw() as i32;

        // FF write log emit for wide FFs.  Packed FFs (dst.raw() ==
        // dst_ff_current_offset) skip the direct store entirely so
        // intra-cycle reads return the OLD current value (NBA correct);
        // unpacked FFs (dst.raw() == next_offset) keep the direct store
        // for in-block chain forwarding via load_cache, then emit log
        // entries against the canonical current_offset so
        // `ff_commit_from_log` applies them at cycle end.
        let is_ff = self.dst.is_ff();
        let ff_packed = is_ff && (self.dst.raw() == self.dst_ff_current_offset);
        let log_current_offset = if is_ff {
            self.dst_ff_current_offset as i32
        } else {
            0
        };

        // Source representation: build_binary returns a POINTER iff
        // `builds_wide_pointer()` (the keystone predicate), NOT simply when
        // width > 128.  A wide-WIDTH expression that build_binary still
        // returns as a register — a comparison/reduction over wide operands,
        // or any node whose declared width exceeds its build representation —
        // must be promoted into a slot, else `payload` (a scalar) is
        // dereferenced as a pointer (SIGSEGV).  `is_wide_ptr(expr_width)`
        // alone gets this wrong and crashed the v4 OoO core's wide datapath.
        let src_ptr = if returns_wide_pointer(&self.expr) {
            payload
        } else {
            // Build produced a register (narrow or collapsed wide result):
            // force-store it into a fresh wide slot.  We do NOT call
            // ensure_wide_ptr_val here — its internal `is_wide_ptr(src_width)`
            // guard is fooled by an inflated `width` field (> 128 while the
            // build is actually a scalar) and would pass the register straight
            // through unstored, to be dereferenced as a pointer (SIGSEGV).
            let slot = alloc_wide_zero(builder, nb);
            builder
                .ins()
                .store(MemFlagsData::trusted(), payload, slot, 0);
            slot
        };

        // A wide-pointer source is sized to its expression width, which can be
        // narrower than the dst (`wide256 = concat129`); widen it so the
        // apply-mask / store / select-RMW below don't stride `nb` past its slot.
        // A wider source keeps its pointer (surplus high words ignored).
        let src_wide_nb = if returns_wide_pointer(&self.expr) {
            calc_native_bytes(expr_width)
        } else {
            nb // force-stored above into an nb-sized zeroed slot
        };
        let src_ptr = super::helpers::widen_wide_ptr(builder, src_ptr, src_wide_nb, nb);
        // `src_nb >= nb` after the widen; a wider source keeps its size so
        // `rhs_select`'s shift can reach the high bits it carries.
        let src_nb = src_wide_nb.max(nb);
        let extract_window = |context: &mut CraneliftContext,
                              builder: &mut FunctionBuilder,
                              ptr: CraneliftValue,
                              beg: usize,
                              end: usize| {
            use super::helpers::emit_wide_shift_right_mask;
            emit_wide_shift_right_mask(context, builder, ptr, end, beg - end + 1, src_nb)
        };
        let src_ptr = if let Some((beg, end)) = self.rhs_select {
            extract_window(context, builder, src_ptr, beg, end)
        } else {
            src_ptr
        };

        // select: `dst[beg:end] = src` — read the current dst value and
        // read-modify-write the [beg:end] range (2-state; 4-state bailed
        // above).  `src_ptr` becomes the full post-RMW wide value that the
        // store/log path below writes to the canonical dst.
        let src_ptr = if let Some((beg, end)) = self.select {
            use super::helpers::emit_wide_select_rmw;
            // Read the current dst value from `dst.raw()` (= `dst_offset`):
            // for comb that is the live value, for a packed FF the old
            // (last-cycle) current slot, and for an unpacked multi-RMW FF the
            // next slot that already holds prior in-event writes (forwarding).
            let old_ptr = builder.ins().iadd_imm_s(base_addr, dst_offset as i64);
            emit_wide_select_rmw(context, builder, old_ptr, src_ptr, end, beg - end + 1, nb)
        } else {
            src_ptr
        };

        // Apply width mask to the source to truncate extra bits
        emit_wide_apply_mask(context, builder, src_ptr, nb, self.dst_width);

        // Direct store (skip for packed FF — log push alone is enough).
        if !ff_packed {
            for i in 0..n_words {
                let off = (i * 8) as i32;
                let val = builder.ins().load(I64, flags, src_ptr, off);
                builder.ins().store(flags, val, base_addr, dst_offset + off);
            }
        }

        // FF write-log push: emit wide entries holding up to 56 bytes of
        // payload each.  For nb ≤ 56 a single entry suffices; wider FFs
        // chunk into multiple entries at canonical offsets.
        if is_ff {
            emit_wide_log_chunks(context, builder, src_ptr, log_current_offset, nb);
        }

        // 4-state mask: direct store (skip for packed FF) + parallel wide
        // log entries at `current_offset + nb`.
        if let Some(mask_xz) = mask_xz {
            let mask_ptr = if returns_wide_pointer(&self.expr) {
                mask_xz
            } else {
                // Force-store (see the payload src_ptr note above): the
                // width-field guard in ensure_wide_ptr_val is unreliable here.
                let slot = alloc_wide_zero(builder, nb);
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), mask_xz, slot, 0);
                slot
            };
            // Widen the mask half like the payload above.
            let mask_ptr = super::helpers::widen_wide_ptr(builder, mask_ptr, src_wide_nb, nb);
            // rhs_select shifts the mask half in parallel with the payload
            // (4-state).  dst-select RMW only reaches here in 2-state, where
            // there is no mask half, so it never combines with this block.
            let mask_ptr = if let Some((beg, end)) = self.rhs_select {
                extract_window(context, builder, mask_ptr, beg, end)
            } else {
                mask_ptr
            };
            emit_wide_apply_mask(context, builder, mask_ptr, nb, self.dst_width);
            if !ff_packed {
                for i in 0..n_words {
                    let off = (i * 8) as i32;
                    let val = builder.ins().load(I64, flags, mask_ptr, off);
                    builder
                        .ins()
                        .store(flags, val, base_addr, dst_offset + nb as i32 + off);
                }
            }
            if is_ff {
                emit_wide_log_chunks(
                    context,
                    builder,
                    mask_ptr,
                    log_current_offset + nb as i32,
                    nb,
                );
            }
        }

        Some(())
    }
}

impl ProtoIfStatement {
    pub fn can_build_binary(&self) -> bool {
        if let Some(cond) = &self.cond
            && !cond.can_build_binary()
        {
            return false;
        }
        self.true_side.iter().all(|s| s.can_build_binary())
            && self.false_side.iter().all(|s| s.can_build_binary())
    }
    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) -> Option<()> {
        // Cranelift's egraph does not synthesize jump tables itself, so
        // recognize Eq-const chains here and emit `br_table` directly.
        if std::env::var("VERYL_SWITCH_LOWER_DISABLE").ok().as_deref() != Some("1")
            && let Some(chain) = collect_eq_chain(self)
            && chain.arms.len() >= 4
            && let Some(()) = emit_switch_via_br_table(&chain, context, builder, is_last)
        {
            return Some(());
        }

        let true_block = builder.create_block();
        let false_block = builder.create_block();
        let final_block = builder.create_block();

        if cold_if_true_enabled() {
            builder.set_cold_block(true_block);
        }

        // Evaluate condition
        if let Some(x) = &self.cond {
            let (cond_payload, cond_mask_xz) = x.build_binary(context, builder)?;
            let effective_cond = if let Some(mask_xz) = cond_mask_xz {
                builder.ins().band_not(cond_payload, mask_xz)
            } else {
                cond_payload
            };
            builder
                .ins()
                .brif(effective_cond, true_block, &[], false_block, &[]);
        }

        context.load_cache.clear();
        // Disable store elimination inside If blocks: load_cache is cleared
        // at block boundaries, so eliminated stores would leave stale values.
        let prev_store_elim = context.store_elim_enabled;
        context.store_elim_enabled = false;

        builder.switch_to_block(true_block);
        let len = self.true_side.len();
        for (i, x) in self.true_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        context.load_cache.clear();

        builder.switch_to_block(false_block);
        let len = self.false_side.len();
        for (i, x) in self.false_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        builder.switch_to_block(final_block);

        context.load_cache.clear();
        context.store_elim_enabled = prev_store_elim;

        Some(())
    }
}

/// Scalar 1-2 word RMW of a ≤64-bit field `[lo..=hi]` of a wide value at
/// `base_addr + base_off` (`sv`'s low bits hold it), touching only the word(s)
/// the field spans instead of the full-width wide-op sequence — the dominant
/// `--backend cranelift` comb cost. Mirrors AOT-C's `emit_wide_narrow_field_store`.
/// 2-state, comb dst (no write-log); bits ≥ `dst_width` are clamped out.
fn emit_wide_narrow_field_store(
    builder: &mut FunctionBuilder,
    base_addr: cranelift::prelude::Value,
    base_off: i32,
    hi: usize,
    lo: usize,
    dst_width: usize,
    sv: cranelift::prelude::Value,
) {
    if lo >= dst_width {
        return; // whole field out of range → no-op
    }
    let hi = hi.min(dst_width - 1);
    let nbits = hi - lo + 1; // ≤ 64
    let flags = MemFlagsData::trusted();
    let k0 = lo / 64;
    let k1 = hi / 64;
    let b = (lo % 64) as i64;
    let base_mask: u64 = if nbits >= 64 {
        u64::MAX
    } else {
        (1u64 << nbits) - 1
    };
    // `sv` may be I128 (e.g. an unsized literal); the field is ≤64-bit so the
    // low word carries it.
    let sv = if builder.func.dfg.value_type(sv) == I128 {
        builder.ins().ireduce(I64, sv)
    } else {
        sv
    };
    if k0 == k1 {
        let m = base_mask << b;
        let off0 = base_off + (k0 * 8) as i32;
        let d = builder.ins().load(I64, flags, base_addr, off0);
        let cleared = builder.ins().band_imm_u(d, !m as i64);
        let shifted = builder.ins().ishl_imm_u(sv, b);
        let masked = builder.ins().band_imm_u(shifted, m as i64);
        let result = builder.ins().bor(cleared, masked);
        builder.ins().store(flags, result, base_addr, off0);
    } else {
        // Two adjacent words (k1 == k0+1).  b ∈ 1..=63 here: b==0 would keep a
        // ≤64-bit field within one word, so `sh = 64 - b` is never the UB `>>64`.
        let m0: u64 = u64::MAX << b;
        let hb = hi % 64;
        let m1: u64 = if hb >= 63 {
            u64::MAX
        } else {
            (1u64 << (hb + 1)) - 1
        };
        let sh = 64 - b;
        let off0 = base_off + (k0 * 8) as i32;
        let off1 = base_off + (k1 * 8) as i32;
        let d0 = builder.ins().load(I64, flags, base_addr, off0);
        let cleared0 = builder.ins().band_imm_u(d0, !m0 as i64);
        let shifted0 = builder.ins().ishl_imm_u(sv, b);
        let masked0 = builder.ins().band_imm_u(shifted0, m0 as i64);
        let result0 = builder.ins().bor(cleared0, masked0);
        builder.ins().store(flags, result0, base_addr, off0);

        let d1 = builder.ins().load(I64, flags, base_addr, off1);
        let cleared1 = builder.ins().band_imm_u(d1, !m1 as i64);
        let shifted1 = builder.ins().ushr_imm_u(sv, sh);
        let masked1 = builder.ins().band_imm_u(shifted1, m1 as i64);
        let result1 = builder.ins().bor(cleared1, masked1);
        builder.ins().store(flags, result1, base_addr, off1);
    }
}

/// Emit one or more wide write-log entries covering `nb` bytes starting
/// at `src_ptr`.  Each entry holds up to `WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES`
/// (56) bytes; wider FFs are split into multiple entries at consecutive
/// FF offsets.
fn emit_wide_log_chunks(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    src_ptr: cranelift::prelude::Value,
    base_offset: i32,
    nb: usize,
) {
    use crate::ir::write_log::WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES;
    let mut written: usize = 0;
    while written < nb {
        let chunk = std::cmp::min(WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES, nb - written);
        let entry_offset_val = builder
            .ins()
            .iconst(I32, (base_offset as i64) + written as i64);
        let chunk_ptr = if written == 0 {
            src_ptr
        } else {
            builder.ins().iadd_imm_s(src_ptr, written as i64)
        };
        emit_inline_write_log_push_wide(context, builder, entry_offset_val, chunk_ptr, chunk);
        written += chunk;
    }
}

/// Like `emit_wide_log_chunks` but the entry base offset is a runtime I32
/// `Value` (a dynamic-indexed FF element's current-slot offset), not a const.
fn emit_wide_log_chunks_dyn(
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    src_ptr: cranelift::prelude::Value,
    base_offset_val: cranelift::prelude::Value,
    nb: usize,
) {
    use crate::ir::write_log::WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES;
    let mut written: usize = 0;
    while written < nb {
        let chunk = std::cmp::min(WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES, nb - written);
        let entry_offset_val = if written == 0 {
            base_offset_val
        } else {
            builder.ins().iadd_imm_s(base_offset_val, written as i64)
        };
        let chunk_ptr = if written == 0 {
            src_ptr
        } else {
            builder.ins().iadd_imm_s(src_ptr, written as i64)
        };
        emit_inline_write_log_push_wide(context, builder, entry_offset_val, chunk_ptr, chunk);
        written += chunk;
    }
}
// ---------- Switch lowering helpers (br_table emission) ----------

struct EqChainArm<'a> {
    value: u64,
    body: &'a [ProtoStatement],
}

struct EqChain<'a> {
    selector: &'a ProtoExpression,
    arms: Vec<EqChainArm<'a>>,
    default: &'a [ProtoStatement],
}

fn extract_eq_const(cond: &ProtoExpression) -> Option<(&ProtoExpression, u64)> {
    let (x, op, y) = match cond {
        ProtoExpression::Binary { x, op, y, .. } => (x.as_ref(), *op, y.as_ref()),
        _ => return None,
    };
    if !matches!(
        op,
        veryl_analyzer::ir::Op::Eq | veryl_analyzer::ir::Op::EqWildcard
    ) {
        return None;
    }
    fn try_extract<'b>(
        val_side: &'b ProtoExpression,
        var_side: &'b ProtoExpression,
    ) -> Option<(&'b ProtoExpression, u64)> {
        match val_side {
            ProtoExpression::Value { value, .. } => {
                // xz constants would match any value under wildcard
                // semantics and cannot be encoded as a table index.
                if value.is_xz() {
                    None
                } else {
                    value.to_u64().map(|v| (var_side, v))
                }
            }
            _ => None,
        }
    }
    if let Some(r) = try_extract(y, x) {
        return Some(r);
    }
    try_extract(x, y)
}

fn same_var_read(a: &ProtoExpression, b: &ProtoExpression) -> bool {
    match (a, b) {
        (
            ProtoExpression::Variable {
                var_offset: oa,
                select: sa,
                dynamic_select: dsa,
                ..
            },
            ProtoExpression::Variable {
                var_offset: ob,
                select: sb,
                dynamic_select: dsb,
                ..
            },
        ) => oa == ob && sa == sb && dsa.is_none() && dsb.is_none(),
        _ => false,
    }
}

fn collect_eq_chain(start: &ProtoIfStatement) -> Option<EqChain<'_>> {
    let mut arms: Vec<EqChainArm<'_>> = Vec::new();
    let mut selector: Option<&ProtoExpression> = None;
    let mut current = start;
    // Else-chain once the eq-const prefix stops: the *last consumed* arm's
    // false_side. A dirty break descends into a non-eq-const / different-selector
    // node that must stay whole as the default, not be reduced to its false_side.
    let mut default: &[ProtoStatement] = &[];
    loop {
        let cond = current.cond.as_ref()?;
        let (var_expr, const_val) = match extract_eq_const(cond) {
            Some(p) => p,
            None => break,
        };
        // Dynamic-indexed reads cannot share a single hoisted dispatch
        // value across arms, so they break the chain.
        if let ProtoExpression::Variable {
            dynamic_select: Some(_),
            ..
        } = var_expr
        {
            break;
        }
        if let Some(sel) = selector {
            if !same_var_read(sel, var_expr) {
                break;
            }
        } else {
            selector = Some(var_expr);
        }
        arms.push(EqChainArm {
            value: const_val,
            body: &current.true_side,
        });
        default = &current.false_side[..];
        if current.false_side.len() == 1
            && let ProtoStatement::If(next) = &current.false_side[0]
        {
            current = next;
            continue;
        }
        break;
    }
    if arms.len() < 2 {
        return None;
    }
    Some(EqChain {
        selector: selector?,
        arms,
        default,
    })
}

fn emit_switch_via_br_table(
    chain: &EqChain,
    context: &mut CraneliftContext,
    builder: &mut FunctionBuilder,
    is_last: bool,
) -> Option<()> {
    use cranelift::codegen::ir::JumpTableData;

    // Cap table size to bound emission cost / i-cache footprint.
    const SWITCH_LOWER_LIMIT: u64 = 256;
    let max = chain.arms.iter().map(|a| a.value).max()?;
    if max >= SWITCH_LOWER_LIMIT {
        return None;
    }

    // Strip mask_xz so xz bits don't perturb the table index.
    let (sel_payload, sel_mask_xz) = chain.selector.build_binary(context, builder)?;
    let sel_clean = if let Some(mask) = sel_mask_xz {
        builder.ins().band_not(sel_payload, mask)
    } else {
        sel_payload
    };
    // `br_table` requires an I32 selector. Inputs may arrive as I64 (the
    // default scalar size) or I128 (wide selector); coerce.
    let sel_ty = builder.func.dfg.value_type(sel_clean);
    let sel_clean = if sel_ty == I32 {
        sel_clean
    } else if sel_ty.bits() > 32 {
        builder.ins().ireduce(I32, sel_clean)
    } else {
        builder.ins().uextend(I32, sel_clean)
    };

    let arm_blocks: Vec<_> = chain.arms.iter().map(|_| builder.create_block()).collect();
    let default_block = builder.create_block();
    let final_block = builder.create_block();

    let table_size = (max as usize) + 1;
    let mut entries: Vec<_> = (0..table_size)
        .map(|_| builder.func.dfg.block_call(default_block, &[]))
        .collect();
    // Reverse so a duplicate value resolves to the first arm: entries is
    // last-write-wins, and `case` is first-match.
    for (i, arm) in chain.arms.iter().enumerate().rev() {
        entries[arm.value as usize] = builder.func.dfg.block_call(arm_blocks[i], &[]);
    }
    let default_call = builder.func.dfg.block_call(default_block, &[]);
    let jt_data = JumpTableData::new(default_call, &entries);
    let jt = builder.func.create_jump_table(jt_data);
    builder.ins().br_table(sel_clean, jt);

    // load_cache is cleared at block boundaries, so an elided store
    // would leak a stale value into a sibling arm.
    let prev_store_elim = context.store_elim_enabled;
    context.store_elim_enabled = false;

    for (i, arm) in chain.arms.iter().enumerate() {
        builder.switch_to_block(arm_blocks[i]);
        context.load_cache.clear();
        let len = arm.body.len();
        for (j, s) in arm.body.iter().enumerate() {
            let last = is_last && (j + 1 == len);
            s.build_binary(context, builder, last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }
    }

    builder.switch_to_block(default_block);
    context.load_cache.clear();
    let len = chain.default.len();
    for (j, s) in chain.default.iter().enumerate() {
        let last = is_last && (j + 1 == len);
        s.build_binary(context, builder, last)?;
    }
    if is_last {
        builder.ins().return_(&[]);
    } else {
        builder.ins().jump(final_block, &[]);
    }

    builder.switch_to_block(final_block);
    context.load_cache.clear();
    context.store_elim_enabled = prev_store_elim;

    Some(())
}

/// Extract the constant selector values from a `case` arm condition — succeeds
/// only for `sel == const` leaves OR-combined against ONE non-dynamic selector,
/// so a range/casez/mixed-selector leaf rejects it (→ comparison cascade).
fn extract_case_eq_values<'a>(
    cond: &'a ProtoExpression,
    selector: &mut Option<&'a ProtoExpression>,
    out: &mut Vec<u64>,
) -> bool {
    if let ProtoExpression::Binary {
        x,
        op: veryl_analyzer::ir::Op::LogicOr,
        y,
        ..
    } = cond
    {
        return extract_case_eq_values(x, selector, out)
            && extract_case_eq_values(y, selector, out);
    }
    let Some((var_expr, const_val)) = extract_eq_const(cond) else {
        return false;
    };
    if let ProtoExpression::Variable {
        dynamic_select: Some(_),
        ..
    } = var_expr
    {
        return false;
    }
    match selector {
        Some(sel) if !same_var_read(sel, var_expr) => return false,
        Some(_) => {}
        None => *selector = Some(var_expr),
    }
    out.push(const_val);
    true
}

impl ProtoCaseStatement {
    pub fn can_build_binary(&self) -> bool {
        self.arms
            .iter()
            .all(|arm| arm.cond.can_build_binary() && arm.body.iter().all(|s| s.can_build_binary()))
            && self.default.iter().all(|s| s.can_build_binary())
    }

    /// Reshape the flat arms into an `EqChain` (one entry per matched value) so
    /// the shared `br_table` emitter can be reused; `None` if not all eq-const.
    fn as_eq_chain(&self) -> Option<EqChain<'_>> {
        let mut selector: Option<&ProtoExpression> = None;
        let mut arms: Vec<EqChainArm<'_>> = Vec::new();
        for arm in &self.arms {
            let mut values = Vec::new();
            if !extract_case_eq_values(&arm.cond, &mut selector, &mut values) {
                return None;
            }
            for value in values {
                arms.push(EqChainArm {
                    value,
                    body: &arm.body,
                });
            }
        }
        Some(EqChain {
            selector: selector?,
            arms,
            default: &self.default,
        })
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) -> Option<()> {
        // Dense eq-const arms → a real jump table (as the nested-if path did).
        if std::env::var("VERYL_SWITCH_LOWER_DISABLE").ok().as_deref() != Some("1")
            && let Some(chain) = self.as_eq_chain()
            && chain.arms.len() >= 4
            && let Some(()) = emit_switch_via_br_table(&chain, context, builder, is_last)
        {
            return Some(());
        }
        // Otherwise a comparison cascade, built iteratively (only into bodies).
        let final_block = builder.create_block();
        let prev_store_elim = context.store_elim_enabled;
        // load_cache is cleared at block boundaries; an elided store would
        // leak a stale value into a sibling arm.
        context.store_elim_enabled = false;

        for arm in &self.arms {
            let body_block = builder.create_block();
            let next_block = builder.create_block();

            let (cond_payload, cond_mask_xz) = arm.cond.build_binary(context, builder)?;
            let effective_cond = if let Some(mask_xz) = cond_mask_xz {
                builder.ins().band_not(cond_payload, mask_xz)
            } else {
                cond_payload
            };
            builder
                .ins()
                .brif(effective_cond, body_block, &[], next_block, &[]);

            builder.switch_to_block(body_block);
            context.load_cache.clear();
            let len = arm.body.len();
            for (i, s) in arm.body.iter().enumerate() {
                let last = is_last && (i + 1 == len);
                s.build_binary(context, builder, last)?;
            }
            if is_last {
                builder.ins().return_(&[]);
            } else {
                builder.ins().jump(final_block, &[]);
            }

            builder.switch_to_block(next_block);
            context.load_cache.clear();
        }

        let len = self.default.len();
        for (i, s) in self.default.iter().enumerate() {
            let last = is_last && (i + 1 == len);
            s.build_binary(context, builder, last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        builder.switch_to_block(final_block);
        context.load_cache.clear();
        context.store_elim_enabled = prev_store_elim;

        Some(())
    }
}
// PGO cold-block heuristic (env-gated, default ON).  Cranelift
// `set_cold_block` lays the marked block out-of-line and biases static
// branch prediction toward the hot path.  Marking the if-true side
// cold wins on designs whose `if (rare_guard) { ... }` patterns
// dominate (true_side is the rare path); opt out via
// `VERYL_COLD_IF_TRUE=0`.
fn cold_if_true_enabled() -> bool {
    use std::sync::OnceLock;
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("VERYL_COLD_IF_TRUE").ok().as_deref() != Some("0"))
}
#[cfg(test)]
mod eq_chain_tests {
    use super::*;
    use crate::ir::{ExpressionContext, Value, VarOffset};
    use veryl_analyzer::ir::Op;
    use veryl_analyzer::value::ValueU64;

    fn ctx(width: usize) -> ExpressionContext {
        ExpressionContext {
            width,
            signed: false,
        }
    }
    fn var_expr(off: isize, width: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: None,
            dynamic_select: None,
            width,
            var_full_width: width,
            expr_context: ctx(width),
        }
    }
    fn const_expr(payload: u64) -> ProtoExpression {
        ProtoExpression::Value {
            value: Value::U64(ValueU64 {
                payload,
                mask_xz: 0,
                width: 8,
                signed: false,
            }),
            width: 8,
            expr_context: ctx(8),
        }
    }
    fn cmp(off: isize, op: Op, val: u64) -> ProtoExpression {
        ProtoExpression::Binary {
            x: Box::new(var_expr(off, 8)),
            op,
            y: Box::new(const_expr(val)),
            width: 1,
            expr_context: ctx(1),
        }
    }
    fn if_node(
        cond: ProtoExpression,
        t: Vec<ProtoStatement>,
        f: Vec<ProtoStatement>,
    ) -> ProtoIfStatement {
        ProtoIfStatement {
            cond: Some(cond),
            true_side: t,
            false_side: f,
        }
    }
    fn eq_prefix(sel: isize, tail: Vec<ProtoStatement>) -> ProtoIfStatement {
        let mut chain = tail;
        for v in (0..4u64).rev() {
            chain = vec![ProtoStatement::If(if_node(
                cmp(sel, Op::Eq, v),
                vec![ProtoStatement::Break],
                chain,
            ))];
        }
        match chain.into_iter().next().unwrap() {
            ProtoStatement::If(x) => x,
            _ => unreachable!(),
        }
    }

    // A node that ends the eq-const prefix because it tests a *different*
    // selector must survive whole — its cond and true_side — inside the
    // chain default.
    #[test]
    fn dirty_break_keeps_node_in_default_selector_mismatch() {
        let dirty = ProtoStatement::If(if_node(
            cmp(0x20, Op::Eq, 5),
            vec![ProtoStatement::Break],
            vec![ProtoStatement::Break],
        ));
        let head = eq_prefix(0x10, vec![dirty]);
        let chain = collect_eq_chain(&head).expect("4-arm eq chain");
        assert_eq!(
            chain.arms.iter().map(|a| a.value).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(chain.default.len(), 1);
        assert!(
            matches!(chain.default[0], ProtoStatement::If(_)),
            "default must retain the whole different-selector node"
        );
    }

    // Same, but the prefix ends on a non-Eq operator (extract_eq_const fails).
    #[test]
    fn dirty_break_keeps_node_in_default_non_eq_op() {
        let dirty = ProtoStatement::If(if_node(
            cmp(0x10, Op::Less, 4),
            vec![ProtoStatement::Break],
            vec![ProtoStatement::Break],
        ));
        let head = eq_prefix(0x10, vec![dirty]);
        let chain = collect_eq_chain(&head).expect("4-arm eq chain");
        assert_eq!(chain.arms.len(), 4);
        assert_eq!(chain.default.len(), 1);
        assert!(matches!(chain.default[0], ProtoStatement::If(_)));
    }

    // A clean chain whose final else is plain statements keeps that else as
    // the default (regression guard for the common case).
    #[test]
    fn clean_chain_default_is_final_else() {
        let head = eq_prefix(0x10, vec![ProtoStatement::Break]);
        let chain = collect_eq_chain(&head).expect("4-arm eq chain");
        assert_eq!(chain.arms.len(), 4);
        assert_eq!(chain.default.len(), 1);
        assert!(matches!(chain.default[0], ProtoStatement::Break));
    }
}
