use super::*;
use crate::ir::ProtoDynamicBitSelect;

pub(super) const INLINE: &str = r#"
static inline uint64_t vs_mask(uint64_t n) { return n >= 64 ? UINT64_MAX : ((1ull << n) - 1); }
static inline uint64_t vs_load(const uint8_t* p, uint64_t nb, uint64_t word) {
    uint64_t offset = word * 8, value = 0;
    if (offset < nb) { uint64_t n = nb - offset; if (n > 8) n = 8; __builtin_memcpy(&value, p + offset, n); }
    return value;
}
static inline uint64_t vs_bits(const uint8_t* p, uint64_t nb, uint64_t bit) {
    unsigned shift = bit & 63; uint64_t low = vs_load(p, nb, bit >> 6);
    return shift ? (low >> shift) | (vs_load(p, nb, (bit >> 6) + 1) << (64 - shift)) : low;
}
static inline void vs_read(uint8_t* d, const uint8_t* s, uint64_t snb, uint64_t window, uint64_t start, uint64_t skip, uint64_t count) {
    for (uint64_t word = 0; word * 64 < window; ++word) {
        uint64_t out = word * 64, lo = out > skip ? out : skip;
        uint64_t hi = out + 64 < skip + count ? out + 64 : skip + count;
        if (hi > window) hi = window;
        uint64_t value = lo < hi ? (vs_bits(s, snb, start + lo - skip) & vs_mask(hi - lo)) << (lo - out) : 0;
        __builtin_memcpy(d + word * 8, &value, 8);
    }
}
static inline uint64_t vs_merge(const uint8_t* s, uint64_t snb, uint64_t start, uint64_t skip, uint64_t count, uint64_t word, uint64_t old) {
    uint64_t base = word * 64, lo = base > start ? base : start;
    uint64_t hi = base + 64 < start + count ? base + 64 : start + count;
    if (lo >= hi) return old;
    uint64_t mask = vs_mask(hi - lo) << (lo - base);
    uint64_t bits = vs_bits(s, snb, skip + lo - start) << (lo - base);
    return (old & ~mask) | (bits & mask);
}
"#;

fn selected(expr: &ProtoExpression) -> Option<&ProtoDynamicBitSelect> {
    match expr {
        ProtoExpression::Variable { dynamic_select, .. }
        | ProtoExpression::DynamicVariable { dynamic_select, .. } => {
            dynamic_select.as_ref().filter(|d| d.adjusted())
        }
        _ => None,
    }
}

pub(super) fn handles(expr: &ProtoExpression) -> bool {
    selected(expr).is_some()
}

fn bounds(ds: &ProtoDynamicBitSelect, index: &str, width: usize) -> String {
    let elements = ds.row_elements.unwrap_or(ds.num_elements);
    let slots = elements + ds.low_adjust;
    let stride = slots.next_power_of_two();
    let domain = if ds.row_elements.is_some() {
        ds.num_elements
    } else {
        slots
    };
    let decode = if ds.row_elements.is_some() {
        format!(
            "uint64_t _row = (_index / {stride}ull) * {}ull; _index %= {stride}ull;",
            elements * ds.elem_width
        )
    } else {
        "uint64_t _row = 0;".into()
    };
    format!(
        "uint64_t _index = (uint64_t)({index}); unsigned _valid = _index < {domain}ull; if (!_valid) _index = 0; \
        {decode} uint64_t _low = _index < {adjust}ull ? 0 : _index - {adjust}ull; \
        uint64_t _skip = _index < {adjust}ull ? ({adjust}ull - _index) * {elem}ull : 0; \
        uint64_t _start = _row + _low * {elem}ull; uint64_t _count = 0; \
        if (_valid && _low < {elements}ull && _skip < {window}ull && _start < {width}ull) {{ \
            _count = {window}ull - _skip; uint64_t _available = ({elements}ull - _low) * {elem}ull; \
            if (_count > _available) _count = _available; if (_count > {width}ull - _start) _count = {width}ull - _start; }} ",
        adjust = ds.low_adjust,
        elem = ds.elem_width,
        window = ds.window
    )
}

pub(super) fn read(expr: &ProtoExpression, pre: &mut String) -> Option<WideRef> {
    let ds = selected(expr)?;
    let (source, width, valid) = match expr {
        ProtoExpression::Variable {
            var_offset,
            var_full_width,
            ..
        } => {
            let source = if *var_full_width <= 128 {
                let t = next_wide_tmp();
                let ty = if *var_full_width > 64 {
                    "__uint128_t"
                } else {
                    "uint64_t"
                };
                let value = emit_var_load(var_offset, *var_full_width)?;
                pre.push_str(&format!("{ty} _source{t} = ({ty})({value}); "));
                format!("((const uint8_t*)&_source{t})")
            } else {
                pointer(*var_offset)?
            };
            (source, *var_full_width, String::new())
        }
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            element_native_bytes,
            index_expr,
            num_elements,
            ..
        } => {
            let index = emit_expr(index_expr)?;
            let base = pointer(*base_offset)?;
            (
                format!("({base} + _array * {stride}ll)"),
                element_native_bytes * 8,
                format!(
                    "uint64_t _array = (uint64_t)({index}); unsigned _av = _array < {num_elements}ull; if (!_av) _array = 0;"
                ),
            )
        }
        _ => return None,
    };
    let index = emit_expr(&ds.index_expr)?;
    let nb = native_bytes(ds.window).max(8);
    let nw = wide_words(nb);
    let t = next_wide_tmp();
    let guard = if valid.is_empty() {
        ""
    } else {
        "if (!_av) _count = 0;"
    };
    let position = bounds(ds, &index, width);
    pre.push_str(&format!("uint64_t _w{t}[{nw}] = {{0}}; {{ {valid}{position}{guard}vs_read((uint8_t*)_w{t}, (const uint8_t*)({source}), {}ull, {}ull, _start, _skip, _count); }} ", native_bytes(width), ds.window));
    Some(WideRef {
        addr: format!("((uint8_t*)_w{t})"),
        nb: native_bytes(ds.window),
        width: ds.window,
    })
}

fn pointer(offset: VarOffset) -> Option<String> {
    (offset.raw() >= 0).then(|| {
        format!(
            "({} + {:#x})",
            if offset.is_ff() {
                "ff_values"
            } else {
                "comb_values"
            },
            offset.raw()
        )
    })
}

struct Store<'a> {
    expr: &'a ProtoExpression,
    rhs: Option<(usize, usize)>,
    ds: &'a ProtoDynamicBitSelect,
    width: usize,
    dst: String,
    log: Option<String>,
    in_place: bool,
    outer: String,
    span: Option<(u32, u32)>,
}

fn store(a: Store<'_>) -> Option<String> {
    let mut pre = String::new();
    let nb = native_bytes(a.width);
    let src_nb = native_bytes(
        a.expr
            .width()
            .max(a.ds.window)
            .max(a.rhs.map_or(0, |(hi, _)| hi + 1)),
    )
    .max(8);
    let signed = a.rhs.is_none() && a.expr.expr_context().signed;
    let mut source = emit_wide_operand_signed(a.expr, src_nb, signed, &mut pre)?;
    let (start, count) = a.rhs.map_or((0, a.ds.window), |(hi, low)| {
        (low, (hi - low + 1).min(a.ds.window))
    });
    let t = next_wide_tmp();
    let field_nb = native_bytes(a.ds.window).max(8);
    let nw = wide_words(field_nb);
    pre.push_str(&format!("uint64_t _w{t}[{nw}] = {{0}}; vs_read((uint8_t*)_w{t}, {}, {src_nb}ull, {}ull, {start}ull, 0, {count}ull); ", source.addr, a.ds.window));
    source.addr = format!("((const uint8_t*)_w{t})");
    let index = emit_expr(&a.ds.index_expr)?;
    let position = bounds(a.ds, &index, a.width);
    let body = |word: &str| {
        let mut body = format!(
            "{{ uint64_t _word = {word}; uint64_t _old = vs_load((const uint8_t*)({}), {nb}ull, _word); uint64_t _new = vs_merge({}, {field_nb}ull, _start, _skip, _count, _word, _old); ",
            a.dst, source.addr
        );
        if a.in_place {
            body.push_str(&format!(
                "__builtin_memcpy((uint8_t*)({}) + _word * 8, &_new, {}u); ",
                a.dst,
                nb.min(8)
            ));
        }
        if let Some(log) = &a.log {
            let offset = format!("({log} + _word * 8)");
            // Each FF must stay in one pool: commit drains narrow before wide.
            if nb > 16 {
                body.push_str(&emit_wide_log_chunks(
                    "((const uint8_t*)&_new)",
                    &offset,
                    8,
                    a.span,
                ));
            } else {
                body.push_str(&emit_log_push_in(&offset, "_new", nb.min(8), a.span));
            }
        }
        body.push_str("} ");
        body
    };
    let loop_body = if a.log.is_some() {
        (0..(a.ds.window + 63).div_ceil(64).min(a.width.div_ceil(64)))
            .map(|k| {
                format!(
                    "if (_first + {k}ull <= _last) {}",
                    body(&format!("(_first + {k}ull)"))
                )
            })
            .collect::<String>()
    } else {
        format!(
            "for (uint64_t _w = _first; _w <= _last; ++_w) {}",
            body("_w")
        )
    };
    Some(format!(
        "{{ {pre} {} {position} if (_count) {{ uint64_t _first = _start >> 6, _last = (_start + _count - 1) >> 6; {loop_body} }} }}",
        a.outer
    ))
}

pub(super) fn assign(a: &ProtoAssignStatement) -> Option<String> {
    let log = (a.dst.is_ff() && !a.comb_direct).then(|| format!("{}ull", a.dst_ff_current_offset));
    let packed = log.is_some() && a.dst.raw() == a.dst_ff_current_offset;
    store(Store {
        expr: &a.expr,
        rhs: a.rhs_select,
        ds: a.dynamic_select.as_ref()?,
        width: a.dst_width,
        dst: pointer(a.dst)?,
        log,
        in_place: !packed,
        outer: String::new(),
        span: a.dst_ff_current_offset.try_into().ok().and_then(|lo: u32| {
            lo.checked_add(native_bytes(a.dst_width) as u32)
                .map(|hi| (lo, hi))
        }),
    })
}

pub(super) fn assign_dynamic(a: &ProtoAssignDynamicStatement) -> Option<String> {
    let index = emit_expr(&a.dst_index_expr)?;
    let base = pointer(a.dst_base)?;
    let outer = format!(
        "uint64_t _array = (uint64_t)({index}); if (_array >= {}ull) {{ }} else",
        a.dst_num_elements
    );
    let log = (a.dst_base.is_ff() && !a.comb_direct).then(|| {
        format!(
            "({}ull + _array * {}ll)",
            a.dst_ff_current_base_offset, a.dst_stride
        )
    });
    let packed = log.is_some() && a.dst_base.raw() == a.dst_ff_current_base_offset;
    // The outer array guard must enclose bounds computation as well as the stores.
    let body = store(Store {
        expr: &a.expr,
        rhs: a.rhs_select,
        ds: a.dynamic_select.as_ref()?,
        width: a.dst_width,
        dst: format!("({base} + _array * {}ll)", a.dst_stride),
        log,
        in_place: !packed,
        outer: String::new(),
        span: dyn_ff_span(a, a.dst_ff_current_base_offset, native_bytes(a.dst_width)),
    })?;
    Some(format!("{{ {outer} {{ {body} }} }}"))
}
