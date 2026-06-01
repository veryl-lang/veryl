//! Emit a Module's comb network and per-event FF-next logic as C,
//! compile with `cc -O3` to a `.so`, and dispatch the loaded function
//! instead of per-chunk Cranelift.  One big C function lets the host
//! compiler keep values in registers across statements, closing the
//! codegen gap vs Cranelift's per-chunk spill/reload.
//!
//! Uncovered constructs return `None` from the emitters and fall back
//! to Cranelift (per-module for comb, per-event for events).

use crate::FuncPtr;
use crate::ir::{
    ExpressionContext, ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoExpression,
    ProtoForBound, ProtoForRange, ProtoForStatement, ProtoStatement, ProtoSystemFunctionCall,
    VarOffset, native_bytes, veryl_aot_sysfn_print,
};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};
use veryl_analyzer::ir::Op;
use veryl_analyzer::value::Value;

/// C declarations for the wide-op (>128-bit) helper function-pointer table.
/// The emitted `.so` calls the SAME Rust `wide_ops::*` helpers Cranelift uses
/// (via `call_indirect`), so AOT-C and Cranelift are bit-identical by
/// construction.  The table is published once at dlopen via `veryl_set_wideops`
/// (see `compile_source`).  Field order MUST match `WideOpsTable` below (a
/// `#[repr(C)]` struct of `usize` is layout-compatible with this struct of
/// same-sized function pointers).
const WIDEOPS_C_DECLS: &str = "\
typedef void (*veryl_wbin)(uint8_t*, const uint8_t*, const uint8_t*, uint32_t);\n\
typedef void (*veryl_wun)(uint8_t*, const uint8_t*, uint32_t);\n\
typedef void (*veryl_wshift)(uint8_t*, const uint8_t*, uint64_t, uint32_t);\n\
typedef int64_t (*veryl_wcmp)(const uint8_t*, const uint8_t*, uint32_t);\n\
typedef int64_t (*veryl_wred)(const uint8_t*, uint32_t);\n\
typedef void (*veryl_wmask)(uint8_t*, const uint8_t*, uint32_t);\n\
typedef struct {\n\
  veryl_wbin band, bor, bxor, bxor_not, band_not, add, sub, mul;\n\
  veryl_wun bnot, negate, copy;\n\
  veryl_wshift shl, lshr, ashr;\n\
  veryl_wcmp eq, ne, ucmp, scmp;\n\
  veryl_wred is_nonzero, is_all_ones, popcnt_parity;\n\
  veryl_wmask apply_mask, fill_ones;\n\
} veryl_wideops_t;\n\
__attribute__((visibility(\"default\"))) veryl_wideops_t veryl_wideops;\n\
__attribute__((visibility(\"default\"))) void veryl_set_wideops(const void* t) { veryl_wideops = *(const veryl_wideops_t*)t; }\n";

/// Inline C implementations of the wide-op helpers, emitted into every AOT-C
/// `.so` so the hot wide arithmetic compiles in-place (no `call_indirect`
/// through the Rust binary).  Call sites emit `vw_<op>(...)` instead of the
/// `veryl_wideops.<op>(...)` table call; with a compile-time-constant `nb` gcc inlines,
/// fully unrolls the per-word loop, and auto-vectorizes the bitwise ops.  A
/// bit-exact mirror of `wide_ops.rs` (the Cranelift path still calls those
/// helpers, so
/// `--backend-validate` differential-tests this C against them).  Unused
/// `static inline` defs are dropped silently (no -Wunused for `static inline`).
const WIDEOPS_C_INLINE: &str = r##"
#define VW_RD(p,i) (((const veryl_u64_ua*)(p))[(i)])
#define VW_WR(p,i,v) (((veryl_u64_ua*)(p))[(i)] = (v))
static inline void vw_band(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i, VW_RD(a,i) & VW_RD(b,i)); }
static inline void vw_bor(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i, VW_RD(a,i) | VW_RD(b,i)); }
static inline void vw_bxor(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i, VW_RD(a,i) ^ VW_RD(b,i)); }
static inline void vw_bxor_not(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i, ~(VW_RD(a,i) ^ VW_RD(b,i))); }
static inline void vw_band_not(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i, VW_RD(a,i) & ~VW_RD(b,i)); }
static inline void vw_bnot(uint8_t* d,const uint8_t* a,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i, ~VW_RD(a,i)); }
static inline void vw_add(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; uint64_t carry=0;
  for(unsigned i=0;i<n;i++){ uint64_t ai=VW_RD(a,i),bi=VW_RD(b,i);
    uint64_t s1=ai+bi; uint64_t c1=(s1<ai); uint64_t s2=s1+carry; uint64_t c2=(s2<s1);
    VW_WR(d,i,s2); carry=c1+c2; } }
static inline void vw_sub(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; uint64_t borrow=0;
  for(unsigned i=0;i<n;i++){ uint64_t ai=VW_RD(a,i),bi=VW_RD(b,i);
    uint64_t d1=ai-bi; uint64_t b1=(ai<bi); uint64_t d2=d1-borrow; uint64_t b2=(d1<borrow);
    VW_WR(d,i,d2); borrow=b1+b2; } }
static inline void vw_mul(uint8_t* d,const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i,0);
  for(unsigned i=0;i<n;i++){ uint64_t ai=VW_RD(a,i); if(ai==0) continue; __uint128_t carry=0;
    for(unsigned j=0;j<n;j++){ if(i+j>=n) break;
      __uint128_t prod=(__uint128_t)ai*(__uint128_t)VW_RD(b,j)+(__uint128_t)VW_RD(d,i+j)+carry;
      VW_WR(d,i+j,(uint64_t)prod); carry=prod>>64; } } }
static inline void vw_negate(uint8_t* d,const uint8_t* a,uint32_t nb){
  unsigned n=nb/8; uint64_t carry=1;
  for(unsigned i=0;i<n;i++){ uint64_t t=~VW_RD(a,i); uint64_t s=t+carry; uint64_t c=(s<t);
    VW_WR(d,i,s); carry=c; } }
static inline void vw_copy(uint8_t* d,const uint8_t* s,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++) VW_WR(d,i, VW_RD(s,i)); }
static inline int64_t vw_eq(const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++){ if(VW_RD(a,i)!=VW_RD(b,i)) return 0; } return 1; }
static inline int64_t vw_ne(const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++){ if(VW_RD(a,i)!=VW_RD(b,i)) return 1; } return 0; }
static inline int64_t vw_ucmp(const uint8_t* a,const uint8_t* b,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=n;i-->0;){ uint64_t ai=VW_RD(a,i),bi=VW_RD(b,i);
    if(ai<bi) return -1; if(ai>bi) return 1; } return 0; }
static inline int64_t vw_scmp(const uint8_t* a,const uint8_t* b,uint32_t packed){
  uint32_t nb=packed&0xFFFF, width=packed>>16; if(width==0||nb==0) return 0;
  unsigned sw=(width-1)/64, sb=(width-1)%64;
  uint64_t as=(VW_RD(a,sw)>>sb)&1, bs=(VW_RD(b,sw)>>sb)&1;
  if(as!=bs){ return as==1? -1 : 1; } return vw_ucmp(a,b,nb); }
static inline void vw_shl(uint8_t* d,const uint8_t* a,uint64_t amount,uint32_t nb){
  unsigned n=nb/8; unsigned ws=(unsigned)(amount/64); uint32_t bs=(uint32_t)(amount%64);
  if(ws>=n){ for(unsigned i=0;i<n;i++) VW_WR(d,i,0); return; }
  for(unsigned i=n;i-->0;){ long si=(long)i-(long)ws;
    uint64_t lo = si>=0 ? VW_RD(a,(unsigned)si) : 0;
    uint64_t hi = si>0 ? VW_RD(a,(unsigned)si-1) : 0;
    VW_WR(d,i, bs==0 ? lo : (lo<<bs)|(hi>>(64-bs))); } }
static inline void vw_lshr(uint8_t* d,const uint8_t* a,uint64_t amount,uint32_t nb){
  unsigned n=nb/8; unsigned ws=(unsigned)(amount/64); uint32_t bs=(uint32_t)(amount%64);
  if(ws>=n){ for(unsigned i=0;i<n;i++) VW_WR(d,i,0); return; }
  for(unsigned i=0;i<n;i++){ unsigned si=i+ws;
    uint64_t lo = si<n ? VW_RD(a,si) : 0;
    uint64_t hi = si+1<n ? VW_RD(a,si+1) : 0;
    VW_WR(d,i, bs==0 ? lo : (lo>>bs)|(hi<<(64-bs))); } }
static inline void vw_ashr(uint8_t* d,const uint8_t* a,uint64_t amount,uint32_t packed){
  uint32_t nb=packed&0xFFFF, width=packed>>16; if(nb==0||width==0) return;
  unsigned n=nb/8; unsigned sw=(width-1)/64, sb=(width-1)%64;
  uint64_t sign=(VW_RD(a,sw)>>sb)&1;
  vw_lshr(d,a,amount,nb);
  if(sign==1 && amount>0){
    unsigned fill_start = amount>=(uint64_t)width ? 0u : (unsigned)((uint64_t)width-amount);
    for(unsigned bp=fill_start; bp<width; bp++){ unsigned w=bp/64, b=bp%64;
      if(w<n) VW_WR(d,w, VW_RD(d,w) | ((uint64_t)1<<b)); } } }
static inline int64_t vw_is_nonzero(const uint8_t* a,uint32_t nb){
  unsigned n=nb/8; for(unsigned i=0;i<n;i++){ if(VW_RD(a,i)!=0) return 1; } return 0; }
static inline int64_t vw_is_all_ones(const uint8_t* a,uint32_t packed){
  uint32_t width=packed>>16; if(width==0) return 1;
  unsigned fw=width/64; uint32_t rem=width%64;
  for(unsigned i=0;i<fw;i++){ if(VW_RD(a,i)!=~(uint64_t)0) return 0; }
  if(rem>0){ uint64_t m=((uint64_t)1<<rem)-1; if((VW_RD(a,fw)&m)!=m) return 0; }
  return 1; }
static inline int64_t vw_popcnt_parity(const uint8_t* a,uint32_t nb){
  unsigned n=nb/8; uint32_t total=0;
  for(unsigned i=0;i<n;i++) total^=(uint32_t)__builtin_popcountll(VW_RD(a,i));
  return total&1; }
static inline void vw_apply_mask(uint8_t* d,const uint8_t* unused,uint32_t packed){
  (void)unused; uint32_t nb=packed&0xFFFF, width=packed>>16; if(width==0||nb==0) return;
  unsigned n=nb/8; unsigned fw=width/64; uint32_t rem=width%64;
  if(rem>0 && fw<n){ uint64_t m=((uint64_t)1<<rem)-1; VW_WR(d,fw, VW_RD(d,fw)&m); }
  for(unsigned i=fw+(rem>0?1u:0u); i<n; i++) VW_WR(d,i,0); }
static inline void vw_fill_ones(uint8_t* d,const uint8_t* unused,uint32_t packed){
  (void)unused; uint32_t nb=packed&0xFFFF, width=packed>>16; if(nb==0) return;
  unsigned n=nb/8; unsigned fw=width/64; uint32_t rem=width%64;
  unsigned lim = fw<n?fw:n; for(unsigned i=0;i<lim;i++) VW_WR(d,i,~(uint64_t)0);
  if(rem>0 && fw<n) VW_WR(d,fw, ((uint64_t)1<<rem)-1);
  for(unsigned i=fw+(rem>0?1u:0u); i<n; i++) VW_WR(d,i,0); }
"##;

/// `#[repr(C)]` mirror of the emitted `veryl_wideops_t`.  Each field is the
/// address of the corresponding `wide_ops::*` helper; the field ORDER must
/// match `WIDEOPS_C_DECLS` exactly.
#[repr(C)]
struct WideOpsTable {
    band: usize,
    bor: usize,
    bxor: usize,
    bxor_not: usize,
    band_not: usize,
    add: usize,
    sub: usize,
    mul: usize,
    bnot: usize,
    negate: usize,
    copy: usize,
    shl: usize,
    lshr: usize,
    ashr: usize,
    eq: usize,
    ne: usize,
    ucmp: usize,
    scmp: usize,
    is_nonzero: usize,
    is_all_ones: usize,
    popcnt_parity: usize,
    apply_mask: usize,
    fill_ones: usize,
}

fn wideops_table() -> WideOpsTable {
    use crate::backend::cranelift::helpers::wide_fn_addrs as w;
    WideOpsTable {
        band: w::band(),
        bor: w::bor(),
        bxor: w::bxor(),
        bxor_not: w::bxor_not(),
        band_not: w::band_not(),
        add: w::add(),
        sub: w::sub(),
        mul: w::mul(),
        bnot: w::bnot(),
        negate: w::negate(),
        copy: w::copy(),
        shl: w::shl(),
        lshr: w::lshr(),
        ashr: w::ashr(),
        eq: w::eq(),
        ne: w::ne(),
        ucmp: w::ucmp(),
        scmp: w::scmp(),
        is_nonzero: w::is_nonzero(),
        is_all_ones: w::is_all_ones(),
        popcnt_parity: w::popcnt_parity(),
        apply_mask: w::apply_mask(),
        fill_ones: w::fill_ones(),
    }
}

// ───────────────────── wide (>128-bit) value emission ─────────────────────
//
// AOT-C has no statement/prelude side-channel: `emit_expr` returns a single C
// expression.  A wide value cannot be a C scalar, so it is materialized as a
// C-local `uint64_t _wN[]` scratch (or, for a leaf read, a direct pointer
// into a flat buffer).  `emit_wide_expr` appends scratch declarations and
// `vw_*` calls to a flat `pre` buffer and returns a `WideRef`
// naming the result pointer.  Consumers wrap `pre` in ONE block:
//   * a wide store    → `{ <pre> vw_copy(buf+off, ref, nb); ... }`
//   * a narrow result (compare/reduction over wide operands) → a GCC
//     statement-expression `({ <pre> <i64 helper call>; })`.
// Because every scratch is declared in the SAME flat block, all stay live for
// the whole block — unlike nested statement-expressions, whose locals would
// dangle once each inner `({...})` closes.  The 64-bit chunks are accessed
// through `veryl_u64_ua` (1-byte-aligned alias) on the buffer side, since wide
// values can land at 4-byte-aligned offsets; the helpers themselves access
// memory unaligned.  2-state only; 4-state wide bails to None.

thread_local! {
    static WIDE_TMP_CTR: Cell<usize> = const { Cell::new(0) };
}
/// Fresh `_wN` index, unique within a function emit (monotonic; reset by
/// `emit_function` / `emit_event_function` so emitted source is deterministic).
fn next_wide_tmp() -> usize {
    WIDE_TMP_CTR.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}
fn reset_wide_tmp() {
    WIDE_TMP_CTR.with(|c| c.set(0));
}

/// Pack `nb` (byte count) into the low 16 bits and `width` (bit count) into
/// the high 16 bits — the irregular ABI of `wide_ashr`/`wide_scmp`/
/// `wide_apply_mask`/`wide_fill_ones`/`wide_is_all_ones`.  Mirrors
/// `wide_ops::pack_nb_width`.
fn wpack(nb: usize, width: usize) -> u32 {
    (nb as u32 & 0xFFFF) | ((width as u32) << 16)
}

/// A wide (>128-bit) value materialized for the AOT-C path.  `addr` is a C
/// expression of type `uint8_t*` pointing at `nb` little-endian u64 bytes
/// (a flat-buffer address for a leaf read, or a `uint64_t _wN[]` scratch).
struct WideRef {
    addr: String,
    nb: usize,
    width: usize,
}

/// Materialize a wide constant into a fresh `_wN[]` scratch (2-state: payload
/// digits only).  Mirrors the Cranelift `Value` wide arm (expression.rs
/// 378-484): the unsized all-bit sentinel (`Value::U64{width==0}`) fills to
/// `max(ctx_width, proto_width)`; otherwise the declared `proto_width`.
fn emit_wide_const(
    value: &Value,
    proto_width: usize,
    ctx_width: usize,
    pre: &mut String,
) -> Option<WideRef> {
    let (width, digits): (usize, Vec<u64>) = match value {
        Value::U64(x) if x.width == 0 => {
            let target = ctx_width.max(proto_width);
            let count = native_bytes(target) / 8;
            let d = if x.payload != 0 {
                vec![u64::MAX; count]
            } else {
                vec![0u64; count]
            };
            (target, d)
        }
        Value::U64(x) => (proto_width, vec![x.payload]),
        Value::BigUint(x) => (proto_width, x.payload.to_u64_digits()),
    };
    let nb = native_bytes(width);
    let nw = nb / 8;
    let t = next_wide_tmp();
    let mut init = String::new();
    for i in 0..nw {
        if i > 0 {
            init.push_str(", ");
        }
        init.push_str(&format!("0x{:x}ULL", digits.get(i).copied().unwrap_or(0)));
    }
    pre.push_str(&format!("uint64_t _w{t}[{nw}] = {{ {init} }}; "));
    Some(WideRef {
        addr: format!("((uint8_t*)_w{t})"),
        nb,
        width,
    })
}

/// Recursively materialize a wide-result expression.  Called only when
/// `expr.builds_wide_pointer()` is true (so it never sees a comparison/
/// reduction, which produce a narrow register handled in `emit_expr_inner`).
/// Returns `None` (→ module bails to Cranelift) for any uncovered shape.
fn emit_wide_expr(expr: &ProtoExpression, pre: &mut String) -> Option<WideRef> {
    match expr {
        ProtoExpression::Value {
            value,
            width,
            expr_context,
        } => emit_wide_const(value, *width, expr_context.width, pre),
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            var_full_width,
            ..
        } => {
            // A plain wide leaf read aliases the buffer directly.  A wide
            // select / dynamic-select is interpreter-only (build_binary
            // returns a register for a ≤64 select; wider bails).
            if select.is_some() || dynamic_select.is_some() {
                return None;
            }
            let off = match var_offset {
                VarOffset::Ff(o) | VarOffset::Comb(o) => *o,
            };
            if off < 0 {
                return None;
            }
            let buf = match var_offset {
                VarOffset::Ff(_) => "ff_values",
                VarOffset::Comb(_) => "comb_values",
            };
            Some(WideRef {
                addr: format!("((uint8_t*)({buf} + {off:#x}))"),
                nb: native_bytes(*var_full_width),
                width: *var_full_width,
            })
        }
        ProtoExpression::Binary {
            x,
            op,
            y,
            expr_context,
            ..
        } => emit_wide_binary(x, *op, y, expr_context.width, pre),
        ProtoExpression::Unary {
            op,
            x,
            expr_context,
            ..
        } => emit_wide_unary(*op, x, expr_context.width, pre),
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            expr_context,
            ..
        } => emit_wide_ternary(cond, true_expr, false_expr, expr_context.width, pre),
        ProtoExpression::Concatenation {
            elements,
            expr_context,
            ..
        } => emit_wide_concat(elements, expr_context.width, pre),
        // Wide (>16 native-byte) dynamic-array element, full read (no select):
        // the element lives at `base + base_off + stride*idx`; alias it as the
        // wide value pointer (read-only, so no copy).  Narrow/wide-result
        // selects and dynamic bit-selects bail to the interpreter.
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            element_native_bytes,
            index_expr,
            num_elements,
            select,
            dynamic_select,
            ..
        } => {
            if select.is_some() || dynamic_select.is_some() || *num_elements == 0 {
                return None;
            }
            let off = match base_offset {
                VarOffset::Ff(o) | VarOffset::Comb(o) => *o,
            };
            if off < 0 {
                return None;
            }
            let buf = match base_offset {
                VarOffset::Ff(_) => "ff_values",
                VarOffset::Comb(_) => "comb_values",
            };
            let idx = emit_expr(index_expr)?;
            let max_idx = num_elements.saturating_sub(1);
            let t = next_wide_tmp();
            // Clamp the index once; the address below references `_wi{t}`,
            // which lives in the same flat `pre` block.
            pre.push_str(&format!(
                "uint64_t _wi{t} = (uint64_t)({idx}); _wi{t} = _wi{t} < {max} ? _wi{t} : {max}; ",
                max = max_idx,
            ));
            Some(WideRef {
                addr: format!(
                    "((uint8_t*)({buf} + {off:#x} + (intptr_t){stride} * (intptr_t)_wi{t}))"
                ),
                nb: *element_native_bytes,
                width: expr.width(),
            })
        }
    }
}

/// Produce a `WideRef` of exactly `target_nb` bytes for `expr`.  A wide
/// operand is used directly (zero-extended into a larger scratch if its size
/// class is smaller); a narrow (≤128) scalar operand is promoted into a
/// zeroed scratch with its value at word 0 — matching Cranelift's
/// `ensure_wide_ptr_val`.
fn emit_wide_operand(
    expr: &ProtoExpression,
    target_nb: usize,
    pre: &mut String,
) -> Option<WideRef> {
    let tnw = target_nb / 8;
    if expr.builds_wide_pointer() {
        let r = emit_wide_expr(expr, pre)?;
        if r.nb == target_nb {
            return Some(r);
        }
        // Resize into a fresh target_nb scratch.  Copy only `min(r.nb,
        // target_nb)` bytes: when r is narrower the high words stay zero
        // (zero-extend); when r is WIDER (an operand size class above the
        // result, e.g. `c192 = a256 + b256`) the extra words are dropped —
        // a target_nb-byte copy of an r.nb-byte source would otherwise
        // overflow the scratch.  Mirrors Cranelift storing only dst_nb words.
        let snb = r.nb.min(target_nb);
        let t = next_wide_tmp();
        pre.push_str(&format!(
            "uint64_t _w{t}[{tnw}] = {{0}}; vw_copy((uint8_t*)_w{t}, {src}, {snb}u); ",
            src = r.addr,
        ));
        return Some(WideRef {
            addr: format!("((uint8_t*)_w{t})"),
            nb: target_nb,
            width: r.width,
        });
    }
    // `builds_wide_pointer(expr)` is false → emit_expr yields a ≤128-bit
    // scalar register (it returns None for a genuinely >128-bit value that
    // can't be a C scalar — that is the only real "can't represent" case).
    // Do NOT gate on `expr.width()`: a node's `width` field can spuriously
    // exceed its evaluation width (some IR shapes do), and emit_expr
    // still produces a valid scalar — bailing on the field would force the
    // whole comb module off the AOT-C fast path.  Promote via `__uint128_t`
    // (lossless for both u64 and 65-128-bit scalars) into the zeroed slot.
    let scalar = emit_expr(expr)?;
    let t = next_wide_tmp();
    if tnw >= 2 {
        pre.push_str(&format!(
            "uint64_t _w{t}[{tnw}] = {{0}}; __uint128_t _t{t} = (__uint128_t)({scalar}); \
             _w{t}[0] = (uint64_t)_t{t}; _w{t}[1] = (uint64_t)(_t{t} >> 64); "
        ));
    } else {
        pre.push_str(&format!(
            "uint64_t _w{t}[{tnw}] = {{0}}; _w{t}[0] = (uint64_t)({scalar}); "
        ));
    }
    Some(WideRef {
        addr: format!("((uint8_t*)_w{t})"),
        nb: target_nb,
        width: expr.width(),
    })
}

/// Shift amount: the low 64 bits of `y` (Cranelift loads word 0 of the
/// promoted operand).  A narrow scalar IS that low word; a wide `y` reads
/// word 0 of its buffer.
fn wide_shift_amount(y: &ProtoExpression, pre: &mut String) -> Option<String> {
    if y.builds_wide_pointer() {
        let r = emit_wide_expr(y, pre)?;
        Some(format!("((const veryl_u64_ua*)({}))[0]", r.addr))
    } else {
        emit_expr(y)
    }
}

/// Wide binary op with a wide result (band/bor/bxor/bxor_not/add/sub/mul and
/// the shifts).  Div/Rem/Pow → None (interpreter).  Mirrors
/// `build_binary_wide_binary`'s non-comparison arm (expression.rs 2140-2241):
/// width mask applied iff `result_nb == op_nb`.
fn emit_wide_binary(
    x: &ProtoExpression,
    op: Op,
    y: &ProtoExpression,
    width: usize,
    pre: &mut String,
) -> Option<WideRef> {
    let result_nb = native_bytes(width);
    let op_nb = native_bytes(width.max(x.width()).max(y.width()));
    let nw = op_nb / 8;
    let mask_pack = wpack(op_nb, width);
    match op {
        Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor | Op::Add | Op::Sub | Op::Mul => {
            let x_ref = emit_wide_operand(x, op_nb, pre)?;
            let y_ref = emit_wide_operand(y, op_nb, pre)?;
            let fname = match op {
                Op::BitAnd => "band",
                Op::BitOr => "bor",
                Op::BitXor => "bxor",
                Op::BitXnor => "bxor_not",
                Op::Add => "add",
                Op::Sub => "sub",
                Op::Mul => "mul",
                _ => unreachable!(),
            };
            let t = next_wide_tmp();
            pre.push_str(&format!(
                "uint64_t _w{t}[{nw}]; vw_{fname}((uint8_t*)_w{t}, {x}, {y}, {op_nb}u); ",
                x = x_ref.addr,
                y = y_ref.addr,
            ));
            if result_nb == op_nb {
                pre.push_str(&format!(
                    "vw_apply_mask((uint8_t*)_w{t}, (const uint8_t*)0, {mask_pack}u); "
                ));
            }
            Some(WideRef {
                addr: format!("((uint8_t*)_w{t})"),
                nb: op_nb,
                width,
            })
        }
        Op::LogicShiftL | Op::ArithShiftL | Op::LogicShiftR | Op::ArithShiftR => {
            let x_ref = emit_wide_operand(x, op_nb, pre)?;
            let amount = wide_shift_amount(y, pre)?;
            let fname = match op {
                Op::LogicShiftL | Op::ArithShiftL => "shl",
                Op::LogicShiftR => "lshr",
                Op::ArithShiftR => "ashr",
                _ => unreachable!(),
            };
            // shl/lshr take plain nb; ashr packs the OPERAND width.
            let last = if matches!(op, Op::ArithShiftR) {
                format!("{}u", wpack(op_nb, x.width()))
            } else {
                format!("{op_nb}u")
            };
            let t = next_wide_tmp();
            pre.push_str(&format!(
                "uint64_t _w{t}[{nw}]; vw_{fname}((uint8_t*)_w{t}, {x}, (uint64_t)({amount}), {last}); ",
                x = x_ref.addr,
            ));
            if result_nb == op_nb {
                pre.push_str(&format!(
                    "vw_apply_mask((uint8_t*)_w{t}, (const uint8_t*)0, {mask_pack}u); "
                ));
            }
            Some(WideRef {
                addr: format!("((uint8_t*)_w{t})"),
                nb: op_nb,
                width,
            })
        }
        _ => None,
    }
}

/// Wide unary non-reduction (`Add` identity / `Sub` negate / `BitNot`).
/// Mirrors `build_binary_wide_unary` (expression.rs 1925-1955): negate/bnot
/// mask after the op; identity is unmasked.
fn emit_wide_unary(op: Op, x: &ProtoExpression, width: usize, pre: &mut String) -> Option<WideRef> {
    let nb = native_bytes(width);
    let nw = nb / 8;
    let x_ref = emit_wide_operand(x, nb, pre)?;
    match op {
        Op::Add => Some(WideRef {
            addr: x_ref.addr,
            nb,
            width,
        }),
        Op::Sub | Op::BitNot => {
            let fname = if matches!(op, Op::Sub) {
                "negate"
            } else {
                "bnot"
            };
            let t = next_wide_tmp();
            pre.push_str(&format!(
                "uint64_t _w{t}[{nw}]; vw_{fname}((uint8_t*)_w{t}, {x}, {nb}u); \
                 vw_apply_mask((uint8_t*)_w{t}, (const uint8_t*)0, {p}u); ",
                x = x_ref.addr,
                p = wpack(nb, width),
            ));
            Some(WideRef {
                addr: format!("((uint8_t*)_w{t})"),
                nb,
                width,
            })
        }
        _ => None,
    }
}

/// Wide ternary: a narrow condition selects per-word between two wide arms
/// (Cranelift `emit_wide_select`, expression.rs 287-299).
fn emit_wide_ternary(
    cond: &ProtoExpression,
    true_expr: &ProtoExpression,
    false_expr: &ProtoExpression,
    width: usize,
    pre: &mut String,
) -> Option<WideRef> {
    let nb = native_bytes(width);
    let nw = nb / 8;
    let c = emit_expr(cond)?;
    let t_ref = emit_wide_operand(true_expr, nb, pre)?;
    let f_ref = emit_wide_operand(false_expr, nb, pre)?;
    let t = next_wide_tmp();
    pre.push_str(&format!(
        "uint64_t _w{t}[{nw}]; int _c{t} = (({c}) != 0); \
         for (int _i{t} = 0; _i{t} < {nw}; _i{t}++) \
         _w{t}[_i{t}] = _c{t} ? ((const veryl_u64_ua*)({tp}))[_i{t}] \
                              : ((const veryl_u64_ua*)({fp}))[_i{t}]; ",
        tp = t_ref.addr,
        fp = f_ref.addr,
    ));
    Some(WideRef {
        addr: format!("((uint8_t*)_w{t})"),
        nb,
        width,
    })
}

/// Wide concatenation: high-to-low `acc = (acc << elem_width) | elem`, then a
/// final width mask (Cranelift expression.rs 247-272).
fn emit_wide_concat(
    elements: &[(Box<ProtoExpression>, usize, usize)],
    width: usize,
    pre: &mut String,
) -> Option<WideRef> {
    let nb = native_bytes(width);
    let nw = nb / 8;
    let acc = next_wide_tmp();
    pre.push_str(&format!("uint64_t _w{acc}[{nw}] = {{0}}; "));
    for (elem, repeat, elem_width) in elements {
        let e_ref = emit_wide_operand(elem, nb, pre)?;
        for _ in 0..*repeat {
            let sh = next_wide_tmp();
            pre.push_str(&format!(
                "uint64_t _w{sh}[{nw}]; vw_shl((uint8_t*)_w{sh}, (const uint8_t*)_w{acc}, {ew}ull, {nb}u); \
                 vw_bor((uint8_t*)_w{acc}, (const uint8_t*)_w{sh}, {e}, {nb}u); ",
                ew = elem_width,
                e = e_ref.addr,
            ));
        }
    }
    pre.push_str(&format!(
        "vw_apply_mask((uint8_t*)_w{acc}, (const uint8_t*)0, {p}u); ",
        p = wpack(nb, width),
    ));
    Some(WideRef {
        addr: format!("((uint8_t*)_w{acc})"),
        nb,
        width,
    })
}

/// Wide comparison / logic over wide operands → a narrow `uint64_t` 0/1
/// result, wrapped in a self-contained GCC statement-expression.  Mirrors the
/// `is_cmp` arm of `build_binary_wide_binary` (expression.rs 2037-2138).
fn emit_wide_cmp_binary(
    x: &ProtoExpression,
    op: Op,
    y: &ProtoExpression,
    expr_context: &ExpressionContext,
) -> Option<String> {
    let mut pre = String::new();
    let op_nb = native_bytes(expr_context.width.max(x.width()).max(y.width()));
    let x_ref = emit_wide_operand(x, op_nb, &mut pre)?;
    let y_ref = emit_wide_operand(y, op_nb, &mut pre)?;
    let a = x_ref.addr;
    let b = y_ref.addr;
    let result = match op {
        Op::Eq | Op::EqWildcard => format!("(uint64_t)vw_eq({a}, {b}, {op_nb}u)"),
        Op::Ne | Op::NeWildcard => format!("(uint64_t)vw_ne({a}, {b}, {op_nb}u)"),
        Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq => {
            let cmp = if expr_context.signed {
                format!(
                    "vw_scmp({a}, {b}, {p}u)",
                    p = wpack(op_nb, expr_context.width)
                )
            } else {
                format!("vw_ucmp({a}, {b}, {op_nb}u)")
            };
            let test = match op {
                Op::Greater => "> 0",
                Op::GreaterEq => ">= 0",
                Op::Less => "< 0",
                Op::LessEq => "<= 0",
                _ => unreachable!(),
            };
            format!("(uint64_t)(({cmp}) {test})")
        }
        Op::LogicAnd => format!(
            "(uint64_t)((vw_is_nonzero({a}, {op_nb}u) != 0) && (vw_is_nonzero({b}, {op_nb}u) != 0))"
        ),
        Op::LogicOr => format!(
            "(uint64_t)((vw_is_nonzero({a}, {op_nb}u) != 0) || (vw_is_nonzero({b}, {op_nb}u) != 0))"
        ),
        _ => return None,
    };
    Some(format!("({{ {pre}{result}; }})"))
}

/// Wide unary reduction over a wide operand → a narrow `uint64_t` 0/1 result.
/// Mirrors `build_binary_wide_unary`'s reduction arm (expression.rs
/// 1835-1922).  `is_all_ones` takes a packed (nb|width<<16) arg; the others
/// take plain nb.
fn emit_wide_reduce_unary(op: Op, x: &ProtoExpression) -> Option<String> {
    let mut pre = String::new();
    let x_nb = native_bytes(x.width());
    let x_ref = emit_wide_operand(x, x_nb, &mut pre)?;
    let a = x_ref.addr;
    let packed = wpack(x_nb, x.width());
    let result = match op {
        Op::BitAnd => format!("(uint64_t)vw_is_all_ones({a}, {packed}u)"),
        Op::BitNand => format!("(uint64_t)(vw_is_all_ones({a}, {packed}u) ^ 1)"),
        Op::BitOr => format!("(uint64_t)vw_is_nonzero({a}, {x_nb}u)"),
        Op::LogicNot | Op::BitNor => {
            format!("(uint64_t)(vw_is_nonzero({a}, {x_nb}u) ^ 1)")
        }
        Op::BitXor => format!("(uint64_t)vw_popcnt_parity({a}, {x_nb}u)"),
        Op::BitXnor => format!("(uint64_t)(vw_popcnt_parity({a}, {x_nb}u) ^ 1)"),
        _ => return None,
    };
    Some(format!("({{ {pre}{result}; }})"))
}

/// Narrow (≤64-bit) bit-select READ of a WIDE (>128-bit) flat-buffer
/// variable: funnel-shift + mask the `[lo .. lo+nbits)` range out of the
/// little-endian u64 words at `buf + off`, producing a `uint64_t` C
/// expression.  Mirrors Cranelift `emit_wide_bit_select_read_narrow`.
/// Reads through `veryl_u64_ua` (the value can sit at a 4-byte-aligned
/// offset).  `nbits` must be in 1..=64.
fn emit_wide_var_select_read(buf: &str, off: isize, lo: usize, nbits: usize) -> String {
    emit_wide_select_read_at(&format!("{buf} + {off:#x}"), lo, nbits)
}

/// As `emit_wide_var_select_read`, but reading from an arbitrary `uint8_t*`
/// base-pointer C expression (used for dynamic-indexed wide array elements,
/// where the base is `buf + base_off + stride*idx`).  `nbits` in 1..=64.
fn emit_wide_select_read_at(base_ptr: &str, lo: usize, nbits: usize) -> String {
    let word = lo / 64;
    let bit = lo % 64;
    let base = format!("((const veryl_u64_ua*)({base_ptr}))");
    let mut e = if bit == 0 {
        format!("{base}[{word}]")
    } else {
        format!("({base}[{word}] >> {bit})")
    };
    // Straddle into the next word (only when bit > 0, which holds whenever
    // bit + nbits > 64 given nbits ≤ 64 — so `64 - bit` is in 1..=63, never
    // an undefined `<< 64`).
    if bit + nbits > 64 {
        e = format!(
            "({e} | ({base}[{w1}] << {sh}))",
            w1 = word + 1,
            sh = 64 - bit
        );
    }
    if nbits < 64 {
        let mask = (1u64 << nbits) - 1;
        e = format!("({e} & 0x{mask:x}ULL)");
    }
    e
}

/// Emit one or more `WriteLogWideEntry` pushes covering `nb` payload bytes
/// from `src_ptr` (a `uint8_t*` C expression) at FF byte offset `base_off`
/// (a C expression).  Each entry holds ≤56 bytes; larger values chunk.
/// Mirrors `event_write_log_push_wide` / `emit_wide_log_chunks`.
fn emit_wide_log_chunks(src_ptr: &str, base_off: &str, nb: usize) -> String {
    use crate::ir::write_log::{
        WRITE_LOG_WIDE_ENTRY_OFFSET_NB, WRITE_LOG_WIDE_ENTRY_OFFSET_OFFSET,
        WRITE_LOG_WIDE_ENTRY_OFFSET_PAYLOAD, WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES,
        WRITE_LOG_WIDE_ENTRY_SIZE, WRITE_LOG_WIDE_OFFSET_COUNT, WRITE_LOG_WIDE_OFFSET_ENTRIES_PTR,
    };
    let cap = WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES;
    let mut out = String::new();
    let mut written = 0usize;
    while written < nb {
        let chunk = (nb - written).min(cap);
        out.push_str(&format!(
            "{{ unsigned char* _lb = (unsigned char*)write_log; \
                unsigned int _lc = *(unsigned int*)(_lb + {cnt}); \
                unsigned char* _ls = (*(unsigned char**)(_lb + {eptr})) + (unsigned long)_lc * {esz}ul; \
                *(unsigned int*)(_ls + {o_off}) = (unsigned int)(({base}) + {w}u); \
                *(unsigned char*)(_ls + {o_nb}) = (unsigned char){chunk}u; \
                __builtin_memcpy(_ls + {o_pay}, ({src}) + {w}u, {chunk}u); \
                *(unsigned int*)(_lb + {cnt}) = _lc + 1u; }} ",
            cnt = WRITE_LOG_WIDE_OFFSET_COUNT,
            eptr = WRITE_LOG_WIDE_OFFSET_ENTRIES_PTR,
            esz = WRITE_LOG_WIDE_ENTRY_SIZE,
            o_off = WRITE_LOG_WIDE_ENTRY_OFFSET_OFFSET,
            o_nb = WRITE_LOG_WIDE_ENTRY_OFFSET_NB,
            o_pay = WRITE_LOG_WIDE_ENTRY_OFFSET_PAYLOAD,
            base = base_off,
            src = src_ptr,
            w = written,
        ));
        written += chunk;
    }
    out
}

/// Lazily-published compiled `.so`.  `None` while the background
/// compile runs; callers fall back to Cranelift until then.  Shared
/// via `Arc` across `Ir`s built from one `ProtoModule`.
pub type AotCell = Arc<OnceLock<EmittedModule>>;

// Event-path: FF-target assigns push a WriteLogEntry inline through
// the `write_log` arg the comb path leaves unused; ff_commit_from_log
// applies them at cycle end.  2-state narrow packed FFs only;
// everything else bails to Cranelift.
use std::cell::Cell;
thread_local! {
    static EVENT_MODE: Cell<bool> = const { Cell::new(false) };
}
fn event_mode() -> bool {
    EVENT_MODE.with(|c| c.get())
}
fn set_event_mode(on: bool) {
    EVENT_MODE.with(|c| c.set(on));
}

/// Inline narrow WriteLogEntry push.  `offset_expr` / `payload_expr`
/// are C expressions; `wc` is native bytes ∈ {1,2,4,8}.
fn emit_log_push(offset_expr: &str, payload_expr: &str, wc: usize) -> String {
    // Offsets shared with the Cranelift push via write_log.rs consts,
    // so a layout change can't silently desync this emitted C.
    use crate::ir::write_log::{
        WRITE_LOG_ENTRY_OFFSET_MASK_XZ, WRITE_LOG_ENTRY_OFFSET_OFFSET,
        WRITE_LOG_ENTRY_OFFSET_PAYLOAD, WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS, WRITE_LOG_ENTRY_SIZE,
        WRITE_LOG_NARROW_OFFSET_COUNT, WRITE_LOG_NARROW_OFFSET_ENTRIES_PTR,
    };
    format!(
        "{{ unsigned char* _lb = (unsigned char*)write_log; \
            unsigned int _lc = *(unsigned int*)(_lb + {cnt}); \
            unsigned char* _ls = (*(unsigned char**)(_lb + {eptr})) + (unsigned long)_lc * {esz}ul; \
            *(unsigned int*)(_ls + {o_off}) = (unsigned int)({off}); \
            *(unsigned short*)(_ls + {o_mask}) = 0; \
            *(unsigned short*)(_ls + {o_wc}) = (unsigned short){wc}u; \
            *(unsigned long long*)(_ls + {o_pay}) = (unsigned long long)({pay}); \
            *(unsigned int*)(_lb + {cnt}) = _lc + 1u; }}",
        cnt = WRITE_LOG_NARROW_OFFSET_COUNT,
        eptr = WRITE_LOG_NARROW_OFFSET_ENTRIES_PTR,
        esz = WRITE_LOG_ENTRY_SIZE,
        o_off = WRITE_LOG_ENTRY_OFFSET_OFFSET,
        o_mask = WRITE_LOG_ENTRY_OFFSET_MASK_XZ,
        o_wc = WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS,
        o_pay = WRITE_LOG_ENTRY_OFFSET_PAYLOAD,
        off = offset_expr,
        pay = payload_expr,
        wc = wc,
    )
}

/// AOT-C fallback diagnostics gate (`VERYL_AOT_C_DIAG=1` covers both
/// comb and event; legacy `VERYL_AOT_C_EVENT_DIAG=1` is event-only).
pub fn diag_enabled() -> bool {
    std::env::var("VERYL_AOT_C_DIAG").as_deref() == Ok("1")
}

/// Capped event-FF bail-reason diagnostic.
fn ev_diag(msg: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    if (std::env::var("VERYL_AOT_C_EVENT_DIAG").as_deref() == Ok("1") || diag_enabled())
        && N.fetch_add(1, Ordering::Relaxed) < 24
    {
        eprintln!("[aot_event_ff] {msg}");
    }
}

/// Short description of the first uncovered statement after a comb
/// bail.  Re-runs the emit, so call only when already bailing.
pub fn comb_fallback_reason(stmts: &[ProtoStatement]) -> String {
    for s in stmts {
        if emit_stmt(s).is_none() {
            return diag_find_fail(s);
        }
    }
    "no single stmt isolated".to_string()
}

/// Census of EVERY uncovered comb leaf statement (not just the first), so the
/// `VERYL_AOT_C_DIAG` whole-comb fallback report shows all distinct reasons a
/// module bails — guiding which wide constructs still need native coverage.
pub fn comb_uncovered_census(stmts: &[ProtoStatement]) -> Vec<String> {
    let mut out = Vec::new();
    for s in stmts {
        collect_uncovered(s, &mut out);
    }
    out
}

fn collect_uncovered(stmt: &ProtoStatement, out: &mut Vec<String>) {
    if emit_stmt(stmt).is_some() {
        return;
    }
    match stmt {
        ProtoStatement::CompiledBlock(cb) => {
            for s in &cb.original_stmts {
                let mut adj = s.clone();
                adj.adjust_offsets(cb.ff_delta_bytes, cb.comb_delta_bytes);
                collect_uncovered(&adj, out);
            }
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &x.cond
                && emit_expr(c).is_none()
            {
                out.push("If-cond-expr".to_string());
            }
            for s in x.true_side.iter().chain(x.false_side.iter()) {
                collect_uncovered(s, out);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                collect_uncovered(s, out);
            }
        }
        ProtoStatement::For(f) => {
            for s in &f.body {
                collect_uncovered(s, out);
            }
        }
        ProtoStatement::Assign(a) => {
            let expr_ok = emit_expr(&a.expr).is_some();
            let why = if expr_ok {
                String::new()
            } else {
                format!(" rhs={}", classify_uncovered_expr(&a.expr))
            };
            out.push(format!(
                "Assign(ff={},dw={},sel={},dynsel={},rhssel={},exprOK={}){why}",
                a.dst.is_ff(),
                a.dst_width,
                a.select.is_some(),
                a.dynamic_select.is_some(),
                a.rhs_select.is_some(),
                expr_ok,
            ))
        }
        ProtoStatement::AssignDynamic(a) => out.push(format!(
            "AssignDyn(ff={},dw={},sel={},dynsel={},idxOK={},exprOK={})",
            a.dst_base.is_ff(),
            a.dst_width,
            a.select.is_some(),
            a.dynamic_select.is_some(),
            emit_expr(&a.dst_index_expr).is_some(),
            emit_expr(&a.expr).is_some(),
        )),
        ProtoStatement::SystemFunctionCall(_) => out.push("SysFn".to_string()),
        _ => out.push("leaf".to_string()),
    }
}

/// Classify the first uncovered sub-EXPRESSION of `e` (the leaf where
/// emit_expr first returns None), for the `VERYL_AOT_C_DIAG` census — so the
/// `exprOK=false` comb bails name the exact wide construct still missing.
fn classify_uncovered_expr(e: &ProtoExpression) -> String {
    if emit_expr(e).is_some() {
        return "(covered)".to_string();
    }
    match e {
        ProtoExpression::Variable {
            var_full_width,
            select,
            dynamic_select,
            width,
            ..
        } => format!(
            "Var(vfw={var_full_width},w={width},sel={},dynsel={})",
            select.is_some(),
            dynamic_select.is_some()
        ),
        ProtoExpression::Value { width, .. } => format!("Val(w={width})"),
        ProtoExpression::Unary { op, x, width, .. } => {
            if emit_expr(x).is_none() {
                format!("Un({op:?})/{}", classify_uncovered_expr(x))
            } else {
                format!("Un({op:?},w={width},xw={})", x.width())
            }
        }
        ProtoExpression::Binary {
            op, x, y, width, ..
        } => {
            if emit_expr(x).is_none() {
                format!("Bin({op:?})/x:{}", classify_uncovered_expr(x))
            } else if emit_expr(y).is_none() {
                format!("Bin({op:?})/y:{}", classify_uncovered_expr(y))
            } else {
                format!("Bin({op:?},w={width},xw={},yw={})", x.width(), y.width())
            }
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            width,
            ..
        } => {
            if emit_expr(cond).is_none() {
                format!("Tern/c:{}", classify_uncovered_expr(cond))
            } else if emit_expr(true_expr).is_none() {
                format!("Tern/t:{}", classify_uncovered_expr(true_expr))
            } else if emit_expr(false_expr).is_none() {
                format!("Tern/f:{}", classify_uncovered_expr(false_expr))
            } else {
                format!("Tern(w={width})")
            }
        }
        ProtoExpression::Concatenation {
            width, elements, ..
        } => {
            for (el, _, _) in elements {
                if emit_expr(el).is_none() {
                    return format!("Concat/{}", classify_uncovered_expr(el));
                }
            }
            format!("Concat(w={width},n={})", elements.len())
        }
        ProtoExpression::DynamicVariable {
            width,
            element_native_bytes,
            select,
            dynamic_select,
            ..
        } => format!(
            "DynVar(w={width},enb={element_native_bytes},sel={},dynsel={})",
            select.is_some(),
            dynamic_select.is_some()
        ),
    }
}

/// Descend into a rejected statement to name the first failing leaf.
/// Re-runs emit; event_mode must already be set by the caller.
fn diag_find_fail(stmt: &ProtoStatement) -> String {
    match stmt {
        ProtoStatement::CompiledBlock(cb) => {
            for s in &cb.original_stmts {
                let mut adj = s.clone();
                adj.adjust_offsets(cb.ff_delta_bytes, cb.comb_delta_bytes);
                if emit_stmt(&adj).is_none() {
                    return format!("CB/{}", diag_find_fail(&adj));
                }
            }
            "CB(?)".to_string()
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &x.cond
                && emit_expr(c).is_none()
            {
                return "If-cond-expr".to_string();
            }
            for s in x.true_side.iter().chain(x.false_side.iter()) {
                if emit_stmt(s).is_none() {
                    return format!("If/{}", diag_find_fail(s));
                }
            }
            "If(?)".to_string()
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                if emit_stmt(s).is_none() {
                    return format!("Seq/{}", diag_find_fail(s));
                }
            }
            "Seq(?)".to_string()
        }
        ProtoStatement::Assign(a) => format!(
            "Assign(ff={},dw={},sel={:?},dynsel={},rhssel={:?},exprOK={})",
            a.dst.is_ff(),
            a.dst_width,
            a.select,
            a.dynamic_select.is_some(),
            a.rhs_select,
            emit_expr(&a.expr).is_some(),
        ),
        ProtoStatement::AssignDynamic(a) => format!(
            "AssignDyn(ff={},dw={},sel={:?},dynsel={},idxOK={},exprOK={})",
            a.dst_base.is_ff(),
            a.dst_width,
            a.select,
            a.dynamic_select.is_some(),
            emit_expr(&a.dst_index_expr).is_some(),
            emit_expr(&a.expr).is_some(),
        ),
        ProtoStatement::SystemFunctionCall(_) => "SysFn".to_string(),
        ProtoStatement::For(_) => "For".to_string(),
        ProtoStatement::Break => "Break".to_string(),
        _ => "leaf".to_string(),
    }
}

/// Mirror of `AssignStatement::eval_step`'s `value.select(beg, end)`.
fn apply_rhs_select(rhs: String, rhs_select: Option<(usize, usize)>) -> Option<String> {
    match rhs_select {
        None => Some(rhs),
        Some((hi, lo)) => {
            let nbits = hi.checked_sub(lo)?.checked_add(1)?;
            if nbits >= 64 {
                return None;
            }
            let mask = (1u64 << nbits) - 1;
            Some(format!(
                "((({rhs}) >> {lo}) & 0x{m:x}ULL)",
                rhs = rhs,
                lo = lo,
                m = mask
            ))
        }
    }
}

/// Low-`width` bitmask (width ≤ 64).
fn width_mask(width: usize) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

/// Event-path WIDE FF write (static dst, `dst_width > 64`): materialize the
/// masked RHS into a scratch and push it through the 64-byte WriteLogWideEntry
/// pool (≤56-byte payload chunks).  Covers 65-128 bit (scalar promoted) and
/// >128 bit (helper-table value).  Select / dynamic_select / rhs_select wide
/// > FFs stay on Cranelift (the module bails).  2-state only.
fn emit_event_ff_assign_wide(a: &ProtoAssignStatement) -> Option<String> {
    if a.select.is_some() || a.dynamic_select.is_some() || a.rhs_select.is_some() {
        ev_diag(&format!(
            "wide FF: select={:?} dynsel={} rhssel={:?} width={}",
            a.select,
            a.dynamic_select.is_some(),
            a.rhs_select,
            a.dst_width
        ));
        return None;
    }
    let dst_raw = match a.dst {
        VarOffset::Ff(o) => o,
        VarOffset::Comb(_) => return None,
    };
    let cur_off = a.dst_ff_current_offset;
    if cur_off < 0 || dst_raw < 0 {
        return None;
    }
    let packed = dst_raw == cur_off;
    let nb = native_bytes(a.dst_width);
    let nw = nb / 8;
    let mut pre = String::new();
    // Build the RHS to `nb` bytes, then copy into a fresh scratch and mask it
    // there (the canonical FF slot must not be clobbered before commit; the
    // source may alias a flat-buffer read).
    let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
    let d = next_wide_tmp();
    pre.push_str(&format!(
        "uint64_t _w{d}[{nw}]; vw_copy((uint8_t*)_w{d}, {src}, {nb}u); \
         vw_apply_mask((uint8_t*)_w{d}, (const uint8_t*)0, {p}u); ",
        src = r.addr,
        p = wpack(nb, a.dst_width),
    ));
    // Dual-slot FF: mirror the narrow path by writing the next physical slot
    // directly (vestigial — ff_commit applies the log — but kept for parity).
    let store = if packed {
        String::new()
    } else {
        format!(
            "vw_copy((uint8_t*)(ff_values + {dst:#x}), (const uint8_t*)_w{d}, {nb}u); ",
            dst = dst_raw,
        )
    };
    let push = emit_wide_log_chunks(&format!("(uint8_t*)_w{d}"), &format!("{cur_off:#x}"), nb);
    Some(format!("{{ {pre}{store}{push} }}"))
}

/// Event-path FF write (static dst): pushes a WriteLogEntry at the
/// canonical current offset.  2-state narrow packed FFs only.
fn emit_event_ff_assign(a: &ProtoAssignStatement) -> Option<String> {
    if a.dst_width == 0 {
        ev_diag("static FF: width=0");
        return None;
    }
    // Wide FF (>64): the narrow WriteLogEntry payload is u64-only, so any
    // FF wider than 64 bits routes through the wide write-log pool (covers
    // 65-128 via __uint128_t promotion and >128 via the helper table).
    if a.dst_width > 64 {
        return emit_event_ff_assign_wide(a);
    }
    let nb = native_bytes(a.dst_width);
    let cty = native_c_type(nb)?;
    let dst_raw = match a.dst {
        VarOffset::Ff(o) => o,
        VarOffset::Comb(_) => return None,
    };
    let cur_off = a.dst_ff_current_offset;
    if cur_off < 0 || dst_raw < 0 {
        return None;
    }
    // Packed FF: dst == canonical current offset, no direct store (log only).
    // Dual-slot FF: dst is the next slot (cur_off + nb); mirror the interpret
    // path by writing the next slot directly AND pushing the log at cur_off.
    let packed = dst_raw == cur_off;
    let log_off = format!("{:#x}", cur_off);
    let dst_off = format!("{:#x}", dst_raw);
    let dwmask = width_mask(a.dst_width);
    let rhs = apply_rhs_select(emit_expr_root(&a.expr)?, a.rhs_select)?;
    // Runtime-indexed bit-slice write into a packed FF (`ff[dyn_idx] <= v`):
    // RMW with a runtime shift = idx*elem_width.  Mirrors the dynamic_select
    // arm of AssignStatement::eval_step.
    if let Some(dyn_sel) = &a.dynamic_select {
        let ew = dyn_sel.elem_width;
        let ne = dyn_sel.num_elements;
        if ew == 0 || ew >= 64 || ne == 0 || ne.checked_mul(ew)? > 64 {
            ev_diag(&format!(
                "static FF: dynamic_select ew={ew} ne={ne} unsupported"
            ));
            return None;
        }
        let vmask = (1u64 << ew) - 1;
        let max_idx = ne - 1;
        let idx = emit_expr(&dyn_sel.index_expr)?;
        let body = format!(
            "uint64_t _di_raw = (uint64_t)({idx}); \
             uint64_t _di = _di_raw < {max} ? _di_raw : {max}; \
             uint64_t _sh = _di * {ew}ull; \
             uint64_t _m = ((((uint64_t)*((const {ct}*)(ff_values + {dst})) & ~(0x{vm:x}ULL << _sh)) | \
                 (((uint64_t)({rhs}) & 0x{vm:x}ULL) << _sh)) & 0x{dw:x}ULL);",
            idx = idx,
            max = max_idx,
            ew = ew,
            ct = cty,
            dst = dst_off,
            vm = vmask,
            rhs = rhs,
            dw = dwmask,
        );
        let store = if packed {
            String::new()
        } else {
            format!(
                "*(({ct}*)(ff_values + {dst})) = ({ct})_m;",
                ct = cty,
                dst = dst_off
            )
        };
        let push = emit_log_push(&log_off, "_m", nb);
        return Some(format!("{{ {body} {store} {push} }}"));
    }
    if let Some((hi, lo)) = a.select {
        let nbits = hi.checked_sub(lo)?.checked_add(1)?;
        if nbits >= 64 {
            return None;
        }
        let vmask = (1u64 << nbits) - 1;
        let pmask = vmask << lo;
        // RMW: read the dst slot (matches AssignStatement::eval_step reading
        // `self.dst`), merge [lo,hi], write dst if dual-slot, push merged.
        let merged = format!(
            "((((uint64_t)*((const {ct}*)(ff_values + {dst})) & ~0x{pm:x}ULL) | \
               ((((uint64_t)({rhs})) & 0x{vm:x}ULL) << {lo})) & 0x{dw:x}ULL)",
            ct = cty,
            dst = dst_off,
            pm = pmask,
            rhs = rhs,
            vm = vmask,
            lo = lo,
            dw = dwmask,
        );
        let push = emit_log_push(&log_off, "_m", nb);
        if packed {
            Some(format!("{{ uint64_t _m = {merged}; {push} }}"))
        } else {
            Some(format!(
                "{{ uint64_t _m = {merged}; *(({ct}*)(ff_values + {dst})) = ({ct})_m; {push} }}",
                ct = cty,
                dst = dst_off,
            ))
        }
    } else {
        let payload = format!(
            "(((uint64_t)({rhs})) & 0x{dw:x}ULL)",
            rhs = rhs,
            dw = dwmask
        );
        let push = emit_log_push(&log_off, "_v", nb);
        if packed {
            Some(format!("{{ uint64_t _v = {payload}; {push} }}"))
        } else {
            Some(format!(
                "{{ uint64_t _v = {payload}; *(({ct}*)(ff_values + {dst})) = ({ct})_v; {push} }}",
                ct = cty,
                dst = dst_off,
            ))
        }
    }
}

/// Event-path FF write to a dynamic-indexed array.  Writes the element
/// slot and pushes a WriteLogEntry at `current_base + stride*idx`.
/// 2-state, narrow, no select/dynamic_select; else bails.
fn emit_event_ff_assign_dynamic(a: &ProtoAssignDynamicStatement) -> Option<String> {
    if a.select.is_some() || a.dynamic_select.is_some() {
        ev_diag(&format!(
            "dyn FF: select={:?} dynsel={}",
            a.select,
            a.dynamic_select.is_some()
        ));
        return None;
    }
    if a.dst_width == 0 || a.dst_width > 64 {
        ev_diag(&format!("dyn FF: width={}", a.dst_width));
        return None;
    }
    if a.dst_num_elements == 0 {
        return None;
    }
    let nb = native_bytes(a.dst_width);
    let cty = native_c_type(nb)?;
    let dst_base_raw = match a.dst_base {
        VarOffset::Ff(o) => o,
        VarOffset::Comb(_) => return None,
    };
    let cur_base = a.dst_ff_current_base_offset;
    if cur_base < 0 || dst_base_raw < 0 {
        return None;
    }
    let rhs = apply_rhs_select(emit_expr_root(&a.expr)?, a.rhs_select)?;
    let idx = emit_expr(&a.dst_index_expr)?;
    let max_idx = a.dst_num_elements.saturating_sub(1);
    let dwmask = width_mask(a.dst_width);
    let payload = format!(
        "(((uint64_t)({rhs})) & 0x{dw:x}ULL)",
        rhs = rhs,
        dw = dwmask
    );
    let push = emit_log_push("_woff", "_wval", nb);
    Some(format!(
        "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
            uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
            uint64_t _wval = {pay}; \
            *(({ct}*)(ff_values + {wbase:#x} + (intptr_t){stride} * (intptr_t)_idx)) = ({ct})_wval; \
            unsigned int _woff = (unsigned int)((intptr_t){cbase:#x} + (intptr_t){stride} * (intptr_t)_idx); \
            {push} }});",
        idx = idx,
        max = max_idx,
        pay = payload,
        ct = cty,
        wbase = dst_base_raw,
        stride = a.dst_stride,
        cbase = cur_base,
        push = push,
    ))
}

/// Compiled AOT-C eval handle.  Owns the loaded shared library so the
/// `func` pointer remains valid.  Drop order: `func` is invalidated when
/// `_lib` drops, so this struct must outlive every dispatch.
pub struct EmittedModule {
    pub func: FuncPtr,
    /// Keep-alive: dropping unloads the library and invalidates `func`.
    _lib: libloading::Library,
}

/// Compile `src` to an `EmittedModule` published through a `OnceLock`.  When
/// `async_mode` is true a background thread fills the cell (empty until ready →
/// callers stay on Cranelift, then hot-swap to AOT-C the cycle the `.so` is
/// ready — hiding the cold gcc latency); otherwise it is filled synchronously
/// before return.  A compile failure (e.g. missing `cc`) leaves the cell empty
/// → graceful Cranelift fallback either way.
fn compile_or_spawn(src: String, async_mode: bool) -> AotCell {
    let cell = Arc::new(OnceLock::new());
    if async_mode {
        let c = Arc::clone(&cell);
        std::thread::spawn(move || {
            if let Ok(m) = compile_source(&src) {
                let _ = c.set(m);
            }
        });
    } else if let Ok(m) = compile_source(&src) {
        let _ = cell.set(m);
    }
    cell
}

/// Prepare the comb AOT-C eval handle.  Whether to attempt AOT-C at all is the
/// caller's decision (gated on `Config::aot_c` in `conv`); this only emits +
/// compiles.  `None` when the emitter can't cover every comb stmt; `Some(cell)`
/// otherwise — `cell.get()` is `None` until the `.so` is ready (`async_mode`).
pub fn prepare_comb(stmts: &[ProtoStatement], async_mode: bool) -> Option<AotCell> {
    let src = emit_function(stmts)?; // coverage gate (sync, fast)
    Some(compile_or_spawn(src, async_mode))
}

/// Event-path `prepare_comb`.  Caller gates on `Config::aot_c_event`.
pub fn prepare_event(stmts: &[ProtoStatement], async_mode: bool) -> Option<AotCell> {
    let src = emit_event_function(stmts)?;
    Some(compile_or_spawn(src, async_mode))
}

/// Emit one `veryl_aot_eval` function for an event statement sequence.
/// FF-target assigns push WriteLogEntries via `write_log` (unused in
/// the comb path).
fn emit_event_function(stmts: &[ProtoStatement]) -> Option<String> {
    reset_wide_tmp();
    let diag = std::env::var("VERYL_AOT_C_EVENT_DIAG").as_deref() == Ok("1");
    set_event_mode(true);
    let body_res = (|| {
        let mut cb = String::new();
        for (i, stmt) in stmts.iter().enumerate() {
            let s = match emit_stmt(stmt) {
                Some(s) => s,
                None => {
                    if diag {
                        let label: &str = match stmt {
                            ProtoStatement::Assign(a) => {
                                if a.dst.is_ff() {
                                    let raw = match a.dst {
                                        VarOffset::Ff(o) => o,
                                        VarOffset::Comb(o) => o,
                                    };
                                    eprintln!(
                                        "[aot_event_diag] bail stmt#{i} Assign(FF) dst_raw={} cur_off={} packed={} width={} select={:?} dynsel={}",
                                        raw,
                                        a.dst_ff_current_offset,
                                        raw == a.dst_ff_current_offset,
                                        a.dst_width,
                                        a.select,
                                        a.dynamic_select.is_some(),
                                    );
                                }
                                "Assign"
                            }
                            ProtoStatement::AssignDynamic(a) => {
                                eprintln!(
                                    "[aot_event_diag] bail stmt#{i} AssignDynamic dst_ff={} width={} select={:?} dynsel={}",
                                    a.dst_base.is_ff(),
                                    a.dst_width,
                                    a.select,
                                    a.dynamic_select.is_some(),
                                );
                                "AssignDynamic"
                            }
                            ProtoStatement::If(_) => "If",
                            ProtoStatement::SequentialBlock(_) => "SeqBlock",
                            ProtoStatement::CompiledBlock(_) => "CompiledBlock",
                            ProtoStatement::For(_) => "For",
                            ProtoStatement::SystemFunctionCall(_) => "SysFn",
                            ProtoStatement::Break => "Break",
                            _ => "Other",
                        };
                        let leaf = diag_find_fail(stmt);
                        eprintln!(
                            "[aot_event_diag] first bail at stmt#{i} kind={label} leaf={leaf} (total={})",
                            stmts.len()
                        );
                        // Full census of ALL uncovered event stmts (event_mode
                        // is set), so a single fix doesn't just surface the
                        // next bail.  Mirrors the whole_comb census.
                        let mut census: Vec<String> = Vec::new();
                        for s in stmts {
                            collect_uncovered(s, &mut census);
                        }
                        let mut counts: HashMap<String, usize> = Default::default();
                        for c in census {
                            *counts.entry(c).or_default() += 1;
                        }
                        let mut v: Vec<_> = counts.into_iter().collect();
                        v.sort_by_key(|x| std::cmp::Reverse(x.1));
                        eprintln!(
                            "[aot_event_census] {} distinct uncovered event stmts:",
                            v.len()
                        );
                        for (k, n) in v.iter().take(40) {
                            eprintln!("  {n:6}x  {k}");
                        }
                    }
                    return None;
                }
            };
            cb.push_str("    ");
            cb.push_str(&s);
            cb.push('\n');
        }
        if diag {
            eprintln!(
                "[aot_event_diag] ALL {} top-level event stmts emitted OK",
                stmts.len()
            );
        }
        Some(cb)
    })();
    set_event_mode(false);
    let body = body_res?;
    let mut src = String::from(
        "// AOT-C event; do not edit.\n\
         #include <stdint.h>\n\
         typedef __uint128_t veryl_u128_ua __attribute__((__aligned__(1)));\n\
         typedef uint64_t veryl_u64_ua __attribute__((__aligned__(1)));\n\
         typedef void (*veryl_sysfn_t)(const unsigned char*, unsigned long, const unsigned long long*, const unsigned int*, unsigned long, unsigned);\n\
         __attribute__((visibility(\"default\"))) veryl_sysfn_t veryl_sysfn_cb = 0;\n\
         __attribute__((visibility(\"default\"))) void veryl_set_sysfn_cb(void *p) { veryl_sysfn_cb = (veryl_sysfn_t)p; }\n",
    );
    src.push_str(WIDEOPS_C_DECLS);
    src.push_str(WIDEOPS_C_INLINE);
    src.push_str(
        "\n\
         __attribute__((visibility(\"default\")))\n\
         void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log) {\n",
    );
    src.push_str(&body);
    src.push_str("}\n");
    Some(src)
}

/// Event-path `$display` / `$write` → a call into the Rust formatter
/// (`veryl_sysfn_cb`, wired by `compile_source`), instead of bailing.  Reuses
/// the interpret path's formatting + `output_buffer` for byte-identical,
/// correctly-buffered output.  Args must be ≤ 64 bits (wider → bail to
/// Cranelift, preserving correctness).  `newline` = true for `$display`.
fn emit_event_print(format_str: &str, args: &[ProtoExpression], newline: bool) -> Option<String> {
    let n = args.len();
    let nl = newline as u32;
    let flen = format_str.len();
    // Pass the format string as raw bytes (no C escaping needed).
    let fbytes: String = format_str
        .as_bytes()
        .iter()
        .map(|b| format!("{b},"))
        .collect();
    let mut s = format!("{{ static const unsigned char _f[] = {{ {fbytes}0 }};");
    if n == 0 {
        s.push_str(&format!(
            " if (veryl_sysfn_cb) veryl_sysfn_cb(_f, {flen}ul, 0, 0, 0ul, {nl}u); }}"
        ));
        return Some(s);
    }
    s.push_str(&format!(
        " unsigned long long _v[{n}]; unsigned int _w[{n}];"
    ));
    for (i, arg) in args.iter().enumerate() {
        let w = arg.width();
        if w == 0 || w > 64 {
            return None; // wide arg → bail to Cranelift
        }
        let e = emit_expr(arg)?;
        let mask = width_mask(w);
        // Pack signedness (bit 16) alongside the width so the Rust formatter
        // rebuilds the AnalyzerValue exactly as the interpreter's eval() would
        // — signedness changes %d/%s output (signed decimal) and the event
        // path must match the Cranelift/interpret path byte-for-byte.
        let packed = w | ((arg.expr_context().signed as usize) << 16);
        s.push_str(&format!(
            " _v[{i}] = (unsigned long long)({e}) & 0x{mask:x}ULL; _w[{i}] = {packed}u;"
        ));
    }
    s.push_str(&format!(
        " if (veryl_sysfn_cb) veryl_sysfn_cb(_f, {flen}ul, _v, _w, {n}ul, {nl}u); }}"
    ));
    Some(s)
}

/// Compile C source to a `.so`, dlopen it, return a handle owning the
/// library and exposing `veryl_aot_eval`.
///
/// Caches under `$XDG_CACHE_HOME/veryl/aot_c/` (overridable via
/// `VERYL_AOT_CACHE_DIR`).  Cache key is FNV-1a over `src` plus
/// everything that changes the produced code (simulator version,
/// compiler, flags, target arch/OS).
///
/// Any failure (compile / dlopen / missing symbol) returns `Err`;
/// `compile_or_spawn` discards it to fall back to Cranelift.
pub fn compile_source(src: &str) -> Result<EmittedModule, String> {
    let cache_dir = aot_c_cache_dir().map_err(|e| format!("cache dir: {e}"))?;
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("create_dir_all: {e}"))?;

    let cc_name = std::env::var("VERYL_AOT_CC").unwrap_or_else(|_| "cc".to_string());
    // Full flag list — built once and used for *both* the cache key and the
    // actual invocation so they can never drift apart.
    let mut flags: Vec<String> = [
        // -fstrict-aliasing (default at -O3) lets gcc assume the differently
        // typed pointer views of one FF (e.g. uint64_t store vs uint32_t
        // bit-select read) don't alias and cache stale values; -fno-strict-
        // aliasing prevents that.  -fvisibility=hidden frees LTO to inline/DCE.
        "-O3",
        "-fPIC",
        "-shared",
        "-fvisibility=hidden",
        "-fno-strict-aliasing",
        "-Wno-unused-but-set-variable",
        "-Wno-overflow",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Optional extra flags via VERYL_AOT_CFLAGS (e.g. PGO sweeps).
    if let Ok(extra) = std::env::var("VERYL_AOT_CFLAGS") {
        flags.extend(extra.split_whitespace().map(str::to_string));
    }

    // Cache key = version + compiler + flags + target arch/OS + source.
    let flags_joined = flags.join(" ");
    let hash = fnv1a_64_hex_parts(&[
        env!("CARGO_PKG_VERSION"),
        &cc_name,
        &flags_joined,
        std::env::consts::ARCH,
        std::env::consts::OS,
        src,
    ]);
    let so_path = cache_dir.join(format!("veryl_aot_{hash}.so"));

    if !so_path.exists() {
        // Write source next to .so so cache hits never need it; placing
        // it in the same dir makes manual debugging (`cc -E`) trivial.
        let c_path = cache_dir.join(format!("veryl_aot_{hash}.c"));
        std::fs::write(&c_path, src).map_err(|e| format!("write {}: {}", c_path.display(), e))?;

        let mut cmd = Command::new(&cc_name);
        cmd.args(&flags).arg("-o").arg(&so_path).arg(&c_path);

        let out = cmd
            .output()
            .map_err(|e| format!("spawn cc: {e} (set VERYL_AOT_CC to override)"))?;
        if !out.status.success() {
            // Leak the .c on failure so the user can inspect it; remove
            // any stale .so to keep the cache consistent.
            let _ = std::fs::remove_file(&so_path);
            return Err(format!(
                "cc {} failed: {}\n{}",
                c_path.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr),
            ));
        }
    }

    // SAFETY: the .so was just compiled by us (or previously cached) and
    // exposes only `veryl_aot_eval`.  We never unload while the FuncPtr
    // is in use because EmittedModule keeps the library alive.
    let lib = unsafe { libloading::Library::new(&so_path) }
        .map_err(|e| format!("dlopen {}: {}", so_path.display(), e))?;
    let func: FuncPtr = unsafe {
        *lib.get::<FuncPtr>(b"veryl_aot_eval\0")
            .map_err(|e| format!("dlsym veryl_aot_eval: {e}"))?
    };
    // Publish the wide-op helper table into the .so so emitted wide-op calls
    // dispatch to the same `wide_ops::*` Rust helpers Cranelift uses.  The
    // setter is always present (decls emitted unconditionally) and copies the
    // table into the .so's global; unused on narrow-only modules.
    if let Ok(setter) =
        unsafe { lib.get::<unsafe extern "C" fn(*const c_void)>(b"veryl_set_wideops\0") }
    {
        let table = wideops_table();
        unsafe { setter(&table as *const WideOpsTable as *const c_void) };
    }
    // Event modules that emitted $display/$write expose `veryl_set_sysfn_cb`;
    // wire it to the Rust formatter so their output goes through `output_buffer`
    // (byte-identical, correctly buffered).  Absent on comb / sysfn-free
    // modules, where the dlsym simply fails and we skip.
    if let Ok(setter) =
        unsafe { lib.get::<unsafe extern "C" fn(*mut c_void)>(b"veryl_set_sysfn_cb\0") }
    {
        let cb: unsafe extern "C" fn(*const u8, usize, *const u64, *const u32, usize, u32) =
            veryl_aot_sysfn_print;
        unsafe { setter(cb as *mut c_void) };
    }
    Ok(EmittedModule { func, _lib: lib })
}

fn aot_c_cache_dir() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("VERYL_AOT_CACHE_DIR") {
        return Ok(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or("neither XDG_CACHE_HOME nor HOME set")?;
    Ok(base.join("veryl").join("aot_c"))
}

/// FNV-1a 64-bit (hex), with a 0xFF separator between parts so e.g.
/// `["ab","c"]` and `["a","bc"]` differ.
fn fnv1a_64_hex_parts(parts: &[&str]) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h: u64 = FNV_OFFSET;
    for part in parts {
        for &b in part.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
        // Domain separator between parts.
        h ^= 0xff;
        h = h.wrapping_mul(FNV_PRIME);
    }
    format!("{h:016x}")
}

/// Full C source for a comb statement sequence.  Signature matches the
/// Cranelift FuncPtr ABI: `void veryl_aot_eval(uint8_t *ff, uint8_t
/// *comb, uint64_t *log)`.  Comb-target writes store directly;
/// FF-target writes push WriteLogEntries like the event path.
pub fn emit_function(stmts: &[ProtoStatement]) -> Option<String> {
    reset_wide_tmp();
    // Splitting the monolithic body into ~chunk_size-stmt static functions
    // gives gcc -O3 smaller register-allocation and stack-frame scopes per
    // chunk and bounds spill locality (the unsplit body regresses L1d
    // locality).  chunk_size=0 disables splitting (single-function emit).
    // Override via VERYL_AOT_C_CHUNK_SIZE.
    let chunk_size: usize = std::env::var("VERYL_AOT_C_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(900);

    let mut body = String::new();
    body.push_str(
        "// AOT-C generated; do not edit.\n\
         #include <stdint.h>\n\
         typedef __uint128_t veryl_u128_ua __attribute__((__aligned__(1)));\n\
         typedef uint64_t veryl_u64_ua __attribute__((__aligned__(1)));\n",
    );
    body.push_str(WIDEOPS_C_DECLS);
    body.push_str(WIDEOPS_C_INLINE);
    body.push('\n');

    // Emit each chunk's stmts now so we can fail fast on unsupported.
    let chunks: Vec<&[ProtoStatement]> = if chunk_size == 0 || stmts.len() <= chunk_size {
        vec![stmts]
    } else {
        stmts.chunks(chunk_size).collect()
    };
    let mut chunk_bodies: Vec<String> = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        let mut cb = String::new();
        for stmt in *chunk {
            let s = emit_stmt(stmt)?;
            cb.push_str("    ");
            cb.push_str(&s);
            cb.push('\n');
        }
        chunk_bodies.push(cb);
    }

    if chunks.len() == 1 {
        body.push_str(
            "__attribute__((visibility(\"default\")))\n\
             void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log) {\n\
             \x20   (void)write_log;\n",
        );
        body.push_str(&chunk_bodies[0]);
        body.push_str("}\n");
    } else {
        // Each chunk → noinline static function so gcc isolates its
        // regalloc/spill domain.  -flto can still inline if it judges
        // the cost worthwhile, but for the typical 800-stmt heliodor
        // chunk it preserves the boundary.
        for (i, cb) in chunk_bodies.iter().enumerate() {
            body.push_str(&format!(
                "static __attribute__((noinline)) \
                 void veryl_aot_chunk_{i}(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log) {{\n\
                 \x20   (void)write_log;\n",
            ));
            body.push_str(cb);
            body.push_str("}\n\n");
        }
        body.push_str(
            "__attribute__((visibility(\"default\")))\n\
             void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log) {\n",
        );
        for i in 0..chunks.len() {
            body.push_str(&format!(
                "    veryl_aot_chunk_{i}(ff_values, comb_values, write_log);\n",
            ));
        }
        body.push_str("}\n");
    }
    Some(body)
}

/// One terminated C statement from a `ProtoStatement`.  `None` if
/// the variant or its substructures aren't emittable.
pub fn emit_stmt(stmt: &ProtoStatement) -> Option<String> {
    match stmt {
        ProtoStatement::Assign(a) => {
            // Route every FF write through the shadow-slot + WriteLogEntry path
            // (matching Cranelift) — a bare shadow store is never committed, so
            // the value is lost.  Needed in the comb path too: the is_ff
            // refinement can land an FF write here (e.g. function output args).
            // emit_event_ff_assign returns None on uncovered patterns, safely
            // bailing the module to Cranelift.
            if a.dst.is_ff() {
                return emit_event_ff_assign(a);
            }
            // A runtime-indexed bit-slice store into a comb target isn't
            // emitted (FF targets were handled above); bail to Cranelift.
            if a.dynamic_select.is_some() {
                return None;
            }
            // Wide (>128-bit) comb store via the wide-op helper table.  The
            // 65-128 bit range is already handled by the `__uint128_t` path
            // below; only >128 needs the flat-buffer pointer + helpers.
            // Select / rhs_select wide stores stay on Cranelift (S1).
            if a.dst_width > 128 {
                // rhs_select on a wide store stays on Cranelift (rare);
                // dynamic_select bailed above.
                if a.rhs_select.is_some() {
                    return None;
                }
                let VarOffset::Comb(store_off) = a.dst else {
                    return None;
                };
                if store_off < 0 {
                    return None;
                }
                let nb = native_bytes(a.dst_width);
                let nw = nb / 8;
                let mut pre = String::new();
                let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
                let dst = format!("(uint8_t*)(comb_values + {store_off:#x})");
                let dmask = wpack(nb, a.dst_width);
                if let Some((hi, lo)) = a.select {
                    // Wide bit-select WRITE (RMW, 2-state):
                    //   new = (old & ~rangemask) | ((src << lo) & rangemask)
                    // where rangemask = fill_ones(nbits) << lo.  `old` is read
                    // from the destination BEFORE the final copy overwrites it.
                    // Mirrors Cranelift emit_wide_select_rmw.
                    let nbits = hi.checked_sub(lo)?.checked_add(1)?;
                    let rmask = next_wide_tmp();
                    let srcsh = next_wide_tmp();
                    let newv = next_wide_tmp();
                    pre.push_str(&format!(
                        "uint64_t _w{rmask}[{nw}]; \
                         vw_fill_ones((uint8_t*)_w{rmask}, (const uint8_t*)0, {pkn}u); \
                         vw_shl((uint8_t*)_w{rmask}, (const uint8_t*)_w{rmask}, {lo}ull, {nb}u); \
                         uint64_t _w{srcsh}[{nw}]; \
                         vw_shl((uint8_t*)_w{srcsh}, {src}, {lo}ull, {nb}u); \
                         vw_band((uint8_t*)_w{srcsh}, (const uint8_t*)_w{srcsh}, (const uint8_t*)_w{rmask}, {nb}u); \
                         uint64_t _w{newv}[{nw}]; \
                         vw_band_not((uint8_t*)_w{newv}, {dst}, (const uint8_t*)_w{rmask}, {nb}u); \
                         vw_bor((uint8_t*)_w{newv}, (const uint8_t*)_w{newv}, (const uint8_t*)_w{srcsh}, {nb}u); ",
                        pkn = wpack(nb, nbits),
                        src = r.addr,
                        dst = dst,
                    ));
                    return Some(format!(
                        "{{ {pre}vw_copy({dst}, (const uint8_t*)_w{newv}, {nb}u); \
                            vw_apply_mask({dst}, (const uint8_t*)0, {dmask}u); }}"
                    ));
                }
                // No select: plain wide store.  Copy into the destination, then
                // mask THERE (never the source, which may alias a flat-buffer
                // variable read).
                return Some(format!(
                    "{{ {pre}vw_copy({dst}, {src}, {nb}u); \
                        vw_apply_mask({dst}, (const uint8_t*)0, {dmask}u); }}",
                    src = r.addr,
                ));
            }
            let nb = native_bytes(a.dst_width);
            let cty = native_c_type(nb)?;
            // Compute the rhs after rhs_select extraction (mirrors
            // AssignStatement::eval_step's `value.select(beg, end)`).
            let rhs_unselected = emit_expr_root(&a.expr)?;
            let rhs_str = match a.rhs_select {
                None => rhs_unselected,
                Some((rhs_hi, rhs_lo)) => {
                    let nbits = rhs_hi.checked_sub(rhs_lo)?.checked_add(1)?;
                    if nbits >= 64 {
                        return None;
                    }
                    let mask = (1u64 << nbits) - 1;
                    format!(
                        "((({src}) >> {lo}) & 0x{m:x}ULL)",
                        src = rhs_unselected,
                        lo = rhs_lo,
                        m = mask,
                    )
                }
            };
            // FF targets returned via emit_event_ff_assign above, so the
            // destination here is always comb.
            let VarOffset::Comb(store_off) = a.dst else {
                return None;
            };
            let buf = "comb_values";
            // Bit-select store is read-modify-write.
            if let Some((hi, lo)) = a.select {
                let nbits = hi.checked_sub(lo)?.checked_add(1)?;
                // The masked-store math below works in a single u64, so the
                // selected field must fit there.  Wide (>64-bit) selects — e.g.
                // the high chunks of a reversed wide bus, where `lo` itself is
                // ≥ 64 — would overflow `1u64 << nbits` / `<< lo`; bail to
                // Cranelift instead (which handles wide values per word).
                if nbits >= 64 || lo >= 64 || lo + nbits > 64 {
                    return None;
                }
                let value_mask = (1u64 << nbits) - 1;
                let pos_mask = value_mask << lo;
                Some(format!(
                    "{{ uint64_t _v = ((uint64_t)({rhs})) & 0x{vmask:x}ULL; \
                        {ct} _o = *(({ct}*)({b} + {o:#x})); \
                        *(({ct}*)({b} + {o:#x})) = ({ct})((_o & ({ct})(~(uint64_t)0x{pmask:x}ULL)) | ({ct})(_v << {lo})); }}",
                    rhs = rhs_str,
                    vmask = value_mask,
                    ct = cty,
                    b = buf,
                    o = store_off,
                    pmask = pos_mask,
                    lo = lo,
                ))
            } else {
                // Mask the stored value to its declared width when narrower
                // than the native storage type: a sign-extended rhs (e.g.
                // (int64_t)negative cast back to uint32_t) otherwise leaves
                // bits above the declared width set, whereas Cranelift masks
                // to declared bits before storing.
                let native_bits = nb * 8;
                if a.dst_width > 64 && a.dst_width < native_bits {
                    // Wide (65-127 bit) dst: mask in 128-bit arithmetic; a
                    // (uint64_t) cast here would drop the high bits.
                    let mask: u128 = (1u128 << a.dst_width) - 1;
                    Some(format!(
                        "*(({ct}*)({b} + {o:#x})) = ({ct})(((__uint128_t)({rhs})) \
                         & (((__uint128_t)0x{hi:x}ULL << 64) | (__uint128_t)0x{lo:x}ULL));",
                        ct = cty,
                        b = buf,
                        o = store_off,
                        rhs = rhs_str,
                        hi = (mask >> 64) as u64,
                        lo = mask as u64,
                    ))
                } else if a.dst_width < native_bits && a.dst_width > 0 {
                    let mask = (1u64 << a.dst_width) - 1;
                    Some(format!(
                        "*(({ct}*)({b} + {o:#x})) = ({ct})(((uint64_t)({rhs})) & 0x{m:x}ULL);",
                        ct = cty,
                        b = buf,
                        o = store_off,
                        rhs = rhs_str,
                        m = mask,
                    ))
                } else {
                    Some(format!(
                        "*(({ct}*)({b} + {o:#x})) = ({ct})({rhs});",
                        ct = cty,
                        b = buf,
                        o = store_off,
                        rhs = rhs_str,
                    ))
                }
            }
        }
        ProtoStatement::If(if_stmt) => {
            // Mirror the interpreter's IfStatement::eval_step semantics:
            // when `cond == None` the block runs the false_side
            // unconditionally (cond evaluates to false).  When `cond ==
            // Some`, emit a regular if/else.  Returning None for any
            // sub-stmt that the emitter can't handle keeps callers
            // safely on the Cranelift fallback.
            let true_body = emit_block(&if_stmt.true_side)?;
            let false_body = emit_block(&if_stmt.false_side)?;
            match &if_stmt.cond {
                None => Some(format!("{{ {} }}", false_body)),
                Some(cond) => {
                    let c = emit_expr(cond)?;
                    Some(format!(
                        "if ({c}) {{ {t} }} else {{ {f} }}",
                        c = c,
                        t = true_body,
                        f = false_body,
                    ))
                }
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            let inner = emit_block(body)?;
            Some(format!("{{ {} }}", inner))
        }
        ProtoStatement::AssignDynamic(a) => {
            // Event-path dynamic FF write (e.g. register file by rd index):
            // direct element store + WriteLogEntry push.
            if event_mode() && a.dst_base.is_ff() {
                return emit_event_ff_assign_dynamic(a);
            }
            // Mirror ProtoAssignDynamicStatement::eval_step (comb target).
            // `select` is supported as a runtime-addressed RMW; dynamic_select
            // (runtime bit position) is still out of scope.
            if a.dynamic_select.is_some() {
                return None;
            }
            if a.dst_base.is_ff() {
                return None; // handled above in event mode; else out of scope
            }
            // Wide (>128-bit) dynamic-indexed comb store via the wide-op
            // helper table.  A `var` array written by runtime index inside
            // always_ff whose ff_log_base_current_offset is None maps to the
            // comb buffer, so eval_step writes DIRECTLY to `base + stride*idx`
            // with no write-log push.  Mirror that byte for byte (RMW for
            // select, copy+mask for full).  The 65-128 range still bails below.
            if a.dst_width > 128 {
                if a.dynamic_select.is_some() || a.rhs_select.is_some() {
                    return None;
                }
                let VarOffset::Comb(base_off) = a.dst_base else {
                    return None;
                };
                if base_off < 0 || a.dst_num_elements == 0 {
                    return None;
                }
                let nb = native_bytes(a.dst_width);
                let nw = nb / 8;
                let max_idx = a.dst_num_elements.saturating_sub(1);
                let idx_str = emit_expr(&a.dst_index_expr)?;
                let dmask = wpack(nb, a.dst_width);
                let mut pre = String::new();
                let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
                // `_pa` is the element byte-address; declared in the block below
                // before the wide ops reference it.  `pre` (the RHS scratch)
                // does not reference `_pa`/`_idx`, so the ordering is sound.
                let store = if let Some((hi, lo)) = a.select {
                    // Runtime-addressed wide bit-select RMW (2-state):
                    //   new = (old & ~rangemask) | ((src << lo) & rangemask)
                    // Mirrors the static wide-store RMW (Cranelift parity).
                    let nbits = hi.checked_sub(lo)?.checked_add(1)?;
                    let rmask = next_wide_tmp();
                    let srcsh = next_wide_tmp();
                    let newv = next_wide_tmp();
                    format!(
                        "{pre}\
                         uint64_t _w{rmask}[{nw}]; \
                         vw_fill_ones((uint8_t*)_w{rmask}, (const uint8_t*)0, {pkn}u); \
                         vw_shl((uint8_t*)_w{rmask}, (const uint8_t*)_w{rmask}, {lo}ull, {nb}u); \
                         uint64_t _w{srcsh}[{nw}]; \
                         vw_shl((uint8_t*)_w{srcsh}, {src}, {lo}ull, {nb}u); \
                         vw_band((uint8_t*)_w{srcsh}, (const uint8_t*)_w{srcsh}, (const uint8_t*)_w{rmask}, {nb}u); \
                         uint64_t _w{newv}[{nw}]; \
                         vw_band_not((uint8_t*)_w{newv}, _pa, (const uint8_t*)_w{rmask}, {nb}u); \
                         vw_bor((uint8_t*)_w{newv}, (const uint8_t*)_w{newv}, (const uint8_t*)_w{srcsh}, {nb}u); \
                         vw_copy(_pa, (const uint8_t*)_w{newv}, {nb}u); \
                         vw_apply_mask(_pa, (const uint8_t*)0, {dmask}u);",
                        pkn = wpack(nb, nbits),
                        src = r.addr,
                    )
                } else {
                    // Full element write: copy then mask in the destination
                    // (never the source, which may alias a flat-buffer read).
                    format!(
                        "{pre}vw_copy(_pa, {src}, {nb}u); \
                         vw_apply_mask(_pa, (const uint8_t*)0, {dmask}u);",
                        src = r.addr,
                    )
                };
                return Some(format!(
                    "{{ uint64_t _idx_raw = (uint64_t)({idx}); \
                        uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                        uint8_t* _pa = (uint8_t*)(comb_values + {base:#x} + (intptr_t){stride} * (intptr_t)_idx); \
                        {store} }}",
                    idx = idx_str,
                    max = max_idx,
                    base = base_off,
                    stride = a.dst_stride,
                ));
            }
            if a.dst_num_elements == 0 || a.dst_width == 0 || a.dst_width > 64 {
                return None;
            }
            let nb = native_bytes(a.dst_width);
            let cty = native_c_type(nb)?;
            let base_off = match a.dst_base {
                VarOffset::Comb(o) => o,
                VarOffset::Ff(_) => unreachable!(),
            };
            let rhs = apply_rhs_select(emit_expr_root(&a.expr)?, a.rhs_select)?;
            let idx_str = emit_expr(&a.dst_index_expr)?;
            let max_idx = a.dst_num_elements.saturating_sub(1);
            let addr = format!(
                "(comb_values + {off:#x} + (intptr_t){stride} * (intptr_t)_idx)",
                off = base_off,
                stride = a.dst_stride,
            );
            // GCC statement-expression: clamp the index once, then store.
            let store = if let Some((hi, lo)) = a.select {
                let nbits = hi.checked_sub(lo)?.checked_add(1)?;
                if nbits >= 64 {
                    return None;
                }
                let vmask = (1u64 << nbits) - 1;
                let pmask = vmask << lo;
                // Runtime-addressed read-modify-write bit-select store.
                format!(
                    "{ct}* _p = ({ct}*){addr}; {ct} _o = *_p; \
                     *_p = ({ct})((_o & ({ct})(~(uint64_t)0x{pm:x}ULL)) | \
                          ({ct})((((uint64_t)({rhs})) & 0x{vm:x}ULL) << {lo}));",
                    ct = cty,
                    addr = addr,
                    pm = pmask,
                    rhs = rhs,
                    vm = vmask,
                    lo = lo,
                )
            } else {
                let dwmask = width_mask(a.dst_width);
                format!(
                    "*(({ct}*){addr}) = ({ct})(((uint64_t)({rhs})) & 0x{m:x}ULL);",
                    ct = cty,
                    addr = addr,
                    rhs = rhs,
                    m = dwmask,
                )
            };
            Some(format!(
                "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                    uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                    {store} }});",
                idx = idx_str,
                max = max_idx,
                store = store,
            ))
        }
        ProtoStatement::CompiledBlock(cb) => {
            // Walk the original pre-chunk ProtoStatements into the eval body,
            // bypassing `cb.func`, so gcc keeps values in registers across the
            // chunk boundary instead of spilling at a func-ptr call.
            //
            // The original_stmts carry canonical offsets (ff/comb base
            // delta == 0).  At runtime the same compiled code is called
            // with adjusted base pointers; here we instead shift the offsets
            // in the cloned IR by ff_delta_bytes / comb_delta_bytes so the
            // emitted code addresses the right bytes off the canonical buffers.
            let mut s = String::from("{ ");
            for stmt in &cb.original_stmts {
                let mut adjusted = stmt.clone();
                adjusted.adjust_offsets(cb.ff_delta_bytes, cb.comb_delta_bytes);
                let inner = emit_stmt(&adjusted)?;
                s.push_str(&inner);
                s.push(' ');
            }
            s.push('}');
            Some(s)
        }
        ProtoStatement::For(for_stmt) => emit_for(for_stmt),
        ProtoStatement::Break => Some("break;".to_string()),
        ProtoStatement::SystemFunctionCall(call) => {
            // Event path: emit $display/$write as a call into the Rust formatter
            // (veryl_sysfn_cb) so a single rare trace statement no longer forces
            // the whole clock event onto Cranelift.  $finish/$assert/$readmemh
            // affect sim state / need richer handling and stay on Cranelift.
            // Comb path has no output side effects, so bail there as before.
            if event_mode() {
                match call {
                    ProtoSystemFunctionCall::Display { format_str, args } => {
                        emit_event_print(format_str, args, true)
                    }
                    ProtoSystemFunctionCall::Write { format_str, args } => {
                        emit_event_print(format_str, args, false)
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
        ProtoStatement::TbMethodCall { .. } => {
            // ClockNext / ResetAssert advance simulation timeline; the
            // testbench Module that contains them stays on the
            // Cranelift dispatch path.
            None
        }
    }
}

/// `ProtoStatement::For` → C `for` loop.  Requires constant Forward
/// range with loop var ≤ 64 bits; falls back otherwise.
fn emit_for(for_stmt: &ProtoForStatement) -> Option<String> {
    let (start, end_excl, step) = match &for_stmt.range {
        ProtoForRange::Forward {
            start,
            end,
            inclusive,
            step,
        } => {
            let s = match start {
                ProtoForBound::Const(v) => *v,
                ProtoForBound::Dynamic(_) => return None,
            };
            let e = match end {
                ProtoForBound::Const(v) => *v,
                ProtoForBound::Dynamic(_) => return None,
            };
            // Mirror the interpreter: inclusive bumps end by 1 before
            // the i < end comparison.
            let e_excl = if *inclusive { e.checked_add(1)? } else { e };
            (s, e_excl, *step)
        }
        ProtoForRange::Reverse { .. } | ProtoForRange::Stepped { .. } => return None,
    };
    if step == 0 {
        return None; // would be an infinite loop
    }
    if for_stmt.var_width == 0 || for_stmt.var_width > 64 {
        return None;
    }
    let nb = native_bytes(for_stmt.var_width);
    let cty = native_c_type(nb)?;
    let (buf, off) = match for_stmt.var_offset {
        VarOffset::Ff(o) => ("ff_values", o),
        VarOffset::Comb(o) => ("comb_values", o),
    };
    // Body: walk each ProtoStatement, fail fast on unsupported.
    let mut body = String::new();
    for s in &for_stmt.body {
        body.push_str(&emit_stmt(s)?);
        body.push(' ');
    }
    Some(format!(
        "for (uint64_t _it = {start}ULL; _it < {end}ULL; _it += {step}ULL) {{ \
            *(({ct}*)({b} + {off:#x})) = ({ct})_it; \
            {body} \
        }}",
        start = start,
        end = end_excl,
        step = step,
        ct = cty,
        b = buf,
        off = off,
        body = body,
    ))
}

/// Flat statement sequence → one C-source body.  A single failure
/// propagates `None`.
fn emit_block(stmts: &[ProtoStatement]) -> Option<String> {
    let mut s = String::new();
    for st in stmts {
        s.push_str(&emit_stmt(st)?);
        s.push(' ');
    }
    Some(s)
}

/// `ProtoExpression` → parenthesized C expression (typed `uint64_t`;
/// width truncation happens at store time via the dst cast).  `None`
/// if the variant or operator isn't supported.
pub fn emit_expr(expr: &ProtoExpression) -> Option<String> {
    emit_expr_inner(expr, true)
}

/// Like `emit_expr`, but the caller guarantees it ignores result bits at or
/// above the expression's declared width (a store that re-masks to dst_width,
/// or a sign-extension that discards them).  Lets the producer-side width
/// mask be elided.  `needs_clean` then propagates down: a width-growing op's
/// result mask is emitted only when some consumer actually reads those high
/// bits (comparison, shift, concat, …).  See `binary_result_masked_to_width`.
pub fn emit_expr_root(expr: &ProtoExpression) -> Option<String> {
    emit_expr_inner(expr, false)
}

fn emit_expr_inner(expr: &ProtoExpression, needs_clean: bool) -> Option<String> {
    match expr {
        ProtoExpression::Value {
            value,
            width,
            expr_context,
        } => {
            // The Veryl analyzer encodes unsized literals (`'0`, `'1`, `'x`,
            // `'z`) as `Value::U64 { width: 0, ... }`; the actual numeric
            // value is the payload bit pattern repeated to fill the
            // surrounding expression context's width.  emit_value with
            // width=0 would otherwise mask everything to zero, which makes
            // `x == '1` evaluate as `x == 0` (a real bug).  Detect the
            // all-ones case here and fill to expr_context.width.
            let mut effective_width = *width;
            if effective_width == 0
                && let Value::U64(v) = value
                && v.width == 0
                && v.payload != 0
                && expr_context.width > 0
            {
                effective_width = expr_context.width.min(128);
            }
            emit_value(value, effective_width)
        }
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            width,
            var_full_width,
            ..
        } => {
            if let Some(dyn_sel) = dynamic_select {
                // Mirror Expression::Variable::eval with dynamic_select:
                //   load full underlying var, idx = clamp(index_expr),
                //   shift right by idx*elem_width, mask elem_width bits.
                if *var_full_width == 0 || *var_full_width > 64 {
                    return None;
                }
                if dyn_sel.elem_width == 0 || dyn_sel.elem_width >= 64 {
                    return None;
                }
                if dyn_sel.num_elements == 0 {
                    return None;
                }
                let load = emit_var_load(var_offset, *var_full_width)?;
                let idx_str = emit_expr(&dyn_sel.index_expr)?;
                let max_idx = dyn_sel.num_elements.saturating_sub(1);
                let mask = (1u64 << dyn_sel.elem_width) - 1;
                return Some(format!(
                    "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                        uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                        ((({load}) >> (_idx * {ew})) & 0x{mask:x}ULL); }})",
                    idx = idx_str,
                    max = max_idx,
                    load = load,
                    ew = dyn_sel.elem_width,
                    mask = mask,
                ));
            }
            // Wide (>128-bit) underlying variable.  A static narrow (≤64-bit)
            // bit-select extracts a scalar via a funnel-shift+mask read of the
            // flat buffer; a no-select read (full wide value) or a wider-than-
            // 64 select is not a C scalar here (handled by emit_wide_expr in a
            // wide context, or bails).
            if *var_full_width > 128 {
                if let Some((hi, lo)) = select {
                    let nbits = hi.checked_sub(*lo)?.checked_add(1)?;
                    if nbits <= 64 {
                        let (buf, off) = match var_offset {
                            VarOffset::Ff(o) => ("ff_values", *o),
                            VarOffset::Comb(o) => ("comb_values", *o),
                        };
                        if off < 0 {
                            return None;
                        }
                        return Some(emit_wide_var_select_read(buf, off, *lo, nbits));
                    }
                }
                return None;
            }
            // Bit-select reads must load enough bytes to cover the high
            // bit being extracted. Using `*width` (the select bit-count)
            // would cast at native_bytes(nbits) and miss high bytes when
            // hi >= 8 (e.g. mantissa_a width=52 with select=(51,51)).
            // Use the underlying var's full width for the load cast.
            let load_width = if let Some((hi, _)) = select {
                (*hi + 1).max(*width)
            } else {
                *width
            };
            let load = emit_var_load(var_offset, load_width)?;
            if let Some((hi, lo)) = select {
                let nbits = hi.checked_sub(*lo)?.checked_add(1)?;
                if nbits > 64 {
                    return None; // wider-than-64 select → wide result, not a scalar
                }
                if nbits == 64 {
                    // Exactly 64 bits: no mask (`1u64 << 64` overflows); the
                    // shift drops the low `lo` bits and the cast keeps 64.
                    Some(format!(
                        "((uint64_t)(({load}) >> {lo}))",
                        load = load,
                        lo = lo
                    ))
                } else {
                    let mask = (1u64 << nbits) - 1;
                    Some(format!(
                        "((({load}) >> {lo}) & 0x{mask:x}ULL)",
                        load = load,
                        lo = lo,
                        mask = mask,
                    ))
                }
            } else {
                Some(load)
            }
        }
        ProtoExpression::Unary {
            op,
            x,
            expr_context,
            ..
        } => {
            // Wide (>128-bit) operand: the only scalar-producing wide unary is
            // a reduction (BitAnd/BitOr/BitXor/…/LogicNot → 1-bit).  A wide
            // non-reduction (BitNot/Sub) yields a wide value that can't be a C
            // scalar here, so emit_wide_reduce_unary returns None → the module
            // bails to Cranelift (which handles it).
            if x.width() > 128 {
                return emit_wide_reduce_unary(*op, x);
            }
            let xs = emit_expr(x)?;
            // A narrow signed operand loaded as uint64_t is zero-extended, so
            // `-`/`~` leave wrong high bits (e.g. `- 8'shf6` = -(-10) is
            // 0x000a, not 0xff0a).  Sign-extend when signed (like the Binary
            // arm); store-time masking trims to the declared width.
            let xw = x.width();
            let xv = if expr_context.signed && xw > 0 && xw < 64 {
                let shift = 64 - xw;
                format!(
                    "(((int64_t)((uint64_t)({}) << {})) >> {})",
                    xs, shift, shift
                )
            } else {
                format!("((int64_t)((uint64_t)({})))", xs)
            };
            match op {
                // LogicNot yields 0/1 regardless of signedness.
                Op::LogicNot => Some(format!("(!({}))", xs)),
                Op::BitNot => Some(format!("(~({}))", xv)),
                Op::Sub => Some(format!("(-({}))", xv)),
                _ => None, // unsupported
            }
        }
        ProtoExpression::Binary {
            x,
            op,
            y,
            expr_context,
            ..
        } => {
            // Wide (>128-bit) operand: the only scalar-producing wide binary
            // is a comparison/logic op (→ 1-bit).  A wide-result op (add/sub/
            // mul/bitwise/shift) yields a wide value that can't be a C scalar
            // here — emit_wide_cmp_binary returns None for those → bail to
            // Cranelift.  (Wide-result ops are materialized by emit_wide_expr
            // at the wide-store / wide-operand sites, never here.)
            if x.width() > 128 || y.width() > 128 {
                return emit_wide_cmp_binary(x, *op, y, expr_context);
            }
            // Signedness fix-ups: comparisons and Div / Rem need both
            // operands sign-extended to a signed integer wider than
            // their declared width so the C-level operator picks up
            // the right semantics.  Without this, a narrow signed
            // value loaded as uint64_t compares (or divides) as
            // unsigned and negative numbers look like very-large positives.
            let is_signed_cmp = expr_context.signed
                && matches!(op, Op::Less | Op::Greater | Op::LessEq | Op::GreaterEq);
            // Op::Div / Op::Rem use the AND of operand signedness, as the
            // Cranelift backend does.  expr_context.signed alone is not
            // sufficient because merge() with an unsigned sibling can
            // strip the bit even when both operands ARE signed.
            // We approximate by trusting expr_context.signed for the
            // outer expression — heliodor's div/rem are all
            // expr_context.signed when both operands are signed.
            let is_signed_divrem = expr_context.signed && matches!(op, Op::Div | Op::Rem);
            // Operands need pre-masking only where this op reads their high
            // bits. Add/Sub/Mul (low bits suffice; the result mask cleans the
            // rest) and signed compare/div/rem (operands sign-extended below)
            // don't; bitwise propagates `needs_clean`; the rest (unsigned
            // compare, shift, &&/||) need clean operands.
            let operand_needs_clean = if is_signed_cmp || is_signed_divrem {
                false
            } else {
                match op {
                    Op::Add | Op::Sub | Op::Mul => false,
                    Op::BitAnd
                    | Op::BitOr
                    | Op::BitXor
                    | Op::BitNand
                    | Op::BitNor
                    | Op::BitXnor => needs_clean,
                    _ => true,
                }
            };
            let xs = emit_expr_inner(x, operand_needs_clean)?;
            let ys = emit_expr_inner(y, operand_needs_clean)?;
            if is_signed_cmp || is_signed_divrem {
                let x_w = x.width();
                let y_w = y.width();
                if x_w == 0 || y_w == 0 || x_w > 64 || y_w > 64 {
                    // wide / zero-width signed compare.
                    return None;
                }
                let c_op = match op {
                    Op::Less => "<",
                    Op::Greater => ">",
                    Op::LessEq => "<=",
                    Op::GreaterEq => ">=",
                    Op::Div => "/",
                    Op::Rem => "%",
                    _ => unreachable!(),
                };
                let sext = |s: &str, w: usize| -> String {
                    if w == 64 {
                        format!("((int64_t)((uint64_t)({})))", s)
                    } else {
                        let shift = 64 - w;
                        format!("(((int64_t)((uint64_t)({}) << {})) >> {})", s, shift, shift,)
                    }
                };
                let inner = format!("(({}) {} ({}))", sext(&xs, x_w), c_op, sext(&ys, y_w),);
                // For Div / Rem we additionally guard against y == 0
                // (and x == INT64_MIN with y == -1) to mirror the
                // analyzer's checked-div fallback (as the Cranelift backend
                // does).  Wrap the divide in a statement expression so the
                // result drops to 0 on those edge cases; otherwise gcc's
                // -O3 div traps SIGFPE.
                if is_signed_divrem {
                    return Some(format!(
                        "({{ int64_t _y = {y}; int64_t _x = {x}; \
                            (_y == 0) ? (int64_t)0 : \
                            ((_y == -1 && _x == INT64_MIN) ? \
                                {fallback} : (_x {op} _y)); }})",
                        x = sext(&xs, x_w),
                        y = sext(&ys, y_w),
                        op = c_op,
                        fallback = if matches!(op, Op::Rem) { "0" } else { "_x" },
                    ));
                }
                return Some(inner);
            }
            // Most ops map directly.  ArithShiftR uses signed cast.
            let direct = match op {
                Op::Add => Some("+"),
                Op::Sub => Some("-"),
                Op::Mul => Some("*"),
                Op::Div => Some("/"),
                Op::Rem => Some("%"),
                Op::Eq => Some("=="),
                Op::Ne => Some("!="),
                // EqWildcard / NeWildcard reduce to Eq / Ne in 2-state
                // mode (heliodor uses 2-state — `mask_xz` is always 0
                // and the analyzer's eval becomes a plain payload diff,
                // see analyzer/op.rs::eval_value_binary Op::EqWildcard).
                // 4-state semantics would need an X-bit-aware emit; out
                // of scope until a 4-state target is added.
                Op::EqWildcard => Some("=="),
                Op::NeWildcard => Some("!="),
                Op::Less => Some("<"),
                Op::Greater => Some(">"),
                Op::LessEq => Some("<="),
                Op::GreaterEq => Some(">="),
                Op::LogicAnd => Some("&&"),
                Op::LogicOr => Some("||"),
                Op::BitAnd => Some("&"),
                Op::BitOr => Some("|"),
                Op::BitXor => Some("^"),
                Op::LogicShiftL | Op::ArithShiftL => Some("<<"),
                Op::LogicShiftR => Some(">>"),
                _ => None,
            };
            if let Some(c_op) = direct {
                // A >64-bit result computed in 64-bit C truncates. C promotes a
                // uint64_t Add/Sub/Mul operand to __uint128_t when the other is
                // already 128-bit, so only both-narrow truncate; a left shift
                // follows its left operand alone. Bail those (and signed wide,
                // which the block below can't sign-extend to 128) to Cranelift.
                let wide_truncates = match op {
                    Op::Add | Op::Sub | Op::Mul => x.width() <= 64 && y.width() <= 64,
                    Op::LogicShiftL | Op::ArithShiftL => x.width() <= 64,
                    _ => false,
                };
                if expr_context.width > 64 && (wide_truncates || expr_context.signed) {
                    return None;
                }
                // For 65..128-bit shifts the C operator uses a mod-128 count on
                // __uint128_t, so a runtime count >= width wrongly returns the
                // operand instead of 0. Guard with a ternary matching the
                // interpreter / SystemVerilog "count >= width => 0" semantics.
                if expr_context.width > 64
                    && expr_context.width <= 128
                    && matches!(op, Op::LogicShiftL | Op::ArithShiftL | Op::LogicShiftR)
                {
                    let w = expr_context.width;
                    return Some(format!(
                        "(((uint64_t)({ys})) >= {w} ? (__uint128_t)0 : (({xs}) {c_op} ({ys})))"
                    ));
                }
                // Operand-derived overflow predicate, computable in parallel
                // with the op. When it proves no carry past `width` the mask is
                // a no-op. Built here (not in `wmask`) so the closure doesn't
                // borrow `xs`/`ys`. Unsigned only (signed operands are
                // sign-extended, so a high bit no longer means large).
                let overflow_cond: Option<String> =
                    if expr_context.signed || expr_context.width == 0 || expr_context.width >= 64 {
                        None
                    } else {
                        // Shift by W-1 (not `& (1<<(W-1))`) so any operand bit at or
                        // above W-1 trips the predicate — this stays sound even when
                        // an operand is itself an unmasked (dirty) width-growing op
                        // whose bits ≥ W are nonzero.
                        let sh = expr_context.width - 1;
                        let w = expr_context.width;
                        match op {
                            // a|b has no bit ≥ W-1 ⇒ a,b < 2^(W-1) ⇒ a+b < 2^W.
                            Op::Add => Some(format!("((({xs}) | ({ys})) >> {sh})")),
                            // additionally a-b borrows (dirty) unless a >= b.
                            Op::Sub => Some(format!(
                                "(((({xs}) | ({ys})) >> {sh}) != 0 || ({xs}) < ({ys}))"
                            )),
                            // `x << n` overflows iff n reaches W or x has a bit ≥
                            // W-n; `n >= W` is tested first so `W - n` never
                            // underflows. (Mul has no cheap operand-only predicate,
                            // so it keeps the unconditional mask.)
                            Op::LogicShiftL | Op::ArithShiftL => Some(format!(
                                "(({ys}) >= {w} || ((({xs}) >> ({w} - ({ys}))) != 0))"
                            )),
                            _ => None,
                        }
                    };
                // Width-growing results can set bits ≥ width — harmless once
                // stored (the store re-masks) but they corrupt an inlined
                // comparison, so mask to width. With an operand-derived
                // predicate, gate the mask behind a rarely-taken branch to keep
                // it off the critical path; the `volatile` asm stops gcc from
                // if-converting it back to an unconditional `& mask`.
                let wmask = |s: String| -> String {
                    if needs_clean
                        && expr_context.width < 64
                        && matches!(
                            op,
                            Op::Add | Op::Sub | Op::Mul | Op::LogicShiftL | Op::ArithShiftL
                        )
                    {
                        let mask = (1u64 << expr_context.width) - 1;
                        match &overflow_cond {
                            Some(cond) => format!(
                                "({{ uint64_t _t = ({s}); \
                                 if (__builtin_expect(({cond}) != 0, 0)) {{ _t &= 0x{mask:x}ULL; \
                                 __asm__ volatile(\"\" : \"+r\"(_t)); }} _t; }})"
                            ),
                            None => format!("(({s}) & 0x{mask:x}ULL)"),
                        }
                    } else {
                        s
                    }
                };
                // Verilog binary ops widen operands to result width before
                // applying.  When signed, narrow operands must be sign-
                // extended to expr_context.width so e.g. signed `8'shf2 +
                // 8'shf2` in a 16-bit context produces 0xffe4, not 0x01e4.
                // Mirrors `expand_sign` in expression.rs.  Shifts:
                // y is the shift count and must NOT be sign-extended (its
                // narrow MSB is value, not sign); shift_left already keeps
                // bits faithfully so we only widen x.
                if expr_context.signed && expr_context.width > 0 && expr_context.width <= 64 {
                    let x_w = x.width();
                    let y_w = y.width();
                    let target = expr_context.width;
                    let sext = |s: &str, w: usize| -> String {
                        if w == 0 || w >= target {
                            s.to_string()
                        } else {
                            let shift = 64 - w;
                            format!("(((int64_t)((uint64_t)({}) << {})) >> {})", s, shift, shift,)
                        }
                    };
                    let is_shift = matches!(
                        op,
                        Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR
                    );
                    let xe = sext(&xs, x_w);
                    let ye = if is_shift { ys } else { sext(&ys, y_w) };
                    // Verilog `>>` is logical even on a signed operand (only
                    // `>>>` is arithmetic).  Sign-extend to the context width,
                    // mask to it, then shift unsigned — sign-extending to 64
                    // and using C `>>` on an int64 shifts in sign bits (e.g.
                    // `8'shf1 >> 2` in 16-bit is 0x3ffc, not 0xfffc).
                    if matches!(op, Op::LogicShiftR) {
                        let tmask = if target >= 64 {
                            u64::MAX
                        } else {
                            (1u64 << target) - 1
                        };
                        return Some(format!(
                            "(((uint64_t)(({}) & 0x{:x}ULL)) >> ({}))",
                            xe, tmask, ye,
                        ));
                    }
                    return Some(wmask(format!("(({}) {} ({}))", xe, c_op, ye)));
                }
                return Some(wmask(format!("(({}) {} ({}))", xs, c_op, ys)));
            }
            match op {
                Op::ArithShiftR => {
                    // Sign-extend the narrow operand from its declared width
                    // before the arithmetic shift — otherwise the high bits are
                    // zero and `>>` produces 0, not the sign-extended value
                    // (mirrors expression.rs shift_mask_xz).
                    let x_w = x.width();
                    if x_w == 0 || x_w > 64 {
                        return None; // wide / zero-width signed shift
                    }
                    if !expr_context.signed {
                        // `>>>` on an *unsigned* operand is a logical
                        // (zero-fill) shift — only a signed operand gets
                        // sign-extended.  e.g. `8'hf1 >>> 2` is 0x003c,
                        // not 0xfffc.
                        Some(format!("((uint64_t)({xs}) >> ({ys}))", xs = xs, ys = ys,))
                    } else if x_w == 64 {
                        Some(format!(
                            "((uint64_t)((int64_t)((uint64_t)({xs})) >> ({ys})))",
                            xs = xs,
                            ys = ys,
                        ))
                    } else {
                        let shift = 64 - x_w;
                        Some(format!(
                            "((uint64_t)((((int64_t)((uint64_t)({xs}) << {sh})) >> {sh}) >> ({ys})))",
                            xs = xs,
                            ys = ys,
                            sh = shift,
                        ))
                    }
                }
                Op::BitXnor => Some(format!("(~(({}) ^ ({})))", xs, ys)),
                Op::BitNand => Some(format!("(~(({}) & ({})))", xs, ys)),
                Op::BitNor => Some(format!("(~(({}) | ({})))", xs, ys)),
                // `As` is the type-cast op; the analyzer uses it to mark
                // a Binary{x, As, y_type} where y_type is a Type expression
                // (not a value).  At eval time the value passes through
                // unchanged (the analyzer's `Op::As` eval returns `x.clone()`); the
                // surrounding assignment / outer expression handles any
                // width truncation via the C target's type, so we emit
                // the operand directly.
                Op::As => Some(xs),
                _ => None, // Pow / EqWildcard / NeWildcard / etc.
            }
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            // The condition is a truthy test, so its high bits must be clean;
            // the selected branch becomes this result, so the branches inherit
            // `needs_clean`.
            let c = emit_expr(cond)?;
            let t = emit_expr_inner(true_expr, needs_clean)?;
            let f = emit_expr_inner(false_expr, needs_clean)?;
            Some(format!("(({}) ? ({}) : ({}))", c, t, f))
        }
        ProtoExpression::Concatenation {
            elements, width, ..
        } => {
            // Mirror Expression::Concatenation::eval: walk left-to-right
            // (`{a, b, c}` puts a in the high bits), accumulating
            // `acc = (acc << w) | (e & mask_w)` per element/repeat.  Per-element
            // width is the evaluated `val.width` (for Variable that's `width`,
            // for nested exprs `expr.width()`), not the ignored `_elem_width`.
            // Limit: total result width must fit in u64.  A repeat>1 element is
            // duplicated textually; gcc -O3 CSEs the repeated loads.
            if *width == 0 || *width > 128 {
                return None;
            }
            // For total widths >64 the accumulator must be __uint128_t
            // to hold the full result.  Sub-element widths still fit in
            // u64 (we cap each sub at 64 bits); the cast to u128
            // happens at OR-time so high bits aren't truncated.
            let wide_acc = *width > 64;

            // Fast path for a leading 1-bit repeat `{N{bit}}`: emit the JIT
            // `ineg` idiom `(0 - bit) & mask` instead of N nested shift+or
            // pairs.  gcc -O3 cannot collapse the textual expansion on its own,
            // and it blows up cc1's parse + IR-build cost.
            let first_is_bit_repeat = elements
                .first()
                .is_some_and(|(sub, repeat, _)| *repeat > 1 && sub.width() == 1);

            if first_is_bit_repeat && elements.len() == 1 {
                // Single-element 1-bit repeat `{N{bit}}` standalone.
                let sub_str = emit_expr(&elements[0].0)?;
                let mask: u128 = if *width >= 128 {
                    !0u128
                } else {
                    (1u128 << *width) - 1
                };
                if wide_acc {
                    let hi = (mask >> 64) as u64;
                    let lo = mask as u64;
                    return Some(format!(
                        "(((__uint128_t)0 - (__uint128_t)(((uint64_t)({sub})) & 0x1ULL)) & (((__uint128_t)0x{hi:x}ULL << 64) | (__uint128_t)0x{lo:x}ULL))",
                        sub = sub_str,
                        hi = hi,
                        lo = lo,
                    ));
                } else {
                    let mask64 = mask as u64;
                    return Some(format!(
                        "((uint64_t)(0ULL - (((uint64_t)({sub})) & 0x1ULL)) & 0x{mask64:x}ULL)",
                        sub = sub_str,
                        mask64 = mask64,
                    ));
                }
            }

            let mut acc = if wide_acc {
                String::from("((__uint128_t)0)")
            } else {
                String::from("0ULL")
            };

            if first_is_bit_repeat && elements.len() >= 2 {
                // Multi-element with leading 1-bit repeat:
                // `{N{sign}, field1, field2, ...}`. Build the lower
                // part from elements[1..], then fill the upper N bits
                // via `(0 - sign) << lower_width`, mirroring the Cranelift
                // concat lowering in expression.rs.
                let sign_str = emit_expr(&elements[0].0)?;
                let mut lower_width = 0usize;
                for (sub, repeat, elem_width) in &elements[1..] {
                    let sub_width = sub.width();
                    if sub_width == 0 || sub_width > 64 {
                        return None;
                    }
                    let sub_str = emit_expr(sub)?;
                    let mask = if sub_width >= 64 {
                        u64::MAX
                    } else {
                        (1u64 << sub_width) - 1
                    };
                    let ew = *elem_width;
                    for _ in 0..*repeat {
                        if wide_acc {
                            acc = format!(
                                "((({acc}) << {w}) | (((__uint128_t)({sub})) & (__uint128_t)0x{mask:x}ULL))",
                                acc = acc,
                                w = ew,
                                sub = sub_str,
                                mask = mask,
                            );
                        } else {
                            acc = format!(
                                "((({acc}) << {w}) | (({sub}) & 0x{mask:x}ULL))",
                                acc = acc,
                                w = ew,
                                sub = sub_str,
                                mask = mask,
                            );
                        }
                        lower_width += ew;
                    }
                }
                // Mask to total width to discard upper bits left by `(0 - sign)`.
                let mask: u128 = if *width >= 128 {
                    !0u128
                } else {
                    (1u128 << *width) - 1
                };
                if wide_acc {
                    let hi = (mask >> 64) as u64;
                    let lo = mask as u64;
                    return Some(format!(
                        "(((((__uint128_t)0 - (__uint128_t)(((uint64_t)({sign})) & 0x1ULL)) << {lw}) | ({acc})) & (((__uint128_t)0x{hi:x}ULL << 64) | (__uint128_t)0x{lo:x}ULL))",
                        sign = sign_str,
                        lw = lower_width,
                        acc = acc,
                        hi = hi,
                        lo = lo,
                    ));
                } else {
                    let mask64 = mask as u64;
                    return Some(format!(
                        "((((uint64_t)(0ULL - (((uint64_t)({sign})) & 0x1ULL)) << {lw}) | ({acc})) & 0x{mask64:x}ULL)",
                        sign = sign_str,
                        lw = lower_width,
                        acc = acc,
                        mask64 = mask64,
                    ));
                }
            }
            for (sub, repeat, _elem_width) in elements {
                let sub_width = sub.width();
                if sub_width == 0 || sub_width > 64 {
                    return None;
                }
                let sub_str = emit_expr(sub)?;
                let mask = if sub_width >= 64 {
                    u64::MAX
                } else {
                    (1u64 << sub_width) - 1
                };
                for _ in 0..*repeat {
                    if wide_acc {
                        acc = format!(
                            "((({acc}) << {w}) | (((__uint128_t)({sub})) & (__uint128_t)0x{mask:x}ULL))",
                            acc = acc,
                            w = sub_width,
                            sub = sub_str,
                            mask = mask,
                        );
                    } else {
                        acc = format!(
                            "((({acc}) << {w}) | (({sub}) & 0x{mask:x}ULL))",
                            acc = acc,
                            w = sub_width,
                            sub = sub_str,
                            mask = mask,
                        );
                    }
                }
            }
            Some(acc)
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
            // Mirror Expression::DynamicVariable::eval:
            //   idx = clamp(index_expr.to_usize(), 0..num_elements-1)
            //   ptr = base + stride * idx
            //   value = *((Tn*)ptr); if select: extract bits
            // Falls back to Cranelift for a nested dynamic_select or width > 64.
            if dynamic_select.is_some() {
                return None; // unsupported
            }
            // Wide (>16 native-byte) array element: a static narrow (≤64-bit)
            // bit-select reads a field via funnel-shift+mask off the dynamic
            // element address (`buf + base_off + stride*idx`).  A no-select /
            // wide-result read is handled by emit_wide_expr (wide context).
            if *element_native_bytes > 16 {
                if *num_elements == 0 {
                    return None;
                }
                if let Some((hi, lo)) = select {
                    let nbits = hi.checked_sub(*lo)?.checked_add(1)?;
                    if nbits <= 64 {
                        let (buf, base_off) = match base_offset {
                            VarOffset::Ff(o) => ("ff_values", *o),
                            VarOffset::Comb(o) => ("comb_values", *o),
                        };
                        let idx = emit_expr(index_expr)?;
                        let max_idx = num_elements.saturating_sub(1);
                        let addr = format!(
                            "({buf} + {base_off:#x} + (intptr_t){stride} * (intptr_t)_idx)"
                        );
                        let read = emit_wide_select_read_at(&addr, *lo, nbits);
                        return Some(format!(
                            "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                                uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                                {read}; }})",
                            max = max_idx,
                        ));
                    }
                }
                return None;
            }
            if *num_elements == 0 || *width == 0 || *width > 64 {
                return None;
            }
            // With a bit-select the field can sit at a non-zero offset within
            // the element (e.g. a struct field), so load enough bytes to cover
            // its top bit `hi`, not just its width — then `>> lo & mask`
            // extracts it.  No select: `width` is already the element width.
            let read_bits = match select {
                Some((hi, _lo)) => hi.checked_add(1)?,
                None => *width,
            };
            if read_bits > 64 {
                return None;
            }
            let nb_read = native_bytes(read_bits);
            let cty = native_c_type(nb_read)?;
            let (buf, base_off) = match base_offset {
                VarOffset::Ff(o) => ("ff_values", *o),
                VarOffset::Comb(o) => ("comb_values", *o),
            };
            let idx_str = emit_expr(index_expr)?;
            // Clamp at the C level — interpreter uses
            // `min(num_elements-1)`.  We materialize the idx into a
            // GCC statement expression so the index_expr is evaluated
            // exactly once and `idx` is reusable.  Compatible with
            // gcc/clang; we already require gcc to compile the .so.
            let max_idx = num_elements.saturating_sub(1);
            let load_expr = format!(
                "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                    uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                    (uint64_t)*((const {ct}*)({b} + {off:#x} + (intptr_t){stride} * (intptr_t)_idx)); }})",
                idx = idx_str,
                max = max_idx,
                ct = cty,
                b = buf,
                off = base_off,
                stride = stride,
            );
            if let Some((hi, lo)) = select {
                let nbits = hi.checked_sub(*lo)?.checked_add(1)?;
                if nbits > 64 {
                    return None;
                }
                let mask = if nbits >= 64 {
                    u64::MAX
                } else {
                    (1u64 << nbits) - 1
                };
                Some(format!(
                    "((({load}) >> {lo}) & 0x{mask:x}ULL)",
                    load = load_expr,
                    lo = lo,
                    mask = mask,
                ))
            } else {
                Some(load_expr)
            }
        }
    }
}

fn emit_var_load(var_offset: &VarOffset, width: usize) -> Option<String> {
    if width > 128 {
        return None; // > 128 bit
    }
    if width == 0 {
        // 0-width loads occur in heliodor (zero-width sentinels and
        // implicit-default reads); the interpreter treats them as
        // numeric 0, so we emit `(uint64_t)0` rather than allocating
        // a no-op load.
        return Some("((uint64_t)0)".to_string());
    }
    let nb = native_bytes(width);
    let cty = native_c_type(nb)?;
    let (buf, off) = match var_offset {
        VarOffset::Ff(o) => ("ff_values", *o),
        VarOffset::Comb(o) => ("comb_values", *o),
    };
    // Cast result to expr_c_type(width) so 65-128 loads materialize
    // as __uint128_t and ≤64 loads stay as uint64_t.  Storage type
    // matches both: `*(uint128_t*)ptr` reads 16 bytes.
    let result_ty = expr_c_type(width)?;
    Some(format!(
        "(({rt})*((const {ct}*)({b} + {o:#x})))",
        rt = result_ty,
        ct = cty,
        b = buf,
        o = off,
    ))
}

fn emit_value(value: &Value, width: usize) -> Option<String> {
    if width > 128 {
        return None;
    }
    match value {
        Value::U64(v) => {
            // width=0 occurs in heliodor (zero-width sentinels and
            // implicit-default constants); the interpreter treats them as
            // numeric zero, so emit 0ULL — except for the analyzer's encoding
            // of the unsized all-ones literal `'1` (`width: 0, payload != 0`):
            // when the parent context supplies a non-zero width we must emit
            // all-ones truncated to that width, not the raw payload.
            let payload: u128 = if v.width == 0 && v.payload != 0 && v.mask_xz == 0 && width > 0 {
                if width >= 128 {
                    !0u128
                } else {
                    (1u128 << width) - 1
                }
            } else {
                v.payload as u128
            };
            // Note: 65-128 bit U64 values fit in u64 storage with the
            // upper bits zero; we widen via __uint128_t cast.
            let masked: u128 = if width == 0 {
                0
            } else if width >= 128 {
                payload
            } else {
                payload & ((1u128 << width) - 1)
            };
            if width > 64 {
                // C has no 128-bit literal syntax; gcc/clang accept hex
                // literals only up to `unsigned long long` (64 bits).
                // Split into hi:lo and reassemble via shift+or.
                let hi = (masked >> 64) as u64;
                let lo = masked as u64;
                Some(format!(
                    "(((__uint128_t)0x{:x}ULL << 64) | (__uint128_t)0x{:x}ULL)",
                    hi, lo
                ))
            } else {
                Some(format!("0x{:x}ULL", masked as u64))
            }
        }
        Value::BigUint(_) => None, // BigUint constant > 64 bits
    }
}

fn native_c_type(nb: usize) -> Option<&'static str> {
    match nb {
        1 => Some("uint8_t"),
        2 => Some("uint16_t"),
        4 => Some("uint32_t"),
        8 => Some("uint64_t"),
        // 65-128 bit values use the GCC/clang __uint128_t extension (16-byte
        // storage, uint64 operands promote implicitly).  The pointer-cast type
        // must be the 1-byte-aligned alias `veryl_u128_ua` (C prologue): a
        // 128-bit value can sit at an 8-byte offset, where a bare
        // `__uint128_t*` deref SIGSEGVs (gcc emits an aligned SSE move).
        16 => Some("veryl_u128_ua"),
        _ => None, // > 128 bit
    }
}

/// `uint64_t` for ≤64, `__uint128_t` for 65-128.  Wider unsupported.
fn expr_c_type(width: usize) -> Option<&'static str> {
    if width == 0 || width <= 64 {
        Some("uint64_t")
    } else if width <= 128 {
        Some("__uint128_t")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ChunkArtifact;
    use crate::ir::{ExpressionContext, ProtoAssignStatement, ProtoSystemFunctionCall};
    use veryl_analyzer::value::ValueU64;
    use veryl_parser::token_range::TokenRange;

    fn dummy_token() -> TokenRange {
        TokenRange::default()
    }

    #[test]
    fn wideops_table_abi_is_consistent() {
        // The emitted C `veryl_wideops_t` struct, the `#[repr(C)] WideOpsTable`
        // mirror, and `wideops_table()` must agree on field count and order.
        // A reorder/insert in one but not the others silently dispatches wide
        // ops to the wrong helper.  Size pins the count (23 fn pointers); the
        // C decl lists exactly the same 23 names in the same order.
        assert_eq!(
            std::mem::size_of::<WideOpsTable>(),
            23 * std::mem::size_of::<usize>(),
            "WideOpsTable must be exactly 23 function pointers"
        );
        // Every slot must resolve to a real helper address (no zeroed field
        // from a forgotten `wideops_table()` entry).
        let t = wideops_table();
        for (i, &addr) in [
            t.band,
            t.bor,
            t.bxor,
            t.bxor_not,
            t.band_not,
            t.add,
            t.sub,
            t.mul,
            t.bnot,
            t.negate,
            t.copy,
            t.shl,
            t.lshr,
            t.ashr,
            t.eq,
            t.ne,
            t.ucmp,
            t.scmp,
            t.is_nonzero,
            t.is_all_ones,
            t.popcnt_parity,
            t.apply_mask,
            t.fill_ones,
        ]
        .iter()
        .enumerate()
        {
            assert_ne!(addr, 0, "wideops_table field {i} is null");
        }
        // The C struct declares the 23 fields in the documented order.
        for name in [
            "band, bor, bxor, bxor_not, band_not, add, sub, mul;",
            "bnot, negate, copy;",
            "shl, lshr, ashr;",
            "eq, ne, ucmp, scmp;",
            "is_nonzero, is_all_ones, popcnt_parity;",
            "apply_mask, fill_ones;",
        ] {
            assert!(
                WIDEOPS_C_DECLS.contains(name),
                "WIDEOPS_C_DECLS missing field group `{name}`"
            );
        }
    }

    fn ctx(width: usize, signed: bool) -> ExpressionContext {
        ExpressionContext { width, signed }
    }

    fn val_u64(payload: u64, width: usize) -> Value {
        Value::U64(ValueU64 {
            payload,
            mask_xz: 0,
            width: width as u32,
            signed: false,
        })
    }

    fn const_expr(payload: u64, width: usize) -> ProtoExpression {
        ProtoExpression::Value {
            value: val_u64(payload, width),
            width,
            expr_context: ctx(width, false),
        }
    }

    fn var_expr(var_offset: VarOffset, width: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset,
            select: None,
            dynamic_select: None,
            width,
            var_full_width: width,
            expr_context: ctx(width, false),
        }
    }

    #[test]
    fn comb_fallback_reason_names_uncovered_stmt() {
        // A $finish has no comb/cc emit (it affects sim state), so emit_stmt
        // rejects it and the comb network bails to Cranelift.  The
        // VERYL_AOT_C_DIAG reason helper should name the offending stmt kind
        // rather than the generic "nothing isolated" message.
        let stmts = vec![ProtoStatement::SystemFunctionCall(
            ProtoSystemFunctionCall::Finish,
        )];
        assert!(emit_function(&stmts).is_none()); // confirms the comb bails
        assert_eq!(comb_fallback_reason(&stmts), "SysFn");
    }

    #[test]
    fn emit_value_u64() {
        let v = val_u64(0x1234, 32);
        assert_eq!(emit_value(&v, 32).as_deref(), Some("0x1234ULL"));
    }

    #[test]
    fn emit_value_truncates_to_width() {
        // 4-bit value 0xff truncated to 0xf
        let v = val_u64(0xff, 4);
        assert_eq!(emit_value(&v, 4).as_deref(), Some("0xfULL"));
    }

    #[test]
    fn emit_value_rejects_wide() {
        // 65-128 bit values now emit via __uint128_t cast.
        // Reject only at >128 (no native 256-bit C type).
        let v = val_u64(0, 65);
        let s = emit_value(&v, 65).unwrap();
        assert!(s.contains("__uint128_t"));
        assert!(emit_value(&v, 129).is_none());
    }

    #[test]
    fn emit_value_width_zero_emits_zero() {
        // width=0 Values appear in heliodor (zero-width sentinels);
        // emit them as 0ULL to mirror the interpreter's numeric-zero
        // treatment.
        let v = val_u64(0, 0);
        assert_eq!(emit_value(&v, 0).as_deref(), Some("0x0ULL"));
    }

    #[test]
    fn emit_var_comb_u32() {
        assert_eq!(
            emit_var_load(&VarOffset::Comb(0x100), 16).as_deref(),
            Some("((uint64_t)*((const uint32_t*)(comb_values + 0x100)))"),
        );
    }

    #[test]
    fn emit_var_ff_u64() {
        assert_eq!(
            emit_var_load(&VarOffset::Ff(0x40), 64).as_deref(),
            Some("((uint64_t)*((const uint64_t*)(ff_values + 0x40)))"),
        );
    }

    #[test]
    fn emit_expr_binary_add() {
        let e = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Ff(0), 32)),
            op: Op::Add,
            y: Box::new(const_expr(1, 32)),
            width: 32,
            expr_context: ctx(32, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("ff_values + 0x0"));
        assert!(s.contains("0x1ULL"));
        assert!(s.contains(") + ("));
    }

    #[test]
    fn emit_expr_ternary() {
        let e = ProtoExpression::Ternary {
            cond: Box::new(var_expr(VarOffset::Comb(8), 1)),
            true_expr: Box::new(const_expr(0xa, 32)),
            false_expr: Box::new(const_expr(0xb, 32)),
            width: 32,
            expr_context: ctx(32, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains(" ? "));
        assert!(s.contains(" : "));
        assert!(s.contains("0xaULL"));
        assert!(s.contains("0xbULL"));
    }

    #[test]
    fn emit_expr_arith_shift_right_uses_signed_cast() {
        let e = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Ff(16), 32)),
            op: Op::ArithShiftR,
            y: Box::new(const_expr(2, 32)),
            width: 32,
            expr_context: ctx(32, true),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("(int64_t)"));
        assert!(s.contains(">>"));
    }

    #[test]
    fn emit_expr_bit_select() {
        let e = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x10),
            select: Some((7, 4)), // bits [7:4] = 4 bits
            dynamic_select: None,
            width: 4,
            var_full_width: 32,
            expr_context: ctx(4, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains(">> 4"));
        assert!(s.contains("0xf"));
    }

    #[test]
    fn emit_stmt_assign_comb() {
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x20),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xdeadbeef, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        // Comb store: direct offset, no shadow shift.
        assert!(s.contains("comb_values + 0x20"));
        assert!(s.contains("uint32_t"));
        assert!(s.contains("0xdeadbeefULL"));
    }

    #[test]
    fn emit_stmt_assign_ff_dual_slot_stores_and_logs() {
        // Dual-slot FF: current slot at 0x40, shadow (dst) at 0x48 (width 64,
        // nb=8).  An FF write — in the comb path too, since the is_ff
        // refinement can put one there — stores the shadow slot AND pushes a
        // WriteLogEntry at the current offset so ff_commit_from_log copies
        // shadow→current.  (A bare shadow store with no log entry, the old
        // behavior, silently dropped the write.)
        let a = ProtoAssignStatement {
            dst: VarOffset::Ff(0x48),
            dst_width: 64,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0x1234, 64),
            dst_ff_current_offset: 0x40,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        assert!(s.contains("ff_values + 0x48")); // shadow store
        assert!(s.contains("write_log")); // log push
        assert!(s.contains("0x40")); // log offset = current slot
    }

    #[test]
    fn emit_stmt_assign_comb_bit_select_single() {
        // 32-bit comb word, write 1-bit value at bit 5.
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x20),
            dst_width: 32,
            select: Some((5, 5)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(1, 1),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        // Read-modify-write at offset 0x20.
        assert!(s.contains("comb_values + 0x20"));
        // pos_mask = 1 << 5 = 0x20.
        assert!(s.contains("0x20"));
        // Value masked to 1 bit.
        assert!(s.contains("0x1ULL"));
        // Shifted into position by 5.
        assert!(s.contains("<< 5"));
    }

    #[test]
    fn emit_stmt_assign_comb_bit_select_slice() {
        // 32-bit comb word, write 4-bit value at bits [11:8].
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x10),
            dst_width: 32,
            select: Some((11, 8)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xa, 4),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        assert!(s.contains("comb_values + 0x10"));
        // value_mask = 0xf, pos_mask = 0xf << 8 = 0xf00.
        assert!(s.contains("0xfULL"));
        assert!(s.contains("0xf00"));
        assert!(s.contains("<< 8"));
    }

    #[test]
    fn emit_stmt_assign_ff_bit_select_rmw_logs() {
        // FF + bit-select is supported: read-modify-write the slot and push a
        // WriteLogEntry.  Packed FF here (dst == current offset) → log only,
        // no direct store.
        let a = ProtoAssignStatement {
            dst: VarOffset::Ff(0x40),
            dst_width: 32,
            select: Some((3, 0)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xf, 4),
            dst_ff_current_offset: 0x40,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        assert!(s.contains("write_log")); // log push of the RMW result
        assert!(s.contains("ff_values + 0x40")); // RMW read of the slot
    }

    #[test]
    fn emit_stmt_if_else() {
        use crate::ir::ProtoIfStatement;
        let inner_assign = ProtoAssignStatement {
            dst: VarOffset::Comb(0x10),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(1, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let if_stmt = ProtoIfStatement {
            cond: Some(var_expr(VarOffset::Comb(0), 1)),
            true_side: vec![ProtoStatement::Assign(inner_assign.clone())],
            false_side: vec![ProtoStatement::Assign(ProtoAssignStatement {
                expr: const_expr(2, 32),
                ..inner_assign
            })],
        };
        let s = emit_stmt(&ProtoStatement::If(if_stmt)).unwrap();
        assert!(s.starts_with("if ("));
        assert!(s.contains("} else {"));
        assert!(s.contains("0x1ULL"));
        assert!(s.contains("0x2ULL"));
    }

    #[test]
    fn emit_stmt_if_no_cond_runs_false_side() {
        // cond=None → interpreter runs false_side; emitter wraps it in
        // an unconditional block.
        use crate::ir::ProtoIfStatement;
        let f_assign = ProtoAssignStatement {
            dst: VarOffset::Comb(0x10),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xabc, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let if_stmt = ProtoIfStatement {
            cond: None,
            true_side: vec![],
            false_side: vec![ProtoStatement::Assign(f_assign)],
        };
        let s = emit_stmt(&ProtoStatement::If(if_stmt)).unwrap();
        assert!(s.starts_with("{ "));
        assert!(s.contains("0xabcULL"));
        assert!(!s.contains("if ("));
    }

    #[test]
    fn emit_stmt_sequential_block() {
        let assigns: Vec<ProtoStatement> = (0..3)
            .map(|i| {
                ProtoStatement::Assign(ProtoAssignStatement {
                    dst: VarOffset::Comb(0x10 + i * 4),
                    dst_width: 32,
                    select: None,
                    dynamic_select: None,
                    rhs_select: None,
                    expr: const_expr(i as u64, 32),
                    dst_ff_current_offset: 0,
                    token: dummy_token(),
                })
            })
            .collect();
        let s = emit_stmt(&ProtoStatement::SequentialBlock(assigns)).unwrap();
        assert!(s.starts_with("{ "));
        assert!(s.contains("comb_values + 0x10"));
        assert!(s.contains("comb_values + 0x14"));
        assert!(s.contains("comb_values + 0x18"));
    }

    #[test]
    fn emit_expr_concatenation_two_vars() {
        // {a:8, b:8} where a is at comb[0..1] and b at comb[8..9]
        let a = var_expr(VarOffset::Comb(0), 8);
        let b = var_expr(VarOffset::Comb(8), 8);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(a), 1, 8), (Box::new(b), 1, 8)],
            width: 16,
            expr_context: ctx(16, false),
        };
        let s = emit_expr(&e).unwrap();
        // Two shift+OR steps: each iter shifts acc by 8 and ORs in
        // the masked element.
        assert_eq!(s.matches("<< 8").count(), 2);
        assert_eq!(s.matches("0xffULL").count(), 2);
        assert!(s.contains("comb_values + 0x0"));
        assert!(s.contains("comb_values + 0x8"));
    }

    #[test]
    fn emit_expr_concatenation_replicate() {
        // {3{a:4}} → 12 bits total
        let a = var_expr(VarOffset::Comb(0), 4);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(a), 3, 4)],
            width: 12,
            expr_context: ctx(12, false),
        };
        let s = emit_expr(&e).unwrap();
        // repeat=3 yields three nested shift+OR pairs.
        assert_eq!(s.matches("<< 4").count(), 3);
        assert_eq!(s.matches("0xfULL").count(), 3);
    }

    #[test]
    fn emit_expr_concatenation_65_to_128_emits_u128() {
        // 32 + 33 = 65 bits — fits in __uint128_t accumulator.
        let a = var_expr(VarOffset::Comb(0), 32);
        let b = const_expr(0, 33);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(a), 1, 32), (Box::new(b), 1, 33)],
            width: 65,
            expr_context: ctx(65, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("__uint128_t"));
        assert!(s.contains("(__uint128_t)0)"));
    }

    #[test]
    fn emit_expr_concatenation_rejects_wider_than_128() {
        // 64 + 65 = 129 bits — exceeds __uint128_t capacity.
        let a = var_expr(VarOffset::Comb(0), 64);
        let b = const_expr(0, 65);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(a), 1, 64), (Box::new(b), 1, 65)],
            width: 129,
            expr_context: ctx(129, false),
        };
        assert!(emit_expr(&e).is_none());
    }

    #[test]
    fn emit_expr_variable_with_dynamic_select() {
        // 32-bit variable at comb[0x80] with dynamic_select picking
        // 4-bit slices indexed by another comb var.  Result width = 4.
        use crate::ir::ProtoDynamicBitSelect;
        let idx = var_expr(VarOffset::Comb(0), 8);
        let e = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x80),
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(idx),
                elem_width: 4,
                num_elements: 8,
            }),
            width: 4,
            var_full_width: 32,
            expr_context: ctx(4, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("_idx_raw"));
        assert!(s.contains("_idx_raw < 7 ?"));
        assert!(s.contains("comb_values + 0x80"));
        assert!(s.contains("_idx * 4"));
        assert!(s.contains("0xfULL"));
    }

    #[test]
    fn emit_expr_variable_dynamic_select_wide_var_rejects() {
        // var_full_width > 64: must reject (multi-word load not yet supported).
        use crate::ir::ProtoDynamicBitSelect;
        let idx = const_expr(0, 4);
        let e = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0),
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(idx),
                elem_width: 4,
                num_elements: 4,
            }),
            width: 4,
            var_full_width: 96,
            expr_context: ctx(4, false),
        };
        assert!(emit_expr(&e).is_none());
    }

    #[test]
    fn emit_expr_dynamic_variable_no_select() {
        // 4-element u32 array at comb[0x100], stride=4
        let idx = const_expr(2, 4);
        let e = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x100),
            stride: 4,
            element_native_bytes: 4,
            index_expr: Box::new(idx),
            num_elements: 4,
            select: None,
            dynamic_select: None,
            width: 32,
            expr_context: ctx(32, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("_idx_raw"));
        // Clamp to num_elements - 1 == 3.  Comparison is on _idx_raw.
        assert!(s.contains("_idx_raw < 3 ?"));
        assert!(s.contains("comb_values + 0x100"));
        assert!(s.contains("uint32_t"));
        // Stride and clamped idx feed the address computation.
        assert!(s.contains("(intptr_t)4 * (intptr_t)_idx"));
    }

    #[test]
    fn emit_expr_dynamic_variable_with_select() {
        // 8-element u8 array at ff[0x40], select [3:0]
        let idx = var_expr(VarOffset::Comb(0), 4);
        let e = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Ff(0x40),
            stride: 1,
            element_native_bytes: 1,
            index_expr: Box::new(idx),
            num_elements: 8,
            select: Some((3, 0)),
            dynamic_select: None,
            width: 4,
            expr_context: ctx(4, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("ff_values + 0x40"));
        assert!(s.contains(">> 0"));
        assert!(s.contains("0xfULL"));
    }

    #[test]
    fn emit_expr_dynamic_variable_zero_elements_rejects() {
        let idx = const_expr(0, 4);
        let e = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0),
            stride: 4,
            element_native_bytes: 4,
            index_expr: Box::new(idx),
            num_elements: 0,
            select: None,
            dynamic_select: None,
            width: 32,
            expr_context: ctx(32, false),
        };
        assert!(emit_expr(&e).is_none());
    }

    #[test]
    fn emit_stmt_assign_dynamic_comb() {
        use crate::ir::ProtoAssignDynamicStatement;
        let idx = const_expr(2, 4);
        let a = ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(0x100),
            dst_stride: 4,
            dst_num_elements: 4,
            dst_index_expr: idx,
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xdeadbeef, 32),
            dst_ff_current_base_offset: 0,
        };
        let s = emit_stmt(&ProtoStatement::AssignDynamic(a)).unwrap();
        assert!(s.contains("_idx_raw"));
        assert!(s.contains("_idx_raw < 3 ?"));
        assert!(s.contains("comb_values + 0x100"));
        assert!(s.contains("uint32_t"));
        assert!(s.contains("0xdeadbeefULL"));
    }

    #[test]
    fn emit_stmt_assign_dynamic_ff_rejects() {
        // A dynamic FF write in comb mode bails — it's an event-path-only emit.
        use crate::ir::ProtoAssignDynamicStatement;
        let a = ProtoAssignDynamicStatement {
            dst_base: VarOffset::Ff(0x40),
            dst_stride: 4,
            dst_num_elements: 4,
            dst_index_expr: const_expr(0, 4),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0, 32),
            dst_ff_current_base_offset: 0x40,
        };
        assert!(emit_stmt(&ProtoStatement::AssignDynamic(a)).is_none());
    }

    #[test]
    fn emit_stmt_compiled_block_inlines_original_stmts() {
        use crate::ir::CompiledBlockStatement;
        // CompiledBlock wraps two simple comb assigns at canonical
        // offsets; with deltas=0 the emitted code should address those
        // exact offsets.  The FuncPtr is intentionally bogus — we
        // bypass it entirely.
        let inner_a = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x10),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0x1111, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let inner_b = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x20),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0x2222, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let cb = CompiledBlockStatement {
            artifact: bogus_artifact(),
            ff_delta_bytes: 0,
            comb_delta_bytes: 0,
            input_offsets: vec![],
            output_offsets: vec![],
            ff_canonical_offsets: vec![],
            stmt_deps: vec![],
            original_stmts: vec![inner_a, inner_b],
        };
        let s = emit_stmt(&ProtoStatement::CompiledBlock(cb)).unwrap();
        assert!(s.starts_with("{ "));
        assert!(s.contains("comb_values + 0x10"));
        assert!(s.contains("comb_values + 0x20"));
        assert!(s.contains("0x1111ULL"));
        assert!(s.contains("0x2222ULL"));
    }

    #[test]
    fn emit_stmt_compiled_block_applies_comb_delta() {
        use crate::ir::CompiledBlockStatement;
        // comb_delta_bytes=0x100: emitted addresses must shift by 0x100.
        let inner = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x10),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xabc, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let cb = CompiledBlockStatement {
            artifact: bogus_artifact(),
            ff_delta_bytes: 0,
            comb_delta_bytes: 0x100,
            input_offsets: vec![],
            output_offsets: vec![],
            ff_canonical_offsets: vec![],
            stmt_deps: vec![],
            original_stmts: vec![inner],
        };
        let s = emit_stmt(&ProtoStatement::CompiledBlock(cb)).unwrap();
        assert!(s.contains("comb_values + 0x110"));
        assert!(!s.contains("comb_values + 0x10 ")); // base shouldn't appear unshifted
    }

    fn bogus_artifact() -> Arc<ChunkArtifact> {
        // Never actually called — emit_stmt for CompiledBlock bypasses
        // the artifact entirely.  We just need a valid handle for the
        // struct field.
        unsafe extern "system" fn stub(_: *const u8, _: *const u8, _: *mut u8) {}
        Arc::new(ChunkArtifact {
            func: stub,
            keepalive: None,
        })
    }

    #[test]
    fn emit_stmt_for_const_forward() {
        let body_assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x100),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xa, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let for_stmt = ProtoForStatement {
            var_offset: VarOffset::Comb(0),
            var_width: 32,
            var_native_bytes: 4,
            var_signed: false,
            range: ProtoForRange::Forward {
                start: ProtoForBound::Const(0),
                end: ProtoForBound::Const(8),
                inclusive: false,
                step: 1,
            },
            body: vec![body_assign],
        };
        let s = emit_stmt(&ProtoStatement::For(for_stmt)).unwrap();
        assert!(s.contains("for (uint64_t _it = 0ULL"));
        assert!(s.contains("_it < 8ULL"));
        assert!(s.contains("_it += 1ULL"));
        assert!(s.contains("comb_values + 0x0"));
        assert!(s.contains("0xaULL"));
    }

    #[test]
    fn emit_stmt_for_inclusive_bumps_end() {
        let for_stmt = ProtoForStatement {
            var_offset: VarOffset::Comb(0),
            var_width: 8,
            var_native_bytes: 1,
            var_signed: false,
            range: ProtoForRange::Forward {
                start: ProtoForBound::Const(0),
                end: ProtoForBound::Const(7),
                inclusive: true, // 0..=7 → 8 iters
                step: 1,
            },
            body: vec![],
        };
        let s = emit_stmt(&ProtoStatement::For(for_stmt)).unwrap();
        assert!(s.contains("_it < 8ULL"));
    }

    #[test]
    fn emit_stmt_for_dynamic_bound_rejects() {
        let for_stmt = ProtoForStatement {
            var_offset: VarOffset::Comb(0),
            var_width: 32,
            var_native_bytes: 4,
            var_signed: false,
            range: ProtoForRange::Forward {
                start: ProtoForBound::Const(0),
                end: ProtoForBound::Dynamic(const_expr(8, 32)),
                inclusive: false,
                step: 1,
            },
            body: vec![],
        };
        assert!(emit_stmt(&ProtoStatement::For(for_stmt)).is_none());
    }

    #[test]
    fn emit_stmt_break() {
        assert_eq!(emit_stmt(&ProtoStatement::Break).as_deref(), Some("break;"));
    }

    #[test]
    fn emit_function_simple_assign() {
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x10),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(7, 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let src = emit_function(&[ProtoStatement::Assign(a)]).unwrap();
        assert!(src.contains("#include <stdint.h>"));
        assert!(src.contains("veryl_aot_eval"));
        assert!(src.contains("comb_values + 0x10"));
    }

    /// Compile `src` end-to-end; return `None` when the built `.so`
    /// can't load on this host (e.g. cross-arch `cc` on Windows-on-ARM).
    /// Genuine compile failures still panic.
    fn compile_for_test(src: &str, what: &str) -> Option<EmittedModule> {
        match compile_source(src) {
            Ok(m) => Some(m),
            Err(e) if e.starts_with("dlopen") || e.starts_with("dlsym") => {
                eprintln!("{what}: shared object not loadable on this host ({e}); skipping");
                None
            }
            Err(e) => panic!("{what}: {e}"),
        }
    }

    #[test]
    fn emit_function_dynamic_variable_compiles() {
        // End-to-end: emit a DynamicVariable read into a function body
        // (write the loaded element to a fixed comb slot), compile,
        // dlopen, and observe the side effect.  Catches non-portable
        // GCC statement-expression syntax at compile time.
        if Command::new(std::env::var("VERYL_AOT_CC").unwrap_or_else(|_| "cc".to_string()))
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("emit_function_dynamic_variable_compiles: cc unavailable, skipping");
            return;
        }
        // Source array: 4 × u32 at comb[0..16].  Index = comb[16..20]
        // (a u32 too).  Result written to comb[20..24].
        let idx = var_expr(VarOffset::Comb(16), 32);
        let dyn_read = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0),
            stride: 4,
            element_native_bytes: 4,
            index_expr: Box::new(idx),
            num_elements: 4,
            select: None,
            dynamic_select: None,
            width: 32,
            expr_context: ctx(32, false),
        };
        let assign = ProtoAssignStatement {
            dst: VarOffset::Comb(20),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: dyn_read,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let src = emit_function(&[ProtoStatement::Assign(assign)]).unwrap();

        let tmp = std::env::temp_dir().join(format!("veryl_aot_dv_{}", std::process::id()));
        unsafe {
            std::env::set_var("VERYL_AOT_CACHE_DIR", &tmp);
        }
        let Some(module) = compile_for_test(&src, "emit_function_dynamic_variable_compiles") else {
            return;
        };
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 32];
        // Populate the array with distinct values; index = 2 selects
        // element 2 == 0xcccc.
        comb[0..4].copy_from_slice(&0xaaaau32.to_le_bytes());
        comb[4..8].copy_from_slice(&0xbbbbu32.to_le_bytes());
        comb[8..12].copy_from_slice(&0xccccu32.to_le_bytes());
        comb[12..16].copy_from_slice(&0xddddu32.to_le_bytes());
        comb[16..20].copy_from_slice(&2u32.to_le_bytes()); // idx = 2
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
            );
        }
        let written = u32::from_le_bytes(comb[20..24].try_into().unwrap());
        assert_eq!(
            written, 0xcccc,
            "DynamicVariable read should fetch element 2"
        );
        // Out-of-range idx should clamp to last element (0xdddd, index 3).
        comb[16..20].copy_from_slice(&99u32.to_le_bytes());
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
            );
        }
        let written = u32::from_le_bytes(comb[20..24].try_into().unwrap());
        assert_eq!(
            written, 0xdddd,
            "out-of-range idx should clamp to last element"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::remove_var("VERYL_AOT_CACHE_DIR");
        }
    }

    #[test]
    fn fnv1a_64_hex_stable() {
        // Stability check: cache keying must be deterministic across
        // runs.  Two distinct strings must produce distinct hashes (FNV
        // collisions on short inputs are vanishingly rare and would
        // surface here if our impl drifted).
        let a = fnv1a_64_hex_parts(&["hello"]);
        let b = fnv1a_64_hex_parts(&["hello"]);
        let c = fnv1a_64_hex_parts(&["world"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
        // Part boundaries are domain-separated: ["ab","c"] != ["a","bc"].
        assert_ne!(
            fnv1a_64_hex_parts(&["ab", "c"]),
            fnv1a_64_hex_parts(&["a", "bc"]),
        );
        // Same source under different compiler/flags must key differently.
        assert_ne!(
            fnv1a_64_hex_parts(&["v1", "gcc", "-O3", "SRC"]),
            fnv1a_64_hex_parts(&["v1", "clang", "-O3", "SRC"]),
        );
    }

    #[test]
    fn compile_source_round_trip() {
        // End-to-end: compile a hand-written stub C source, dlopen,
        // call through the FuncPtr ABI, observe a side effect on the
        // comb_values buffer.  Skipped when `cc` is unavailable.
        if Command::new(std::env::var("VERYL_AOT_CC").unwrap_or_else(|_| "cc".to_string()))
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("compile_source_round_trip: cc unavailable, skipping");
            return;
        }
        let src = "\
            #include <stdint.h>\n\
            __attribute__((visibility(\"default\")))\n\
            void veryl_aot_eval(uint8_t *ff, uint8_t *comb, uint64_t *log) {\n\
                (void)ff; (void)log;\n\
                *(uint32_t*)(comb + 0) = 0xdeadbeef;\n\
            }\n";
        // Use a per-test cache dir so we don't pollute the user's
        // shared cache and so the test is hermetic.
        let tmp = std::env::temp_dir().join(format!("veryl_aot_test_{}", std::process::id()));
        // SAFETY: tests run in a single-threaded test runner by
        // default; even with --test-threads, env races would only
        // perturb other AOT-C tests, not this one's correctness.
        unsafe {
            std::env::set_var("VERYL_AOT_CACHE_DIR", &tmp);
        }
        let Some(module) = compile_for_test(src, "compile_source_round_trip") else {
            return;
        };
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 16];
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
            );
        }
        let written = u32::from_le_bytes(comb[0..4].try_into().unwrap());
        assert_eq!(written, 0xdeadbeef, "comb[0..4] should be 0xdeadbeef");
        // Best-effort cleanup; ignore failures.
        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::remove_var("VERYL_AOT_CACHE_DIR");
        }
    }
}
