//! Cranelift codegen impl blocks for `ProtoStatement` and friends,
//! sibling to the IR-side impls in `crate::ir::statement`.

use super::helpers::*;
use super::runtime::{
    Context as CraneliftContext, emit_inline_write_log_push, emit_inline_write_log_push_wide,
};
use crate::ir::variable::native_bytes_for as calc_native_bytes_for;
use crate::ir::{
    ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoExpression, ProtoForRange,
    ProtoForStatement, ProtoIfStatement, ProtoStatement,
};
use cranelift::prelude::types::{I32, I64, I128};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::ir as air;
use veryl_analyzer::value::ValueU64;

impl ProtoAssignDynamicStatement {
    pub fn can_build_binary(&self) -> bool {
        if !self.expr.can_build_binary() || !self.dst_index_expr.can_build_binary() {
            return false;
        }
        if let Some(dyn_sel) = &self.dynamic_select {
            let full_width = dyn_sel.elem_width * dyn_sel.num_elements;
            if full_width > 128 || !dyn_sel.index_expr.can_build_binary() {
                return false;
            }
            full_width <= 64
        } else {
            self.dst_width <= 64
        }
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        let (mut payload, mut mask_xz) = self.expr.build_binary(context, builder)?;
        let nb = calc_native_bytes_for(self.dst_width, self.dst_base.is_ff());
        let nb_i32 = nb as i32;

        if let Some((beg, end)) = self.rhs_select {
            let mask = ValueU64::gen_mask(beg - end + 1);

            payload = builder.ins().ushr_imm(payload, end as i64);
            payload = builder.ins().band_imm(payload, mask as i64);

            if let Some(mxz) = mask_xz {
                let mxz = builder.ins().ushr_imm(mxz, end as i64);
                let mxz = builder.ins().band_imm(mxz, mask as i64);
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

        let load_mem_flag = MemFlags::trusted();
        let store_mem_flag = MemFlags::trusted();

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
                    let mask_offset_val = builder.ins().iadd_imm(offset_val, nb as i64);
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

            let elem_mask = gen_mask_for_width(dyn_sel.elem_width);
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

            let payload = builder.ins().ishl_imm(payload, end as i64);
            let org = load_native_to_i64(builder, addr, 0);
            let org = builder.ins().band_imm(org, !mask as i64);
            let result = builder.ins().bor(payload, org);
            if !is_packed_ff_dyn {
                store_i64_to_native(builder, result, addr, 0);
            }
            let mask_result = if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm(mask_xz, end as i64);
                let org = load_native_to_i64(builder, addr, nb_i32);
                let org = builder.ins().band_imm(org, !mask as i64);
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
                    let masked = builder.ins().band_imm(payload, mask as i64);
                    (masked, masked)
                }
            };
            let mask_xz_for_log = if let Some(mask_xz_v) = mask_xz {
                let m = if !matches!(self.dst_width, 8 | 16 | 32 | 64) {
                    let mask = (1u64 << self.dst_width) - 1;
                    builder.ins().band_imm(mask_xz_v, mask as i64)
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
}
impl ProtoForStatement {
    pub fn can_build_binary(&self) -> bool {
        self.range.is_const() && self.body.iter().all(|s| s.can_build_binary())
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

        let (start_val, end_val, step_val, is_reverse) = match &self.range {
            ProtoForRange::Forward {
                start,
                end,
                inclusive,
                step,
            } => {
                let mut e = end.as_const()?;
                if *inclusive {
                    e += 1;
                }
                (start.as_const()?, e, *step, false)
            }
            ProtoForRange::Reverse {
                start,
                end,
                inclusive,
                step,
            } => {
                let mut e = end.as_const()?;
                if *inclusive {
                    e += 1;
                }
                (start.as_const()?, e, *step, true)
            }
            ProtoForRange::Stepped {
                start,
                end,
                inclusive,
                step,
                ..
            } => {
                let mut e = end.as_const()?;
                if *inclusive {
                    e += 1;
                }
                (start.as_const()?, e, *step, false)
            }
        };

        let init_i = if is_reverse {
            builder.ins().iconst(I64, end_val as i64)
        } else {
            builder.ins().iconst(I64, start_val as i64)
        };

        Self::store_counter(context, builder, init_i, base_addr, var_mem_offset, nb);

        builder.ins().jump(header_block, &[]);

        context.load_cache.clear();
        builder.switch_to_block(header_block);

        let i_val = Self::load_counter_as_i64(builder, base_addr, var_mem_offset, nb);

        let cond = if is_reverse {
            let start_const = builder.ins().iconst(I64, start_val as i64);
            builder
                .ins()
                .icmp(IntCC::UnsignedGreaterThan, i_val, start_const)
        } else {
            let end_const = builder.ins().iconst(I64, end_val as i64);
            builder
                .ins()
                .icmp(IntCC::UnsignedLessThan, i_val, end_const)
        };
        builder.ins().brif(cond, body_block, &[], exit_block, &[]);

        context.load_cache.clear();
        let prev_store_elim = context.store_elim_enabled;
        context.store_elim_enabled = false;
        builder.switch_to_block(body_block);

        if is_reverse {
            let i_cur = Self::load_counter_as_i64(builder, base_addr, var_mem_offset, nb);
            let step_c = builder.ins().iconst(I64, step_val as i64);
            let new_i = builder.ins().isub(i_cur, step_c);
            Self::store_counter(context, builder, new_i, base_addr, var_mem_offset, nb);
        }

        for s in &self.body {
            s.build_binary(context, builder, false)?;
        }

        if !is_reverse {
            let i_cur = Self::load_counter_as_i64(builder, base_addr, var_mem_offset, nb);
            let step_c = builder.ins().iconst(I64, step_val as i64);
            let new_i = match &self.range {
                ProtoForRange::Stepped { op, .. } => match op {
                    air::Op::Mul => builder.ins().imul(i_cur, step_c),
                    air::Op::LogicShiftL => builder.ins().ishl(i_cur, step_c),
                    _ => builder.ins().iadd(i_cur, step_c),
                },
                _ => builder.ins().iadd(i_cur, step_c),
            };
            Self::store_counter(context, builder, new_i, base_addr, var_mem_offset, nb);
        }

        builder.ins().jump(header_block, &[]);

        context.load_cache.clear();
        context.store_elim_enabled = prev_store_elim;
        builder.switch_to_block(exit_block);

        Some(())
    }

    fn load_counter_as_i64(
        builder: &mut FunctionBuilder,
        base_addr: cranelift::prelude::Value,
        offset: i32,
        nb: usize,
    ) -> cranelift::prelude::Value {
        if nb <= 4 {
            builder
                .ins()
                .uload32(MemFlags::trusted(), base_addr, offset)
        } else if nb <= 8 {
            builder
                .ins()
                .load(I64, MemFlags::trusted(), base_addr, offset)
        } else {
            let v = builder
                .ins()
                .load(I128, MemFlags::trusted(), base_addr, offset);
            builder.ins().ireduce(I64, v)
        }
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
                .store(MemFlags::trusted(), v32, base_addr, offset);
            if context.use_4state {
                let zero = builder.ins().iconst(I32, 0);
                builder
                    .ins()
                    .store(MemFlags::trusted(), zero, base_addr, offset + nb as i32);
            }
        } else if nb <= 8 {
            builder
                .ins()
                .store(MemFlags::trusted(), val_i64, base_addr, offset);
            if context.use_4state {
                let zero = builder.ins().iconst(I64, 0);
                builder
                    .ins()
                    .store(MemFlags::trusted(), zero, base_addr, offset + nb as i32);
            }
        } else {
            let v128 = builder.ins().uextend(I128, val_i64);
            builder
                .ins()
                .store(MemFlags::trusted(), v128, base_addr, offset);
            if context.use_4state {
                builder.ins().store(
                    MemFlags::trusted(),
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

        let (mut payload, mut mask_xz) = self.expr.build_binary(context, builder)?;
        let nb = calc_native_bytes_for(self.dst_width, self.dst.is_ff());
        let nb_i32 = nb as i32;

        // Widen expression result to I128 for a 128-bit destination,
        // skipping if already I128 (unsized all_bit literal).
        if wide && self.expr.width() <= 64 && builder.func.dfg.value_type(payload) != I128 {
            payload = builder.ins().uextend(I128, payload);
            if let Some(mxz) = mask_xz
                && builder.func.dfg.value_type(mxz) != I128
            {
                mask_xz = Some(builder.ins().uextend(I128, mxz));
            }
        }

        // Narrow known_bits if rhs_select is applied
        let known_bits = if let Some((beg, end)) = self.rhs_select {
            beg - end + 1
        } else {
            known_bits
        };

        if let Some((beg, end)) = self.rhs_select {
            let select_width = beg - end + 1;
            let mask = gen_mask_for_width(select_width);

            payload = builder.ins().ushr_imm(payload, end as i64);
            payload = band_const(builder, payload, mask, wide);

            if let Some(mxz) = mask_xz {
                let mxz = builder.ins().ushr_imm(mxz, end as i64);
                let mxz = band_const(builder, mxz, mask, wide);
                mask_xz = Some(mxz);
            }
        }

        let load_mem_flag = MemFlags::trusted();
        let store_mem_flag = MemFlags::trusted();

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
            let elem_mask = gen_mask_for_width(dyn_sel.elem_width);
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

            // Mask forwarded value to dst_width to match load-path uextend
            let fwd = if self.dst_width < 64 {
                let m = gen_mask_for_width(self.dst_width);
                band_const(builder, result, m, false)
            } else {
                result
            };
            let fwd_m = if self.dst_width < 64 {
                result_mask_xz.map(|v| {
                    let m = gen_mask_for_width(self.dst_width);
                    band_const(builder, v, m, false)
                })
            } else {
                result_mask_xz
            };
            if !is_packed_ff {
                context.load_cache.insert(cache_key, (fwd, fwd_m));
            }

            // dynamic_select RMW log push.  dyn_sel selects bit ranges
            // within a packed FF dst whose byte offset stays static, so
            // `event_write_log_push_static` is fine.  4-state pushes a
            // second entry for the mask_xz portion at
            // `log_current_offset + nb` (matches the storage layout
            // `[payload][mask]` produced by write_native_value).
            if emit_log {
                let offset_val = builder.ins().iconst(I32, log_current_offset as i64);
                let width_class_val = builder.ins().iconst(I32, nb as i64);
                emit_inline_write_log_push(context, builder, offset_val, fwd, width_class_val);
                if context.use_4state
                    && let Some(fwd_m_v) = fwd_m
                {
                    let mask_offset_val = builder
                        .ins()
                        .iconst(I32, (log_current_offset + nb_i32) as i64);
                    emit_inline_write_log_push(
                        context,
                        builder,
                        mask_offset_val,
                        fwd_m_v,
                        width_class_val,
                    );
                }
            }
        } else if let Some((beg, end)) = self.select {
            // Read-modify-write with native width
            let payload = builder.ins().ishl_imm(payload, end as i64);

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

            let not_mask = if wide {
                let mask = gen_mask_range_128(beg, end);
                iconst_128(builder, !mask)
            } else {
                let mask = ValueU64::gen_mask_range(beg, end);
                builder.ins().iconst(I64, !mask as i64)
            };
            let org = builder.ins().band(org_payload, not_mask);
            let result = builder.ins().bor(payload, org);
            if !is_packed_ff {
                store_native_to_native(builder, result, dst_offset);
            }

            let result_mask_xz = if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm(mask_xz, end as i64);
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

            // Mask forwarded value to dst_width to match load-path uextend
            let fwd = if self.dst_width < 64 {
                let m = gen_mask_for_width(self.dst_width);
                band_const(builder, result, m, false)
            } else {
                result
            };
            let fwd_m = if self.dst_width < 64 {
                result_mask_xz.map(|v| {
                    let m = gen_mask_for_width(self.dst_width);
                    band_const(builder, v, m, false)
                })
            } else {
                result_mask_xz
            };
            if !is_packed_ff {
                context.load_cache.insert(cache_key, (fwd, fwd_m));
            }

            // Select RMW log push.  `fwd` is the post-RMW payload masked
            // to dst_width — pushed as the cycle's update to the FF
            // current slot via `ff_commit_from_log`.  4-state appends
            // a mask_xz entry at `log_current_offset + nb`.
            if emit_log {
                let offset_val = builder.ins().iconst(I32, log_current_offset as i64);
                let width_class_val = builder.ins().iconst(I32, nb as i64);
                emit_inline_write_log_push(context, builder, offset_val, fwd, width_class_val);
                if context.use_4state
                    && let Some(fwd_m_v) = fwd_m
                {
                    let mask_offset_val = builder
                        .ins()
                        .iconst(I32, (log_current_offset + nb_i32) as i64);
                    emit_inline_write_log_push(
                        context,
                        builder,
                        mask_offset_val,
                        fwd_m_v,
                        width_class_val,
                    );
                }
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

            // Forward value to load cache.  Mask to dst_width to match
            // load+uextend, with the same elision as the storage mask above.
            // Packed FF skips the cache insert so subsequent same-cycle
            // FF reads load OLD value from memory (NBA semantics).
            let fwd_p = if self.dst_width < 64 && !skip_writer_mask {
                let m = gen_mask_for_width(self.dst_width);
                band_const(builder, payload, m, false)
            } else {
                payload
            };
            let fwd_m = if self.dst_width < 64 && !skip_writer_mask {
                mask_xz.map(|v| {
                    let m = gen_mask_for_width(self.dst_width);
                    band_const(builder, v, m, false)
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
                if nb <= 8 {
                    let offset_val = builder.ins().iconst(I32, log_current_offset as i64);
                    let width_class_val = builder.ins().iconst(I32, nb as i64);
                    emit_inline_write_log_push(
                        context,
                        builder,
                        offset_val,
                        fwd_p,
                        width_class_val,
                    );
                    if context.use_4state
                        && let Some(fwd_m_v) = fwd_m
                    {
                        let mask_offset_val = builder
                            .ins()
                            .iconst(I32, (log_current_offset + nb_i32) as i64);
                        emit_inline_write_log_push(
                            context,
                            builder,
                            mask_offset_val,
                            fwd_m_v,
                            width_class_val,
                        );
                    }
                } else {
                    // Wide FF: split into per-8-byte words.  fwd_p is
                    // typically I128 (built by ensure_wide_ptr_val and
                    // related helpers in the narrow-path expression flow
                    // for 65-128 bit dsts).
                    let n_words = nb / 8;
                    let width_class_val = builder.ins().iconst(I32, 8);
                    for i in 0..n_words {
                        let shifted = if i == 0 {
                            fwd_p
                        } else {
                            builder.ins().ushr_imm(fwd_p, (i * 64) as i64)
                        };
                        let word = builder.ins().ireduce(I64, shifted);
                        let off = log_current_offset + (i * 8) as i32;
                        let entry_offset_val = builder.ins().iconst(I32, off as i64);
                        emit_inline_write_log_push(
                            context,
                            builder,
                            entry_offset_val,
                            word,
                            width_class_val,
                        );
                    }
                    if context.use_4state
                        && let Some(fwd_m_v) = fwd_m
                    {
                        for i in 0..n_words {
                            let shifted = if i == 0 {
                                fwd_m_v
                            } else {
                                builder.ins().ushr_imm(fwd_m_v, (i * 64) as i64)
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
        }

        Some(())
    }

    /// Wide (>128-bit) store: copy from expression pointer to destination memory.
    fn build_binary_wide(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        use super::helpers::{emit_wide_apply_mask, is_wide_ptr};

        // Select on wide destination: fall back to interpreter
        if self.select.is_some() || self.rhs_select.is_some() {
            return None;
        }

        let expr_width = self.expr.width();
        let (payload, mask_xz) = self.expr.build_binary(context, builder)?;
        let nb = calc_native_bytes_for(self.dst_width, self.dst.is_ff());
        let n_words = nb / 8;
        let flags = MemFlags::trusted();

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

        // Source is a wide pointer (for >128-bit expressions)
        // Use expr_width captured before build_binary to determine representation.
        // build_binary returns a pointer for width > 128, register otherwise.
        let src_ptr = if is_wide_ptr(expr_width) {
            payload
        } else {
            // Expression was narrow but dest is wide — store into temp slot
            use super::helpers::ensure_wide_ptr_val;
            ensure_wide_ptr_val(builder, payload, expr_width, nb)
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
            let mask_ptr = if is_wide_ptr(self.expr.width()) {
                mask_xz
            } else {
                use super::helpers::ensure_wide_ptr_val;
                ensure_wide_ptr_val(builder, mask_xz, self.expr.width(), nb)
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

/// Emit one or more wide write-log entries covering `nb` bytes starting
/// at `src_ptr`.  Each entry holds up to `WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES`
/// (56) bytes; wider FFs are split into multiple entries at consecutive
/// FF offsets.
fn emit_wide_log_chunks(
    context: &CraneliftContext,
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
            builder.ins().iadd_imm(src_ptr, written as i64)
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

    let arm_blocks: Vec<_> = chain.arms.iter().map(|_| builder.create_block()).collect();
    let default_block = builder.create_block();
    let final_block = builder.create_block();

    let table_size = (max as usize) + 1;
    let mut entries: Vec<_> = (0..table_size)
        .map(|_| builder.func.dfg.block_call(default_block, &[]))
        .collect();
    for (i, arm) in chain.arms.iter().enumerate() {
        // Later arm wins on duplicate value, matching nested-If fall-through.
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
