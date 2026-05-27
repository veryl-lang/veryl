//! Cranelift codegen `impl ProtoExpression` block, sibling to the
//! IR-side impls in `crate::ir::expression`.  Kept here so the IR
//! layer holds no Cranelift imports.

use super::helpers::*;
use super::helpers::{WideOperandPair, wide_fn_addrs};
use super::runtime::{
    Context as CraneliftContext, HelperSig, alloc_wide_slot, call_helper_ret, call_helper_void,
};
use crate::ir::variable::{
    native_bytes as calc_native_bytes, native_bytes_for as calc_native_bytes_for,
};
use crate::ir::{Op, ProtoExpression, Value};
use crate::wide_ops;
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::{I32, I64, I128};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::value::ValueU64;

impl ProtoExpression {
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
                var_full_width,
                select,
                ..
            } => {
                // Wide path: >128-bit variable → return memory pointer
                if is_wide_ptr(*var_full_width) {
                    let nb = calc_native_bytes_for(*var_full_width, var_offset.is_ff());
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

                // See `var_full_width` doc: `nb` must match the variable's
                // true storage size, not the select range.
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
                let offset = var_offset.raw() as i32;
                let cache_key = *var_offset;
                let wide = read_width > 64;

                // Load CSE: reuse previously loaded values for the same address.
                // For nb==4 variables, mask cached values to 32 bits to match
                // the I32 load + uextend behavior of fresh loads.
                // When the cached value's cranelift type differs from what a
                // fresh load of size `nb` would produce (e.g. I128 cached but
                // narrow select wants I64), coerce it so downstream wide /
                // non-wide code sees consistent types.
                let (mut payload, mut mask_xz) = if !context.disable_load_cache
                    && let Some(&(cached_payload, cached_mask_xz)) =
                        context.load_cache.get(&cache_key)
                {
                    let cached_ty = builder.func.dfg.value_type(cached_payload);
                    let want_wide = nb == 16;
                    let cached_wide = cached_ty == I128;

                    let (p, m) = if cached_wide && !want_wide {
                        let p = builder.ins().ireduce(I64, cached_payload);
                        let m = cached_mask_xz.map(|v| builder.ins().ireduce(I64, v));
                        (p, m)
                    } else if !cached_wide && want_wide {
                        let p = builder.ins().uextend(I128, cached_payload);
                        let m = cached_mask_xz.map(|v| builder.ins().uextend(I128, v));
                        (p, m)
                    } else {
                        (cached_payload, cached_mask_xz)
                    };

                    // No I32 mask: cached payload is the `fwd` value from
                    // ProtoAssignStatement::build_binary, which is already
                    // band_const'd to dst_width when dst_width < 64.  The
                    // fresh-load path below uses uextend from a 32-bit
                    // load, which also leaves upper 32 bits = 0.
                    (p, m)
                } else {
                    let load_mem_flag = MemFlags::trusted();

                    let base_addr = if var_offset.is_ff() {
                        context.ff_values
                    } else {
                        context.comb_values
                    };

                    // Use Cranelift's fused load-and-zero-extend opcodes
                    // (uload8/uload16/uload32) so a width-1 read lowers to
                    // a single x86 movzbq instead of load.i8 + uextend.i64
                    // generating two movzbq instructions (the second is a
                    // redundant register-to-register zero-extend).
                    let payload = match nb {
                        16 => builder.ins().load(I128, load_mem_flag, base_addr, offset),
                        8 => builder.ins().load(I64, load_mem_flag, base_addr, offset),
                        4 => builder.ins().uload32(load_mem_flag, base_addr, offset),
                        2 => builder.ins().uload16(I64, load_mem_flag, base_addr, offset),
                        1 => builder.ins().uload8(I64, load_mem_flag, base_addr, offset),
                        _ => unreachable!("variable load: nb={nb}"),
                    };
                    let mask_xz = if context.use_4state {
                        let mask_xz_offset = offset + nb as i32;
                        let mask_xz = match nb {
                            16 => {
                                builder
                                    .ins()
                                    .load(I128, load_mem_flag, base_addr, mask_xz_offset)
                            }
                            8 => builder
                                .ins()
                                .load(I64, load_mem_flag, base_addr, mask_xz_offset),
                            4 => builder
                                .ins()
                                .uload32(load_mem_flag, base_addr, mask_xz_offset),
                            2 => {
                                builder
                                    .ins()
                                    .uload16(I64, load_mem_flag, base_addr, mask_xz_offset)
                            }
                            1 => {
                                builder
                                    .ins()
                                    .uload8(I64, load_mem_flag, base_addr, mask_xz_offset)
                            }
                            _ => unreachable!("variable mask_xz load: nb={nb}"),
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
                    // Match the cranelift value type to the reported
                    // expression width so downstream ops don't uextend an
                    // I128 again.
                    if wide && dyn_sel.elem_width <= 64 {
                        payload = builder.ins().ireduce(I64, payload);
                        if let Some(mxz) = mask_xz {
                            mask_xz = Some(builder.ins().ireduce(I64, mxz));
                        }
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

                        // Match the cranelift value type to the reported
                        // expression width (see dynamic_select branch).
                        if select_width <= 64 {
                            payload = builder.ins().ireduce(I64, payload);
                            if let Some(mxz) = mask_xz {
                                mask_xz = Some(builder.ins().ireduce(I64, mxz));
                            }
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
            ProtoExpression::Value {
                value,
                width,
                expr_context,
            } => {
                // Materialize the unsized all_bit sentinel (Value width == 0)
                // by replicating its 1-bit pattern across the context width;
                // otherwise `x == '1` would compare against integer 1
                // instead of the all-ones fill.
                let (payload_bit, mask_xz_bit, is_all_bit) = match value {
                    Value::U64(x) if x.width == 0 => (x.payload != 0, x.mask_xz != 0, true),
                    _ => (false, false, false),
                };
                if is_all_bit {
                    let target = std::cmp::max(expr_context.width, *width);
                    if is_wide_ptr(target) {
                        let nb = calc_native_bytes(target);
                        let count = nb / 8;
                        let payload_digits = if payload_bit {
                            vec![u64::MAX; count]
                        } else {
                            vec![0u64; count]
                        };
                        let payload = emit_wide_const(builder, &payload_digits, nb);
                        let mask_xz = if context.use_4state {
                            let mask_digits = if mask_xz_bit {
                                vec![u64::MAX; count]
                            } else {
                                vec![0u64; count]
                            };
                            Some(emit_wide_const(builder, &mask_digits, nb))
                        } else {
                            None
                        };
                        return Some((payload, mask_xz));
                    }
                    let filled_mask = gen_mask_for_width(target);
                    let payload_val = if payload_bit { filled_mask } else { 0 };
                    let mask_xz_val = if mask_xz_bit { filled_mask } else { 0 };
                    if target > 64 {
                        let payload = iconst_128(builder, payload_val);
                        let mask_xz = if context.use_4state {
                            Some(iconst_128(builder, mask_xz_val))
                        } else {
                            None
                        };
                        return Some((payload, mask_xz));
                    }
                    let payload = builder.ins().iconst(I64, payload_val as i64);
                    let mask_xz = if context.use_4state {
                        Some(builder.ins().iconst(I64, mask_xz_val as i64))
                    } else {
                        None
                    };
                    return Some((payload, mask_xz));
                }

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

                // Re-derive signed from the operand contexts for
                // Div/Rem: the outer expr_context may have dropped
                // signed via merge() when a sibling branch is unsigned.
                let signed = if matches!(op, Op::Div | Op::Rem) {
                    x.expr_context().signed & y.expr_context().signed
                } else {
                    expr_context.signed
                };
                let wide = expr_context.width > 64;
                let x_wide = x.width() > 64;
                let y_wide = y.width() > 64;

                // Ensure operand types match: widen I64 to I128 if the other is I128.
                // Dispatch on the actual cranelift value type since an
                // unsized all_bit literal may already be I128 even though
                // its logical width is 0.
                let needs_wide = wide || x_wide || y_wide;
                if needs_wide && builder.func.dfg.value_type(x_payload) != I128 {
                    x_payload = builder.ins().uextend(I128, x_payload);
                    if let Some(xm) = x_mask_xz {
                        x_mask_xz = Some(builder.ins().uextend(I128, xm));
                    }
                }
                if needs_wide && builder.func.dfg.value_type(y_payload) != I128 {
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
                    // The shift count `y` is an unsigned magnitude, so it must
                    // not be sign-extended: a narrow count with its MSB set
                    // would become a huge value, and the shift masks the count
                    // modulo the operand width, yielding the wrong amount.
                    if !matches!(
                        op,
                        Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR
                    ) {
                        (y_payload, y_mask_xz) =
                            expand_sign(width, y.width(), y_payload, y_mask_xz, builder);
                    }
                }

                let payload = match op {
                    Op::Add => builder.ins().iadd(x_payload, y_payload),
                    Op::Sub => builder.ins().isub(x_payload, y_payload),
                    Op::Mul => builder.ins().imul(x_payload, y_payload),
                    Op::Div => {
                        // I128 div/rem rejected in can_build_binary.
                        // cranelift sdiv traps SIGFPE on both y==0 and
                        // signed i64::MIN / -1. y==0 keeps the existing
                        // "return 0" behaviour; the overflow case is
                        // routed to the dividend to match the analyzer
                        // interpreter's checked_div fallback.
                        let block_zero = builder.create_block();
                        let block_ovf = builder.create_block();
                        let block_ok = builder.create_block();
                        let block_end = builder.create_block();
                        builder.append_block_param(block_end, I64);

                        let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                        if signed {
                            let block_check = builder.create_block();
                            builder
                                .ins()
                                .brif(zero_div, block_zero, &[], block_check, &[]);

                            builder.switch_to_block(block_check);
                            let neg_one = builder.ins().icmp_imm(IntCC::Equal, y_payload, -1);
                            let int_min = builder.ins().icmp_imm(IntCC::Equal, x_payload, i64::MIN);
                            let ovf = builder.ins().band(neg_one, int_min);
                            builder.ins().brif(ovf, block_ovf, &[], block_ok, &[]);
                        } else {
                            builder.ins().brif(zero_div, block_zero, &[], block_ok, &[]);
                        }

                        builder.switch_to_block(block_zero);
                        let zero = builder.ins().iconst(I64, 0);
                        builder.ins().jump(block_end, &[BlockArg::Value(zero)]);

                        builder.switch_to_block(block_ovf);
                        builder.ins().jump(block_end, &[BlockArg::Value(x_payload)]);

                        builder.switch_to_block(block_ok);
                        let ret = if signed {
                            builder.ins().sdiv(x_payload, y_payload)
                        } else {
                            builder.ins().udiv(x_payload, y_payload)
                        };
                        builder.ins().jump(block_end, &[BlockArg::Value(ret)]);
                        builder.switch_to_block(block_end);
                        builder.block_params(block_end)[0]
                    }
                    Op::Rem => {
                        // I128 div/rem rejected in can_build_binary.
                        // Both y==0 and signed i64::MIN % -1 trap
                        // cranelift srem; both are routed to 0 to
                        // preserve the original y==0 behaviour and
                        // match the analyzer interpreter's checked_rem
                        // fallback for the overflow case.
                        let block_zero = builder.create_block();
                        let block_ok = builder.create_block();
                        let block_end = builder.create_block();
                        builder.append_block_param(block_end, I64);

                        let bad = if signed {
                            let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                            let neg_one = builder.ins().icmp_imm(IntCC::Equal, y_payload, -1);
                            let int_min = builder.ins().icmp_imm(IntCC::Equal, x_payload, i64::MIN);
                            let ovf = builder.ins().band(neg_one, int_min);
                            builder.ins().bor(zero_div, ovf)
                        } else {
                            builder.ins().icmp_imm(IntCC::Equal, y_payload, 0)
                        };
                        builder.ins().brif(bad, block_zero, &[], block_ok, &[]);

                        builder.switch_to_block(block_zero);
                        let zero = builder.ins().iconst(I64, 0);
                        builder.ins().jump(block_end, &[BlockArg::Value(zero)]);

                        builder.switch_to_block(block_ok);
                        let ret = if signed {
                            builder.ins().srem(x_payload, y_payload)
                        } else {
                            builder.ins().urem(x_payload, y_payload)
                        };
                        builder.ins().jump(block_end, &[BlockArg::Value(ret)]);
                        builder.switch_to_block(block_end);
                        builder.block_params(block_end)[0]
                    }
                    Op::BitAnd => builder.ins().band(x_payload, y_payload),
                    Op::BitOr => builder.ins().bor(x_payload, y_payload),
                    Op::BitXor => builder.ins().bxor(x_payload, y_payload),
                    Op::BitXnor => builder.ins().bxor_not(x_payload, y_payload),
                    Op::Eq => {
                        // (x == 1) for 1-bit clean x is just x;
                        // (x == 0) for 1-bit clean x is x ^ 1.
                        // Skip when 4-state mask handling kicks in (the
                        // post-loop is_xz path needs the icmp result).
                        if !needs_wide
                            && x_mask_xz.is_none()
                            && y_mask_xz.is_none()
                            && x.is_clean_to_width(1)
                            && let ProtoExpression::Value {
                                value: Value::U64(v),
                                ..
                            } = y.as_ref()
                            && v.mask_xz == 0
                            && (v.payload == 0 || v.payload == 1)
                        {
                            if v.payload == 1 {
                                x_payload
                            } else {
                                builder.ins().bxor_imm(x_payload, 1)
                            }
                        } else {
                            let ret = builder.ins().icmp(IntCC::Equal, x_payload, y_payload);
                            builder.ins().uextend(I64, ret)
                        }
                    }
                    Op::Ne => {
                        if !needs_wide
                            && x_mask_xz.is_none()
                            && y_mask_xz.is_none()
                            && x.is_clean_to_width(1)
                            && let ProtoExpression::Value {
                                value: Value::U64(v),
                                ..
                            } = y.as_ref()
                            && v.mask_xz == 0
                            && (v.payload == 0 || v.payload == 1)
                        {
                            if v.payload == 0 {
                                x_payload
                            } else {
                                builder.ins().bxor_imm(x_payload, 1)
                            }
                        } else {
                            let ret = builder.ins().icmp(IntCC::NotEqual, x_payload, y_payload);
                            builder.ins().uextend(I64, ret)
                        }
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
                        // Fast path when both operands are already 0/1
                        // (1-bit clean) and not 4-state: emit a direct
                        // band/bor on I64 instead of icmp+icmp+band+uextend.
                        // The result is itself 1-bit clean so chains compose.
                        if !needs_wide
                            && x_mask_xz.is_none()
                            && y_mask_xz.is_none()
                            && x.is_clean_to_width(1)
                            && y.is_clean_to_width(1)
                        {
                            if matches!(op, Op::LogicAnd) {
                                builder.ins().band(x_payload, y_payload)
                            } else {
                                builder.ins().bor(x_payload, y_payload)
                            }
                        } else {
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
                    // Widen the sign bit to accumulator width, skipping if
                    // the value is already I128 (unsized all_bit literal).
                    let sign_needs_widen = wide
                        && sign_expr.width() <= 64
                        && builder.func.dfg.value_type(sign_payload) != I128;
                    let sign_payload = if sign_needs_widen {
                        builder.ins().uextend(I128, sign_payload)
                    } else {
                        sign_payload
                    };
                    let sign_mask_xz = sign_mask_xz.map(|v| {
                        if sign_needs_widen && builder.func.dfg.value_type(v) != I128 {
                            builder.ins().uextend(I128, v)
                        } else {
                            v
                        }
                    });

                    // Build the lower part from remaining elements
                    let mut lower_width = 0usize;
                    for (expr, repeat, elem_width) in &elements[1..] {
                        let (elem_payload, elem_mask_xz) = expr.build_binary(context, builder)?;
                        let needs_widen = wide
                            && expr.width() <= 64
                            && builder.func.dfg.value_type(elem_payload) != I128;
                        let elem_payload = if needs_widen {
                            builder.ins().uextend(I128, elem_payload)
                        } else {
                            elem_payload
                        };
                        let elem_mask_xz = elem_mask_xz.map(|v| {
                            if needs_widen && builder.func.dfg.value_type(v) != I128 {
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
                        let needs_widen = wide
                            && expr.width() <= 64
                            && builder.func.dfg.value_type(elem_payload) != I128;
                        let elem_payload = if needs_widen {
                            builder.ins().uextend(I128, elem_payload)
                        } else {
                            elem_payload
                        };
                        let elem_mask_xz = elem_mask_xz.map(|v| {
                            if needs_widen && builder.func.dfg.value_type(v) != I128 {
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

                // Widen branches to match; skip when the value is already
                // I128 (unsized all_bit literal).
                if result_wide || t_wide || f_wide {
                    if !t_wide && builder.func.dfg.value_type(true_payload) != I128 {
                        true_payload = builder.ins().uextend(I128, true_payload);
                        if let Some(v) = true_mask_xz
                            && builder.func.dfg.value_type(v) != I128
                        {
                            true_mask_xz = Some(builder.ins().uextend(I128, v));
                        }
                    }
                    if !f_wide && builder.func.dfg.value_type(false_payload) != I128 {
                        false_payload = builder.ins().uextend(I128, false_payload);
                        if let Some(v) = false_mask_xz
                            && builder.func.dfg.value_type(v) != I128
                        {
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
                element_native_bytes,
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

                let nb = *element_native_bytes;
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

                let load_native_to_i64 =
                    |builder: &mut FunctionBuilder, addr: cranelift::prelude::Value, off: i32| {
                        match nb {
                            16 => builder.ins().load(I128, load_mem_flag, addr, off),
                            8 => builder.ins().load(I64, load_mem_flag, addr, off),
                            4 => builder.ins().uload32(load_mem_flag, addr, off),
                            2 => builder.ins().uload16(I64, load_mem_flag, addr, off),
                            1 => builder.ins().uload8(I64, load_mem_flag, addr, off),
                            _ => unreachable!("DynamicVariable load: nb={nb}"),
                        }
                    };
                let mut payload = load_native_to_i64(builder, addr, 0);
                let mut mask_xz = if context.use_4state {
                    Some(load_native_to_i64(builder, addr, nb as i32))
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
