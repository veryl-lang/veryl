use super::expression::marshal_wide_operand;
use super::helpers::*;
use super::runtime::{Context, HelperSig, call_helper_void};
use crate::ir::{
    ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoDynamicBitSelect, ProtoExpression,
    VarOffset, native_bytes,
};
use cranelift::prelude::types::{I32, I64};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlagsData, Value};

pub(super) fn selected(expr: &ProtoExpression) -> Option<&ProtoDynamicBitSelect> {
    match expr {
        ProtoExpression::Variable { dynamic_select, .. }
        | ProtoExpression::DynamicVariable { dynamic_select, .. } => {
            dynamic_select.as_ref().filter(|d| {
                d.row_elements.is_some()
                    || (d.low_adjust != 0
                        && (d.window > 64
                            || matches!(expr, ProtoExpression::DynamicVariable { .. })
                                && d.storage_width() > 128))
            })
        }
        _ => None,
    }
}

fn address(builder: &mut FunctionBuilder, context: &Context, offset: VarOffset) -> Value {
    let base = if offset.is_ff() {
        context.ff_values
    } else {
        context.comb_values
    };
    builder.ins().iadd_imm_s(base, offset.raw() as i64)
}

fn known_index(
    expr: &ProtoExpression,
    elements: usize,
    context: &mut Context,
    builder: &mut FunctionBuilder,
) -> Option<(Value, Value)> {
    let (index, mask) = expr.build_binary(context, builder)?;
    if builder.func.dfg.value_type(index) != I64 {
        return None;
    }
    let mut valid = builder
        .ins()
        .icmp_imm_u(IntCC::UnsignedLessThan, index, elements as i64);
    if let Some(unknown) = wide_any_xz(
        context,
        builder,
        mask,
        None,
        returns_wide_pointer(expr),
        false,
        expr.width(),
        0,
    ) {
        let known = builder.ins().icmp_imm_s(IntCC::Equal, unknown, 0);
        valid = builder.ins().band(valid, known);
    }
    let zero = builder.ins().iconst(I64, 0);
    Some((builder.ins().select(valid, index, zero), valid))
}

fn bounds(
    ds: &ProtoDynamicBitSelect,
    context: &mut Context,
    builder: &mut FunctionBuilder,
    outer: Option<Value>,
) -> Option<(Value, Value, Value)> {
    let (index, mut valid) = known_index(
        &ds.index_expr,
        ds.num_elements
            + if ds.row_elements.is_none() {
                ds.low_adjust
            } else {
                0
            },
        context,
        builder,
    )?;
    if let Some(outer) = outer {
        valid = builder.ins().band(valid, outer);
    }
    let (index, row, elements) = if let Some(elements) = ds.row_elements {
        let stride = (elements + ds.low_adjust).next_power_of_two();
        let row = builder
            .ins()
            .ushr_imm_s(index, stride.trailing_zeros() as i64);
        let local = builder.ins().band_imm_u(index, (stride - 1) as i64);
        (
            local,
            builder
                .ins()
                .imul_imm_s(row, (elements * ds.elem_width) as i64),
            elements,
        )
    } else {
        (index, builder.ins().iconst(I64, 0), ds.num_elements)
    };
    let zero = builder.ins().iconst(I64, 0);
    let (low, skip) = if ds.low_adjust == 0 {
        (index, zero)
    } else {
        let left = builder
            .ins()
            .icmp_imm_u(IntCC::UnsignedLessThan, index, ds.low_adjust as i64);
        let low = builder.ins().iadd_imm_s(index, -(ds.low_adjust as i64));
        let low = builder.ins().select(left, zero, low);
        let adjust = builder.ins().iconst(I64, ds.low_adjust as i64);
        let skip = builder.ins().isub(adjust, index);
        let skip = builder.ins().select(left, skip, zero);
        (low, builder.ins().imul_imm_s(skip, ds.elem_width as i64))
    };
    let in_row = builder
        .ins()
        .icmp_imm_u(IntCC::UnsignedLessThan, low, elements as i64);
    valid = builder.ins().band(valid, in_row);
    let has_bits = builder
        .ins()
        .icmp_imm_u(IntCC::UnsignedLessThan, skip, ds.window as i64);
    valid = builder.ins().band(valid, has_bits);
    let local = builder.ins().imul_imm_s(low, ds.elem_width as i64);
    let start = builder.ins().iadd(row, local);
    let end = builder.ins().iconst(I64, (elements * ds.elem_width) as i64);
    let available = builder.ins().isub(end, local);
    let window = builder.ins().iconst(I64, ds.window as i64);
    let count = builder.ins().isub(window, skip);
    let short = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, available, count);
    let count = builder.ins().select(short, available, count);
    Some((start, skip, builder.ins().select(valid, count, zero)))
}

fn read_plane(
    context: &mut Context,
    builder: &mut FunctionBuilder,
    src: Value,
    width: usize,
    window: usize,
    position: (Value, Value, Value),
    fill: i64,
) -> Value {
    let dst = alloc_wide_zero(builder, native_bytes(window).max(8));
    let width = builder.ins().iconst(I64, width as i64);
    let window = builder.ins().iconst(I64, window as i64);
    let fill = builder.ins().iconst(I64, fill);
    call_helper_void(
        context,
        builder,
        HelperSig::SelectRead,
        crate::wide_ops::wide_window_read as *const () as usize,
        &[
            dst, src, width, window, position.0, position.1, position.2, fill,
        ],
    );
    dst
}

pub(super) fn read(
    expr: &ProtoExpression,
    context: &mut Context,
    builder: &mut FunctionBuilder,
) -> Option<(Value, Option<Value>)> {
    let ds = selected(expr)?;
    let (src, width, valid) = match expr {
        ProtoExpression::Variable {
            var_offset,
            var_full_width,
            ..
        } => (
            address(builder, context, *var_offset),
            *var_full_width,
            None,
        ),
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            element_native_bytes,
            index_expr,
            num_elements,
            ..
        } => {
            let (index, valid) = known_index(index_expr, *num_elements, context, builder)?;
            let base = address(builder, context, *base_offset);
            let delta = builder.ins().imul_imm_s(index, *stride as i64);
            (
                builder.ins().iadd(base, delta),
                element_native_bytes * 8,
                Some(valid),
            )
        }
        _ => return None,
    };
    let position = bounds(ds, context, builder, valid)?;
    let payload = read_plane(context, builder, src, width, ds.window, position, 0);
    let mask = if context.use_4state {
        let src = builder.ins().iadd_imm_s(src, native_bytes(width) as i64);
        Some(read_plane(
            context, builder, src, width, ds.window, position, -1,
        ))
    } else {
        None
    };
    if ds.window > 128 {
        return Some((payload, mask));
    }
    let load = |builder: &mut FunctionBuilder, p| {
        let low = builder.ins().load(I64, MemFlagsData::trusted(), p, 0);
        if ds.window > 64 {
            let high = builder.ins().load(I64, MemFlagsData::trusted(), p, 8);
            builder.ins().iconcat(low, high)
        } else {
            low
        }
    };
    let payload = load(builder, payload);
    let mask = mask.map(|p| load(builder, p));
    Some((payload, mask))
}

struct Store<'a> {
    ds: &'a ProtoDynamicBitSelect,
    expr: &'a ProtoExpression,
    rhs: Option<(usize, usize)>,
    dst: Value,
    width: usize,
    log: Option<Value>,
    in_place: bool,
    valid: Option<Value>,
}

fn store(a: Store<'_>, context: &mut Context, builder: &mut FunctionBuilder) -> Option<()> {
    let (payload, mask) = a.expr.build_binary(context, builder)?;
    let source_width = a.expr.width().max(a.rhs.map_or(0, |(hi, _)| hi + 1));
    let nb = native_bytes(source_width.max(a.ds.window)).max(8);
    let signed = a.rhs.is_none() && a.expr.expr_context().signed;
    let mut source = marshal_wide_operand(
        context,
        builder,
        returns_wide_pointer(a.expr),
        payload,
        a.expr.width(),
        nb,
        signed,
    );
    let mut mask = if context.use_4state {
        Some(match mask {
            Some(mask) => marshal_wide_operand(
                context,
                builder,
                returns_wide_pointer(a.expr),
                mask,
                a.expr.width(),
                nb,
                signed,
            ),
            None => alloc_wide_zero(builder, nb),
        })
    } else {
        None
    };
    let position = bounds(a.ds, context, builder, a.valid)?;
    let skip = position.1;
    if let Some((hi, low)) = a.rhs {
        let begin = builder.ins().iconst(I64, low as i64);
        let zero = builder.ins().iconst(I64, 0);
        let count = builder
            .ins()
            .iconst(I64, (hi - low + 1).min(a.ds.window) as i64);
        source = read_plane(
            context,
            builder,
            source,
            source_width,
            a.ds.window,
            (begin, zero, count),
            0,
        );
        mask = mask.map(|m| {
            read_plane(
                context,
                builder,
                m,
                source_width,
                a.ds.window,
                (begin, zero, count),
                0,
            )
        });
    }
    let count = position.2;
    let upper = builder.ins().iconst(I64, (a.width as i64) << 32);
    let packed = builder.ins().bor(count, upper);
    let zero = builder.ins().iconst(I64, 0);
    let flags = builder
        .ins()
        .iconst(I64, (u64::from(a.in_place) << 32) as i64);
    let log_buf = if a.log.is_some() {
        context.log_buf
    } else {
        zero
    };
    let log_flags = if let Some(log) = a.log {
        let log = builder.ins().uextend(I64, log);
        builder.ins().bor(log, flags)
    } else {
        flags
    };
    call_helper_void(
        context,
        builder,
        HelperSig::SelectStore,
        crate::wide_ops::wide_window_store_from as *const () as usize,
        &[log_buf, a.dst, source, position.0, packed, log_flags, skip],
    );
    if let Some(mask) = mask {
        let dst = builder
            .ins()
            .iadd_imm_s(a.dst, native_bytes(a.width) as i64);
        let log_flags = if let Some(log) = a.log {
            let log = builder.ins().iadd_imm_s(log, native_bytes(a.width) as i64);
            let log = builder.ins().uextend(I64, log);
            builder.ins().bor(log, flags)
        } else {
            flags
        };
        call_helper_void(
            context,
            builder,
            HelperSig::SelectStore,
            crate::wide_ops::wide_window_store_from as *const () as usize,
            &[log_buf, dst, mask, position.0, packed, log_flags, skip],
        );
    }
    context.load_cache.clear();
    Some(())
}

pub(super) fn assign(
    a: &ProtoAssignStatement,
    context: &mut Context,
    builder: &mut FunctionBuilder,
) -> Option<()> {
    let ds = a.dynamic_select.as_ref()?;
    let log = if a.dst.is_ff() && !a.comb_direct {
        let span = u32::try_from(a.dst_ff_current_offset).ok().and_then(|lo| {
            lo.checked_add(native_bytes(a.dst_width) as u32)
                .map(|hi| (lo, hi))
        });
        super::statement::emit_ff_mark_readers(context, builder, span);
        let log = builder.ins().iconst(I32, a.dst_ff_current_offset as i64);
        Some(builder.ins().iadd(log, context.ff_delta))
    } else {
        None
    };
    let dst = address(builder, context, a.dst);
    let packed = log.is_some() && a.dst.raw() == a.dst_ff_current_offset;
    store(
        Store {
            ds,
            expr: &a.expr,
            rhs: a.rhs_select,
            dst,
            width: a.dst_width,
            log,
            in_place: !packed,
            valid: None,
        },
        context,
        builder,
    )
}

pub(super) fn assign_dynamic(
    a: &ProtoAssignDynamicStatement,
    context: &mut Context,
    builder: &mut FunctionBuilder,
) -> Option<()> {
    let ds = a.dynamic_select.as_ref()?;
    let (index, valid) = known_index(&a.dst_index_expr, a.dst_num_elements, context, builder)?;
    let delta = builder.ins().imul_imm_s(index, a.dst_stride as i64);
    let base = address(builder, context, a.dst_base);
    let dst = builder.ins().iadd(base, delta);
    let log = if a.dst_base.is_ff() && !a.comb_direct {
        let span = u32::try_from(a.dst_ff_current_base_offset)
            .ok()
            .and_then(|lo| {
                u32::try_from(a.dst_stride * a.dst_num_elements.saturating_sub(1) as isize)
                    .ok()
                    .and_then(|delta| {
                        lo.checked_add(delta)?
                            .checked_add(native_bytes(a.dst_width) as u32)
                            .map(|hi| (lo, hi))
                    })
            });
        super::statement::emit_ff_mark_readers(context, builder, span);
        let delta = builder.ins().ireduce(I32, delta);
        let base = builder
            .ins()
            .iconst(I32, a.dst_ff_current_base_offset as i64);
        let log = builder.ins().iadd(base, delta);
        Some(builder.ins().iadd(log, context.ff_delta))
    } else {
        None
    };
    let packed = log.is_some() && a.dst_base.raw() == a.dst_ff_current_base_offset;
    store(
        Store {
            ds,
            expr: &a.expr,
            rhs: a.rhs_select,
            dst,
            width: a.dst_width,
            log,
            in_place: !packed,
            valid: Some(valid),
        },
        context,
        builder,
    )
}
