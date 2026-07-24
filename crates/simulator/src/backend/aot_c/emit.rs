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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
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
static inline uint64_t vw_sext_word(const uint8_t* p,unsigned i,uint32_t w,int sign);
static inline void vw_sext_copy(uint8_t* d,const uint8_t* s,uint32_t sw,uint32_t dnb){
  unsigned n=dnb/8; if(sw==0){ for(unsigned i=0;i<n;i++) VW_WR(d,i,0); return; }
  int sign=(int)((VW_RD(s,(sw-1)/64)>>((sw-1)%64))&1);
  for(unsigned i=0;i<n;i++) VW_WR(d,i, vw_sext_word(s,i,sw,sign)); }
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
static inline uint64_t vw_sext_word(const uint8_t* p,unsigned i,uint32_t w,int sign){
  unsigned bits_below=i*64; if(bits_below>=w) return sign? ~(uint64_t)0 : (uint64_t)0;
  uint64_t raw=VW_RD(p,i); unsigned top=w-bits_below; if(top>=64) return raw;
  uint64_t mask=((uint64_t)1<<top)-1; return (raw & mask) | (sign? ~mask : (uint64_t)0); }
static inline int64_t vw_scmp_asym(const uint8_t* a,const uint8_t* b,uint32_t ap,uint32_t bp){
  uint32_t anb=ap&0xFFFF, aw=ap>>16, bnb=bp&0xFFFF, bw=bp>>16;
  if(aw==0||bw==0||anb==0||bnb==0) return 0;
  int as=(int)((VW_RD(a,(aw-1)/64)>>((aw-1)%64))&1);
  int bs=(int)((VW_RD(b,(bw-1)/64)>>((bw-1)%64))&1);
  if(as!=bs){ return as==1? -1 : 1; }
  unsigned anw=anb/8, bnw=bnb/8, words=anw>bnw?anw:bnw;
  for(unsigned i=words;i-->0;){ uint64_t av=vw_sext_word(a,i,aw,as), bv=vw_sext_word(b,i,bw,bs);
    if(av<bv) return -1; if(av>bv) return 1; } return 0; }
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

// ---------------------------------------------------------------------------
// Chunk-local comb intermediate localization (VERYL_AOT_C_LOCALIZE, default on,
// `=0` to opt out).  A comb scalar written and read only within its emit chunk
// is kept in a C local instead of round-tripping `comb_values` (gcc can't drop
// the store — escaping restrict param — but the emitter's global read-set can).
// Soundness: localize only a signal (a) written by one top-level unconditional
// full-width scalar (≤64-bit) Assign, (b) read only in that chunk, (c) not
// blocklisted (event-read / array-range / partial-write / port).  Blocklist
// built in `module.rs`.
thread_local! {
    /// Comb offsets the caller marked unsafe to localize (read outside the
    /// comb function / dynamically / partial-written / port-visible).
    static LOCALIZE_BLOCKLIST: std::cell::RefCell<std::collections::HashSet<isize>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
    /// Comb offsets localized in the chunk currently being emitted.
    static CURRENT_LOCAL: std::cell::RefCell<std::collections::HashSet<isize>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
    /// Runtime-indexed comb array ranges (base, num_elements, stride) — a
    /// candidate offset inside any of these is excluded (a constant-indexed
    /// element could be read dynamically by an event / another statement).
    static LOCALIZE_RANGES: std::cell::RefCell<Vec<(isize, usize, isize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Byte ranges (offset, native_bytes) localized in the just-emitted comb —
    /// these comb_values bytes are intentionally left stale, so the validate
    /// dual-run must skip them.  Read by `prepare_comb` right after emit.
    static LAST_LOCALIZED_BYTES: std::cell::RefCell<Vec<(isize, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Set only between `set_localize_blocklist`/`clear_localize_blocklist`, i.e.
    /// when `module.rs` has installed a sound read-set.  `emit_function`
    /// localizes ONLY when armed, so a direct call (tests, diagnostics) never
    /// localizes unsoundly.
    static LOCALIZE_ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Take the (offset, native_bytes) ranges localized by the most recent
/// `emit_function` call.  `prepare_comb` hands these to the compiled handle so
/// the validate dual-run can skip the intentionally-stale comb bytes.
pub fn take_last_localized_bytes() -> Vec<(isize, usize)> {
    LAST_LOCALIZED_BYTES.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

#[inline]
fn localize_armed() -> bool {
    LOCALIZE_ARMED.with(|a| a.get())
}

/// Install the caller-computed blocklist + array ranges and arm localization
/// for the next comb emit.  The caller (`module.rs`) gates on
/// `VERYL_AOT_C_LOCALIZE` and only calls this when localization is on AND a
/// sound global read-set has been computed.  Always paired with
/// `clear_localize_blocklist`.
pub fn set_localize_blocklist(
    set: std::collections::HashSet<isize>,
    ranges: Vec<(isize, usize, isize)>,
) {
    LOCALIZE_BLOCKLIST.with(|b| *b.borrow_mut() = set);
    LOCALIZE_RANGES.with(|r| *r.borrow_mut() = ranges);
    LOCALIZE_ARMED.with(|a| a.set(true));
}

pub fn clear_localize_blocklist() {
    LOCALIZE_BLOCKLIST.with(|b| b.borrow_mut().clear());
    LOCALIZE_RANGES.with(|r| r.borrow_mut().clear());
    LOCALIZE_ARMED.with(|a| a.set(false));
}

fn clear_current_local() {
    CURRENT_LOCAL.with(|c| c.borrow_mut().clear());
}

#[inline]
fn is_localized(off: isize) -> bool {
    CURRENT_LOCAL.with(|c| c.borrow().contains(&off))
}

/// Comb local hex name for an offset (`_cl_<hex>`).
#[inline]
fn local_name(off: isize) -> String {
    format!("_cl_{off:x}")
}

#[derive(Default)]
struct LocalAnalysis {
    /// off -> Some(chunk) while every write so far is a clean top-level full
    /// scalar Assign in that one chunk; None once disqualified.
    write_chunk: HashMap<isize, Option<usize>>,
    /// Disqualified offsets (conditional / partial / wide / dynamic / array /
    /// CompiledBlock-touched write).
    bad: std::collections::HashSet<isize>,
    /// off -> Some(chunk) while read in one chunk only; None if read in 2+.
    read_chunk: HashMap<isize, Option<usize>>,
    /// off -> native storage byte width (for the validate skip-range).
    width: HashMap<isize, usize>,
}

impl LocalAnalysis {
    fn note_read(&mut self, off: isize, i: usize) {
        match self.read_chunk.get(&off) {
            None => {
                self.read_chunk.insert(off, Some(i));
            }
            Some(Some(k)) if *k != i => {
                self.read_chunk.insert(off, None);
            }
            _ => {}
        }
    }

    fn walk_reads(&mut self, e: &ProtoExpression, i: usize) {
        match e {
            ProtoExpression::HierVariable(_) => {
                unreachable!("hierarchical reference must be resolved by resolve_hier_refs first")
            }
            ProtoExpression::Variable {
                var_offset,
                dynamic_select,
                ..
            } => {
                if let VarOffset::Comb(o) = var_offset {
                    self.note_read(*o, i);
                }
                if let Some(ds) = dynamic_select {
                    self.walk_reads(&ds.index_expr, i);
                }
            }
            ProtoExpression::Value { .. } => {}
            ProtoExpression::Unary { x, .. } => self.walk_reads(x, i),
            ProtoExpression::Binary { x, y, .. } => {
                self.walk_reads(x, i);
                self.walk_reads(y, i);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (e, _, _) in elements {
                    self.walk_reads(e, i);
                }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                self.walk_reads(cond, i);
                self.walk_reads(true_expr, i);
                self.walk_reads(false_expr, i);
            }
            ProtoExpression::DynamicVariable {
                index_expr,
                dynamic_select,
                ..
            } => {
                // The array base/elements are covered by the blocklist (the
                // module-level pass records every runtime-indexed range); only
                // the index sub-expression carries localizable scalar reads.
                self.walk_reads(index_expr, i);
                if let Some(ds) = dynamic_select {
                    self.walk_reads(&ds.index_expr, i);
                }
            }
        }
    }

    fn disqualify(&mut self, off: isize) {
        self.bad.insert(off);
        self.write_chunk.insert(off, None);
    }

    /// Mark every comb offset mentioned (read or write) in a CompiledBlock as
    /// unsafe — its pre-compiled child reads/writes comb_values directly,
    /// bypassing any local we'd introduce.
    fn poison(&mut self, s: &ProtoStatement) {
        match s {
            ProtoStatement::Assign(a) => {
                self.poison_expr(&a.expr);
                if let VarOffset::Comb(o) = a.dst {
                    self.bad.insert(o);
                }
            }
            ProtoStatement::AssignDynamic(a) => {
                self.poison_expr(&a.dst_index_expr);
                self.poison_expr(&a.expr);
                if let VarOffset::Comb(o) = a.dst_base {
                    self.bad.insert(o);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(c) = &x.cond {
                    self.poison_expr(c);
                }
                for s in &x.true_side {
                    self.poison(s);
                }
                for s in &x.false_side {
                    self.poison(s);
                }
            }
            ProtoStatement::Case(x) => {
                for arm in &x.arms {
                    self.poison_expr(&arm.cond);
                    for s in &arm.body {
                        self.poison(s);
                    }
                }
                for s in &x.default {
                    self.poison(s);
                }
            }
            ProtoStatement::For(x) => {
                if let VarOffset::Comb(o) = x.var_offset {
                    self.bad.insert(o);
                }
                for s in &x.body {
                    self.poison(s);
                }
            }
            ProtoStatement::SequentialBlock(b) => {
                for s in b {
                    self.poison(s);
                }
            }
            ProtoStatement::CompiledBlock(x) => {
                for s in &x.original_stmts {
                    self.poison(s);
                }
            }
            ProtoStatement::SystemFunctionCall(_)
            | ProtoStatement::TbMethodCall { .. }
            | ProtoStatement::Break => {}
        }
    }

    fn poison_expr(&mut self, e: &ProtoExpression) {
        match e {
            ProtoExpression::HierVariable(_) => {
                unreachable!("hierarchical reference must be resolved by resolve_hier_refs first")
            }
            ProtoExpression::Variable { var_offset, .. } => {
                if let VarOffset::Comb(o) = var_offset {
                    self.bad.insert(*o);
                }
            }
            ProtoExpression::Value { .. } => {}
            ProtoExpression::Unary { x, .. } => self.poison_expr(x),
            ProtoExpression::Binary { x, y, .. } => {
                self.poison_expr(x);
                self.poison_expr(y);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (e, _, _) in elements {
                    self.poison_expr(e);
                }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                self.poison_expr(cond);
                self.poison_expr(true_expr);
                self.poison_expr(false_expr);
            }
            ProtoExpression::DynamicVariable {
                base_offset,
                index_expr,
                ..
            } => {
                if let VarOffset::Comb(o) = base_offset {
                    self.bad.insert(*o);
                }
                self.poison_expr(index_expr);
            }
        }
    }

    fn walk_stmt(&mut self, s: &ProtoStatement, i: usize, top: bool) {
        match s {
            ProtoStatement::Assign(a) => {
                self.walk_reads(&a.expr, i);
                if let Some(ds) = &a.dynamic_select {
                    self.walk_reads(&ds.index_expr, i);
                }
                if let VarOffset::Comb(o) = a.dst {
                    let clean = top
                        && a.select.is_none()
                        && a.dynamic_select.is_none()
                        && a.rhs_select.is_none()
                        && a.dst_width > 0
                        && a.dst_width <= 64;
                    if clean && !self.bad.contains(&o) {
                        self.width.insert(o, native_bytes(a.dst_width));
                        match self.write_chunk.get(&o) {
                            None => {
                                self.write_chunk.insert(o, Some(i));
                            }
                            Some(Some(k)) if *k == i => {}
                            Some(Some(_)) => {
                                self.disqualify(o);
                            }
                            Some(None) => {}
                        }
                    } else {
                        // Conditional (top == false), partial, wide, or dynamic
                        // write — never localize (latch / overlap hazard).
                        self.disqualify(o);
                    }
                }
            }
            ProtoStatement::AssignDynamic(a) => {
                self.walk_reads(&a.dst_index_expr, i);
                self.walk_reads(&a.expr, i);
                if let Some(ds) = &a.dynamic_select {
                    self.walk_reads(&ds.index_expr, i);
                }
                if let VarOffset::Comb(o) = a.dst_base {
                    self.disqualify(o);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(c) = &x.cond {
                    self.walk_reads(c, i);
                }
                for s in &x.true_side {
                    self.walk_stmt(s, i, false);
                }
                for s in &x.false_side {
                    self.walk_stmt(s, i, false);
                }
            }
            ProtoStatement::Case(x) => {
                // Arm bodies / default run conditionally, like an `If` branch,
                // so their writes are never top-level localizable.
                for arm in &x.arms {
                    self.walk_reads(&arm.cond, i);
                    for s in &arm.body {
                        self.walk_stmt(s, i, false);
                    }
                }
                for s in &x.default {
                    self.walk_stmt(s, i, false);
                }
            }
            ProtoStatement::For(x) => {
                if let VarOffset::Comb(o) = x.var_offset {
                    self.disqualify(o);
                }
                let (start, end) = match &x.range {
                    ProtoForRange::Forward { start, end, .. }
                    | ProtoForRange::Reverse { start, end, .. }
                    | ProtoForRange::Stepped { start, end, .. } => (start, end),
                };
                for b in [start, end] {
                    if let ProtoForBound::Dynamic(e) = b {
                        self.walk_reads(e, i);
                    }
                }
                for s in &x.body {
                    self.walk_stmt(s, i, false);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                // Unconditional grouping — preserve the incoming `top`.
                for s in body {
                    self.walk_stmt(s, i, top);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
                    for a in args {
                        self.walk_reads(a, i);
                    }
                }
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    self.walk_reads(condition, i);
                    for a in args {
                        self.walk_reads(a, i);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { .. } | ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(_) => {
                self.poison(s);
            }
            ProtoStatement::TbMethodCall { .. } | ProtoStatement::Break => {}
        }
    }
}

/// Per-chunk localization sets: comb offsets safe to keep in a C local within
/// each chunk (written by one clean top-level scalar Assign there, read only in
/// that chunk, not blocklisted).  Empty vec of empty sets when the knob is off.
fn compute_localize_sets(
    chunks: &[&[ProtoStatement]],
    blocklist: &std::collections::HashSet<isize>,
    ranges: &[(isize, usize, isize)],
) -> (Vec<std::collections::HashSet<isize>>, HashMap<isize, usize>) {
    let in_range = |off: isize| -> bool {
        ranges.iter().any(|&(base, num, stride)| {
            if stride == 0 || num == 0 {
                return false;
            }
            let delta = off - base;
            delta >= 0 && delta % stride == 0 && (delta / stride) < num as isize
        })
    };
    let mut a = LocalAnalysis::default();
    for (i, chunk) in chunks.iter().enumerate() {
        for s in *chunk {
            a.walk_stmt(s, i, true);
        }
    }
    let mut sets: Vec<std::collections::HashSet<isize>> =
        vec![std::collections::HashSet::new(); chunks.len()];
    for (off, wc) in &a.write_chunk {
        let Some(i) = wc else { continue };
        if a.bad.contains(off) || blocklist.contains(off) || in_range(*off) {
            continue;
        }
        // Reads must be confined to the write chunk (or absent — a dead local
        // assign that gcc removes).
        match a.read_chunk.get(off) {
            Some(Some(k)) if k == i => {}
            None => {}
            _ => continue,
        }
        sets[*i].insert(*off);
    }
    (sets, a.width)
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
        ProtoExpression::HierVariable(_) => None,
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
            // A dynamic-select on a wide var is interpreter-only here.
            if dynamic_select.is_some() {
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
            // Wide-result static bit-select: extract [lo..hi] of the (wide)
            // source into a scratch = (src >> lo) masked to nbits.  A ≤128-bit
            // result is a scalar (builds_wide_pointer routes it away from here);
            // this arm only fires for nbits > 128.  Mirrors the Cranelift
            // emit_wide_bit_select_read (feat/wide-result-bitselect, §26).
            if let Some((hi, lo)) = select {
                let nbits = hi.checked_sub(*lo)?.checked_add(1)?;
                if nbits <= 128 {
                    return None;
                }
                let src_nb = native_bytes(*var_full_width);
                let src_nw = src_nb / 8;
                let res_nb = native_bytes(nbits);
                let t = next_wide_tmp();
                pre.push_str(&format!(
                    "uint64_t _w{t}[{src_nw}]; \
                     vw_lshr((uint8_t*)_w{t}, (const uint8_t*)({buf} + {off:#x}), {lo}ull, {src_nb}u); \
                     vw_apply_mask((uint8_t*)_w{t}, (const uint8_t*)0, {mask}u); ",
                    mask = wpack(src_nb, nbits),
                ));
                return Some(WideRef {
                    addr: format!("((uint8_t*)_w{t})"),
                    nb: res_nb,
                    width: nbits,
                });
            }
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
        } => emit_wide_binary(x, *op, y, expr_context, pre),
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

/// `emit_wide_operand`, then sign-extend the value to the operation width
/// when the context is signed (mirrors Cranelift's wide_resize marshaling).
fn emit_wide_operand_signed(
    expr: &ProtoExpression,
    target_nb: usize,
    signed: bool,
    pre: &mut String,
) -> Option<WideRef> {
    let r = emit_wide_operand(expr, target_nb, pre)?;
    let w = expr.width();
    if signed && w > 0 && w < target_nb * 8 {
        let t = next_wide_tmp();
        let tnw = target_nb / 8;
        pre.push_str(&format!(
            "uint64_t _w{t}[{tnw}]; vw_sext_copy((uint8_t*)_w{t}, {src}, {w}u, {target_nb}u); ",
            src = r.addr,
        ));
        return Some(WideRef {
            addr: format!("((uint8_t*)_w{t})"),
            nb: target_nb,
            width: r.width,
        });
    }
    Some(r)
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

/// Emit a scalar read-modify-write for a `<=64`-bit wide bit-select store
/// `dst[hi:lo] <= src`, where `word_addr(k)` yields the C `veryl_u64_ua*`
/// address of the destination's 64-bit word `k`.  Such a field spans one or
/// two words, so this replaces the general path's full-width wide-op RMW
/// (fill_ones/shl/band/band_not/bor/copy/apply_mask) with one or two scalar
/// word RMWs.
fn emit_wide_narrow_field_store(
    expr: &ProtoExpression,
    hi: usize,
    lo: usize,
    dst_width: usize,
    word_addr: impl Fn(usize) -> String,
) -> Option<String> {
    // Bits >= dst_width must be dropped — the reference paths do so (interpret
    // masks to gen_mask(dst_width); Cranelift and the 8-op path apply_mask) but
    // the frontend doesn't reject an out-of-range LHS select (`// TODO
    // invalid_select`).  Clamping the field to [lo, dst_width) is a
    // compile-time fold that restores that parity without a runtime apply_mask,
    // and keeps the written word index in bounds (k1 < nw).
    if lo >= dst_width {
        return Some(String::from("{ }")); // whole field out of range → no-op
    }
    let hi = hi.min(dst_width - 1);
    let nbits = hi - lo + 1;
    debug_assert!(nbits <= 64);
    let mut pre = String::new();
    let sv = wide_shift_amount(expr, &mut pre)?; // source's low 64 bits
    let k0 = lo / 64;
    let k1 = hi / 64;
    let b = lo % 64;
    if k0 == k1 {
        let base_mask: u64 = if nbits == 64 {
            u64::MAX
        } else {
            (1u64 << nbits) - 1
        };
        let m = base_mask << b;
        let a0 = word_addr(k0);
        Some(format!(
            "{{ {pre}veryl_u64_ua* _d = {a0}; \
                *_d = ((*_d) & ~{m:#x}ULL) | ((((uint64_t)({sv})) << {b}) & {m:#x}ULL); }}"
        ))
    } else {
        // Two words (k1 == k0 + 1): the low (64-b) field bits go to word k0
        // [b:63], the rest to word k1 [0:hi%64].  b >= 1 (b == 0 would keep the
        // field in one word), so sh = 64 - b is never the UB `>> 64`.
        debug_assert!((1..=63).contains(&b));
        let m0: u64 = u64::MAX << b;
        let hb = hi % 64;
        let m1: u64 = if hb == 63 {
            u64::MAX
        } else {
            (1u64 << (hb + 1)) - 1
        };
        let sh = 64 - b;
        let a0 = word_addr(k0);
        let a1 = word_addr(k1);
        Some(format!(
            "{{ {pre}uint64_t _sv = (uint64_t)({sv}); \
                veryl_u64_ua* _d0 = {a0}; \
                veryl_u64_ua* _d1 = {a1}; \
                *_d0 = ((*_d0) & ~{m0:#x}ULL) | ((_sv << {b}) & {m0:#x}ULL); \
                *_d1 = ((*_d1) & ~{m1:#x}ULL) | ((_sv >> {sh}) & {m1:#x}ULL); }}"
        ))
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
    expr_context: &ExpressionContext,
    pre: &mut String,
) -> Option<WideRef> {
    let width = expr_context.width;
    let result_nb = native_bytes(width);
    let op_nb = native_bytes(width.max(x.width()).max(y.width()));
    let nw = op_nb / 8;
    let mask_pack = wpack(op_nb, width);
    match op {
        Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor | Op::Add | Op::Sub | Op::Mul => {
            let x_ref = emit_wide_operand_signed(x, op_nb, expr_context.signed, pre)?;
            let y_ref = emit_wide_operand_signed(y, op_nb, expr_context.signed, pre)?;
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
            // `>>>` in an unsigned context is a logical shift (mirrors Cranelift
            // and the interpreter); signed, x is sign-extended to the full op_nb
            // buffer so the fill comes from the buffer's top bit.
            let is_ashr = matches!(op, Op::ArithShiftR) && expr_context.signed;
            let x_ref = emit_wide_operand_signed(x, op_nb, is_ashr, pre)?;
            let amount = wide_shift_amount(y, pre)?;
            let fname = match op {
                Op::LogicShiftL | Op::ArithShiftL => "shl",
                Op::LogicShiftR => "lshr",
                Op::ArithShiftR if !expr_context.signed => "lshr",
                Op::ArithShiftR => "ashr",
                _ => unreachable!(),
            };
            // shl/lshr take plain nb; ashr packs the buffer width.
            let last = if is_ashr {
                format!("{}u", wpack(op_nb, op_nb * 8))
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

/// Wide concatenation, high-to-low `acc = (acc << elem_width) | elem` with a
/// final width mask.  Each element is placed directly at its precomputed offset:
/// O(total_width), not the O(N·total_width) of re-shifting the whole accumulator
/// once per element.
fn emit_wide_concat(
    elements: &[(Box<ProtoExpression>, usize, usize)],
    width: usize,
    pre: &mut String,
) -> Option<WideRef> {
    let nb = native_bytes(width);
    let nw = nb / 8;
    let acc = next_wide_tmp();
    pre.push_str(&format!("uint64_t _w{acc}[{nw}] = {{0}}; "));

    // High-to-low: the first element takes the highest bits, so offsets descend.
    let total: usize = elements.iter().map(|(_, r, ew)| r * ew).sum();
    let mut hi = total;

    for (elem, repeat, elem_width) in elements {
        let repeat = *repeat;
        let ew = *elem_width;
        if repeat == 0 || ew == 0 {
            continue;
        }
        // Zero adds no bits — skip, but its span still offsets later elements.
        let elem_is_zero = matches!(
            elem.as_ref(),
            ProtoExpression::Value { value, .. }
                if !value.is_xz() && value.payload().iter_u64_digits().next().is_none()
        );
        if elem_is_zero {
            hi -= ew * repeat;
            continue;
        }

        if ew <= 64 {
            // Mask to `ew`: the reference zero-extends the element from elem_width.
            let v = emit_expr(elem)?;
            let e = next_wide_tmp();
            pre.push_str(&format!(
                "uint64_t _e{e} = ((uint64_t)({v})) & 0x{m:x}ULL; ",
                m = width_mask(ew),
            ));
            for _ in 0..repeat {
                hi -= ew;
                let off = hi;
                let w = off / 64;
                if w >= nw {
                    continue; // past the result width
                }
                let b = off % 64;
                pre.push_str(&format!("_w{acc}[{w}] |= _e{e} << {b}; "));
                // `b+ew>64` ⇒ `b>0`, so `64-b ∈ 1..=63` (no shift UB).
                if b + ew > 64 && w + 1 < nw {
                    pre.push_str(&format!(
                        "_w{acc}[{w1}] |= _e{e} >> {sh}; ",
                        w1 = w + 1,
                        sh = 64 - b,
                    ));
                }
            }
        } else {
            // Wide element: shift into position once, not re-accumulated.
            let e_ref = emit_wide_operand(elem, nb, pre)?;
            for _ in 0..repeat {
                hi -= ew;
                let off = hi;
                let sh = next_wide_tmp();
                pre.push_str(&format!(
                    "uint64_t _w{sh}[{nw}]; vw_shl((uint8_t*)_w{sh}, {e}, {off}ull, {nb}u); \
                     vw_bor((uint8_t*)_w{acc}, (const uint8_t*)_w{acc}, (const uint8_t*)_w{sh}, {nb}u); ",
                    e = e_ref.addr,
                ));
            }
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
                // Sign-extend each operand from its OWN width: the result width
                // is 1 (useless for sign location) and a single common width
                // mislocates a narrower operand's sign.
                format!(
                    "vw_scmp_asym({a}, {b}, {ap}u, {bp}u)",
                    ap = wpack(op_nb, x.width()),
                    bp = wpack(op_nb, y.width())
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

/// As `emit_wide_select_read_at`, but for a 65..128-bit window → a
/// `__uint128_t`.  Funnel-shifts the little-endian u64 words at `base + lo/64`:
/// at most two words when `lo` is word-aligned, otherwise up to three (a third
/// word is read only when the window genuinely straddles into it, so the deref
/// stays in bounds).  `nbits` must be in 65..=128.
fn emit_wide_select_read_wide_at(base_ptr: &str, lo: usize, nbits: usize) -> String {
    let word = lo / 64;
    let bit = lo % 64;
    let base = format!("((const veryl_u64_ua*)({base_ptr}))");
    let mut e = if bit == 0 {
        format!(
            "(((__uint128_t)({base}[{w0}])) | (((__uint128_t)({base}[{w1}])) << 64))",
            w0 = word,
            w1 = word + 1,
        )
    } else {
        let mut s = format!(
            "((((__uint128_t)({base}[{w0}])) >> {bit}) \
              | (((__uint128_t)({base}[{w1}])) << {sh1}))",
            w0 = word,
            w1 = word + 1,
            sh1 = 64 - bit,
        );
        // The window reaches a third word only when bit + nbits > 128.
        if bit + nbits > 128 {
            s = format!(
                "({s} | (((__uint128_t)({base}[{w2}])) << {sh2}))",
                w2 = word + 2,
                sh2 = 128 - bit,
            );
        }
        s
    };
    if nbits < 128 {
        e = mask_u128(&e, nbits);
    }
    e
}

/// Mask a `__uint128_t` C expression to `width` (1..127) bits with a
/// hi/lo-split constant (matching the wide-store path's masking).
fn mask_u128(s: &str, width: usize) -> String {
    let m: u128 = (1u128 << width) - 1;
    let hi = (m >> 64) as u64;
    let lo = m as u64;
    format!("(({s}) & (((__uint128_t)0x{hi:x}ULL << 64) | (__uint128_t)0x{lo:x}ULL))")
}

/// Emit one or more `WriteLogWideEntry` pushes covering `nb` payload bytes
/// from `src_ptr` (a `uint8_t*` C expression) at FF byte offset `base_off`
/// (a C expression).  Each entry holds ≤56 bytes; larger values chunk.
/// Unchecked: the event prologue's bulk reserve guarantees room.
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
        EVENT_WIDE_PUSHES.with(|c| c.set(c.get() + 1));
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
    // Worst-case narrow/wide pushes per `veryl_aot_eval` invocation,
    // accumulated during emission (const-loop bodies scaled by trip
    // count).  The event prologue reserves this much up front, so the
    // per-push code needs no capacity check.
    static EVENT_NARROW_PUSHES: Cell<u64> = const { Cell::new(0) };
    static EVENT_WIDE_PUSHES: Cell<u64> = const { Cell::new(0) };
}
fn event_mode() -> bool {
    EVENT_MODE.with(|c| c.get())
}
fn set_event_mode(on: bool) {
    EVENT_MODE.with(|c| c.set(on));
}

/// Inline narrow WriteLogEntry push.  `offset_expr` / `payload_expr`
/// are C expressions; `wc` is native bytes ∈ {1,2,4,8}.  Unchecked: the
/// event prologue's bulk reserve guarantees room.
fn emit_log_push(offset_expr: &str, payload_expr: &str, wc: usize) -> String {
    // Offsets shared with the Cranelift push via write_log.rs consts,
    // so a layout change can't silently desync this emitted C.
    use crate::ir::write_log::{
        WRITE_LOG_ENTRY_OFFSET_MASK_XZ, WRITE_LOG_ENTRY_OFFSET_OFFSET,
        WRITE_LOG_ENTRY_OFFSET_PAYLOAD, WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS, WRITE_LOG_ENTRY_SIZE,
        WRITE_LOG_NARROW_OFFSET_COUNT, WRITE_LOG_NARROW_OFFSET_ENTRIES_PTR,
    };
    EVENT_NARROW_PUSHES.with(|c| c.set(c.get() + 1));
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

/// `VERYL_AOT_C_BOOLFOLD`: branchless LogicAnd/LogicOr (see `emit_expr_inner`).
/// `1` (default) folds only cheap force-eval arms (`is_cheap_boolfold_arm`);
/// `0` = off; `2` = every site (benchmark only).  Cached.
fn boolfold_mode() -> u8 {
    static E: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *E.get_or_init(|| match std::env::var("VERYL_AOT_C_BOOLFOLD").as_deref() {
        Ok("0") => 0,
        Ok("2") => 2,
        _ => 1,
    })
}

/// Cheap force-eval arm for boolfold: a shallow ≤64-bit tree of scalar reads /
/// constants / comparisons / bitwise+logical ops.  Excludes arithmetic, shifts,
/// wide ops, array reads, ternaries, concat — keeps the force-eval UB-free.
fn is_cheap_boolfold_arm(e: &ProtoExpression, depth: u32) -> bool {
    if depth == 0 {
        return false;
    }
    match e {
        ProtoExpression::Value { .. } => true,
        ProtoExpression::Variable {
            var_full_width,
            dynamic_select,
            ..
        } => dynamic_select.is_none() && *var_full_width <= 64,
        ProtoExpression::Unary {
            x, expr_context, ..
        } => expr_context.width <= 64 && is_cheap_boolfold_arm(x, depth - 1),
        ProtoExpression::Binary {
            op,
            x,
            y,
            expr_context,
            ..
        } => {
            expr_context.width <= 64
                && matches!(
                    op,
                    Op::Eq
                        | Op::Ne
                        | Op::EqWildcard
                        | Op::NeWildcard
                        | Op::Less
                        | Op::Greater
                        | Op::LessEq
                        | Op::GreaterEq
                        | Op::LogicAnd
                        | Op::LogicOr
                        | Op::BitAnd
                        | Op::BitOr
                        | Op::BitXor
                )
                && is_cheap_boolfold_arm(x, depth - 1)
                && is_cheap_boolfold_arm(y, depth - 1)
        }
        // DynamicVariable (array read), Ternary, Concatenation: not cheap.
        _ => false,
    }
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
                collect_uncovered(s, out);
            }
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &x.cond
                && emit_expr(c).is_none()
            {
                out.push(format!("If-cond-expr {}", classify_uncovered_expr(c)));
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

/// Is `e` emittable on the AOT-C path? The narrow `emit_expr` check alone
/// spuriously fails every wide node, so the census breadcrumb uses this to
/// name the real uncovered leaf.
fn expr_covered(e: &ProtoExpression) -> bool {
    if e.builds_wide_pointer() {
        emit_wide_expr(e, &mut String::new()).is_some()
    } else {
        emit_expr(e).is_some()
    }
}

/// Classify the first uncovered sub-EXPRESSION of `e` (the leaf where the
/// emit first returns None), for the `VERYL_AOT_C_DIAG` census — so the
/// `exprOK=false` comb bails name the exact wide construct still missing.
fn classify_uncovered_expr(e: &ProtoExpression) -> String {
    if expr_covered(e) {
        return "(covered)".to_string();
    }
    match e {
        ProtoExpression::HierVariable(_) => "hier_variable".to_string(),
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
            if !expr_covered(x) {
                format!("Un({op:?})/{}", classify_uncovered_expr(x))
            } else {
                format!("Un({op:?},w={width},xw={})", x.width())
            }
        }
        ProtoExpression::Binary {
            op, x, y, width, ..
        } => {
            if !expr_covered(x) {
                format!("Bin({op:?})/x:{}", classify_uncovered_expr(x))
            } else if !expr_covered(y) {
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
            if !expr_covered(cond) {
                format!("Tern/c:{}", classify_uncovered_expr(cond))
            } else if !expr_covered(true_expr) {
                format!("Tern/t:{}", classify_uncovered_expr(true_expr))
            } else if !expr_covered(false_expr) {
                format!("Tern/f:{}", classify_uncovered_expr(false_expr))
            } else {
                format!("Tern(w={width})")
            }
        }
        ProtoExpression::Concatenation {
            width, elements, ..
        } => {
            for (el, _, _) in elements {
                if !expr_covered(el) {
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
            index_expr,
            num_elements,
            ..
        } => {
            let ds = match dynamic_select {
                Some(d) => format!(
                    ",ds_ew={},ds_win={},ds_ne={},ds_idx:{}",
                    d.elem_width,
                    d.window,
                    d.num_elements,
                    classify_uncovered_expr(&d.index_expr)
                ),
                None => String::new(),
            };
            format!(
                "DynVar(w={width},enb={element_native_bytes},ne={num_elements},sel={},idx:{}{ds})",
                select.is_some(),
                classify_uncovered_expr(index_expr),
            )
        }
    }
}

/// Descend into a rejected statement to name the first failing leaf.
/// Re-runs emit; event_mode must already be set by the caller.
fn diag_find_fail(stmt: &ProtoStatement) -> String {
    match stmt {
        ProtoStatement::CompiledBlock(cb) => {
            for s in &cb.original_stmts {
                if emit_stmt(s).is_none() {
                    return format!("CB/{}", diag_find_fail(s));
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
        let win = dyn_sel.window;
        if ew == 0 || ew >= 64 || win == 0 || win >= 64 || ne == 0 || ne.checked_mul(ew)? > 64 {
            ev_diag(&format!(
                "static FF: dynamic_select ew={ew} win={win} ne={ne} unsupported"
            ));
            return None;
        }
        let vmask = (1u64 << win) - 1;
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
/// 2-state, narrow; a static bit-select does an element RMW (mirroring
/// AssignDynamicStatement::eval_step: merge into the NEXT slot — same-event
/// prior writes must be visible — and push the merged element).  A dynamic
/// bit-select still bails.
fn emit_event_ff_assign_dynamic(a: &ProtoAssignDynamicStatement) -> Option<String> {
    if a.dynamic_select.is_some() {
        ev_diag("dyn FF: dynsel");
        return None;
    }
    if a.dst_width == 0 {
        ev_diag("dyn FF: width=0");
        return None;
    }
    if a.dst_width > 64 {
        return emit_event_ff_assign_dynamic_wide(a);
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
    let payload = if let Some((hi, lo)) = a.select {
        let nbits = hi.checked_sub(lo)?.checked_add(1)?;
        if hi >= a.dst_width || nbits > 64 {
            ev_diag(&format!("dyn FF: select={:?} w={}", a.select, a.dst_width));
            return None;
        }
        let vmask = width_mask(nbits);
        let pmask = !(vmask << lo) & dwmask;
        format!(
            "(((uint64_t)*((const {ct}*)(ff_values + {wbase:#x} + (intptr_t){stride} * (intptr_t)_idx)) & 0x{pm:x}ULL) \
              | ((((uint64_t)({rhs})) & 0x{vm:x}ULL) << {lo})) & 0x{dw:x}ULL",
            ct = cty,
            wbase = dst_base_raw,
            stride = a.dst_stride,
            pm = pmask,
            rhs = rhs,
            vm = vmask,
            lo = lo,
            dw = dwmask,
        )
    } else {
        format!(
            "(((uint64_t)({rhs})) & 0x{dw:x}ULL)",
            rhs = rhs,
            dw = dwmask
        )
    };
    let push = emit_log_push("_woff", "_wval", nb);
    // Packed: skip the in-place store; the log push delivers it read-OLD (NBA).
    // See AssignDynamicStatement::ff_is_packed. Unpacked keeps it.
    let ff_is_packed = dst_base_raw == cur_base;
    let store = if ff_is_packed {
        String::new()
    } else {
        format!(
            "*(({ct}*)(ff_values + {wbase:#x} + (intptr_t){stride} * (intptr_t)_idx)) = ({ct})_wval; ",
            ct = cty,
            wbase = dst_base_raw,
            stride = a.dst_stride,
        )
    };
    Some(format!(
        "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
            uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
            uint64_t _wval = {pay}; \
            {store}\
            unsigned int _woff = (unsigned int)((intptr_t){cbase:#x} + (intptr_t){stride} * (intptr_t)_idx); \
            {push} }});",
        idx = idx,
        max = max_idx,
        pay = payload,
        store = store,
        cbase = cur_base,
        stride = a.dst_stride,
        push = push,
    ))
}

/// Wide (>64-bit) analogue of `emit_event_ff_assign_dynamic`, routing through
/// the wide write-log pool.  Full-element 2-state only; select / dynamic-select
/// / rhs_select bail (rare; the dcache line-write path has none).
fn emit_event_ff_assign_dynamic_wide(a: &ProtoAssignDynamicStatement) -> Option<String> {
    if a.select.is_some() || a.rhs_select.is_some() {
        ev_diag(&format!(
            "dyn FF wide: select={:?} rhssel={:?} width={}",
            a.select, a.rhs_select, a.dst_width
        ));
        return None;
    }
    if a.dst_num_elements == 0 {
        return None;
    }
    let dst_base_raw = match a.dst_base {
        VarOffset::Ff(o) => o,
        VarOffset::Comb(_) => return None,
    };
    let cur_base = a.dst_ff_current_base_offset;
    if cur_base < 0 || dst_base_raw < 0 {
        return None;
    }
    let nb = native_bytes(a.dst_width);
    let nw = nb / 8;
    let max_idx = a.dst_num_elements.saturating_sub(1);
    let idx = emit_expr(&a.dst_index_expr)?;
    let mut pre = String::new();
    // Mask into a fresh scratch — the source may alias a flat read, and the FF
    // slot must not be clobbered before commit.
    let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
    let d = next_wide_tmp();
    pre.push_str(&format!(
        "uint64_t _w{d}[{nw}]; vw_copy((uint8_t*)_w{d}, {src}, {nb}u); \
         vw_apply_mask((uint8_t*)_w{d}, (const uint8_t*)0, {p}u); ",
        src = r.addr,
        p = wpack(nb, a.dst_width),
    ));
    // Packed: skip the in-place store; the wide log push below delivers it
    // read-OLD (NBA). Not "idempotent with the log" — it landed mid-event, so a
    // same-event reader saw read-NEW. Unpacked keeps it for multi-RMW forwarding.
    let ff_is_packed = dst_base_raw == cur_base;
    let store = if ff_is_packed {
        String::new()
    } else {
        format!(
            "vw_copy((uint8_t*)(ff_values + {wbase:#x} + (intptr_t){stride} * (intptr_t)_idx), \
                     (const uint8_t*)_w{d}, {nb}u); ",
            wbase = dst_base_raw,
            stride = a.dst_stride,
        )
    };
    let push = emit_wide_log_chunks(&format!("(uint8_t*)_w{d}"), "_woff", nb);
    Some(format!(
        "{{ uint64_t _idx_raw = (uint64_t)({idx}); \
            uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
            {pre}{store}\
            unsigned int _woff = (unsigned int)((intptr_t){cbase:#x} + (intptr_t){stride} * (intptr_t)_idx); \
            {push} }}",
        idx = idx,
        max = max_idx,
        pre = pre,
        store = store,
        cbase = cur_base,
        stride = a.dst_stride,
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

/// A background compile request: build `src` and publish the result through
/// `cell` when the `.so` is ready.
struct CompileJob {
    src: String,
    cell: AotCell,
}

/// Concurrent external `cc` cap — the `-jN` knob for the compile pool (see
/// [`compile_pool`]).  Default `max(2, available_parallelism / 4)`, override
/// with `VERYL_AOT_C_COMPILE_JOBS`.  Only a quarter of the cores because
/// `veryl test` already runs the testbenches on `available_parallelism` sim
/// threads; sizing this background pool at the core count makes the (mostly
/// wasted) compiles contend with that useful work and slows the suite.  The
/// floor of 2 lets a boot compile its comb and clock-event in parallel.
fn compile_jobs() -> usize {
    std::env::var("VERYL_AOT_C_COMPILE_JOBS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() / 4).max(2))
                .unwrap_or(2)
        })
}

/// Lazily-started global pool of `compile_jobs()` workers draining a shared
/// queue; returns the job sender.
///
/// In async mode each whole-module compile used to get its own detached
/// `std::thread::spawn` → `cc`.  The simulator never blocks on them (it stays
/// on Cranelift until the `.so` lands), so the ~220-test fast suite spawned
/// `cc` faster than they finished — hundreds at once, load average over 100.
/// The pool caps in-flight `cc` like `make -jN`.
fn compile_pool() -> &'static Sender<CompileJob> {
    static POOL: OnceLock<Sender<CompileJob>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<CompileJob>();
        // Shared receiver behind a Mutex: a worker holds the lock only to
        // dequeue, then releases it before compiling.  recv blocks under the
        // lock only when the queue is empty, so this never serializes compiles.
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..compile_jobs() {
            let rx = Arc::clone(&rx);
            let _ = std::thread::Builder::new()
                .name("veryl-aot-cc".into())
                .spawn(move || {
                    loop {
                        let job = {
                            let guard = match rx.lock() {
                                Ok(g) => g,
                                Err(_) => break, // poisoned: drop this worker
                            };
                            guard.recv()
                        };
                        // Err only if every sender dropped; the sender is
                        // 'static, so this never fires — but exit cleanly.
                        let Ok(job) = job else { break };
                        // Isolate a compile panic so it can't permanently shrink
                        // the pool (compile_source returns Err for all expected
                        // failures, so this only ever fires on a bug).
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if let Ok(m) = compile_source(&job.src) {
                                let _ = job.cell.set(m);
                            }
                        }));
                    }
                });
        }
        tx
    })
}

/// Compile `src` to an `EmittedModule` published through a `OnceLock`.  When
/// `async_mode` is true the compile is queued on the bounded global pool
/// (see [`compile_pool`]) and the cell stays empty until the `.so` is ready →
/// callers stay on Cranelift, then hot-swap to AOT-C the cycle it lands —
/// hiding the cold gcc latency; otherwise it is filled synchronously before
/// return.  A compile failure (e.g. missing `cc`) leaves the cell empty →
/// graceful Cranelift fallback either way.
fn compile_or_spawn(src: String, async_mode: bool) -> AotCell {
    let cell = Arc::new(OnceLock::new());
    if async_mode {
        let job = CompileJob {
            src,
            cell: Arc::clone(&cell),
        };
        // A failed send just leaves the cell empty → Cranelift handles it.
        let _ = compile_pool().send(job);
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
    // Localization never applies to the event path; clear any residue a failed
    // comb emit may have left so event reads never hit `_cl_*`.
    clear_current_local();
    EVENT_NARROW_PUSHES.with(|c| c.set(0));
    EVENT_WIDE_PUSHES.with(|c| c.set(0));
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
    // > u32::MAX pushes per eval can't be reserved in one call; bail to
    // Cranelift (which checks per push) rather than under-reserving.
    let narrow_pushes = u32::try_from(EVENT_NARROW_PUSHES.with(|c| c.get())).ok()?;
    let wide_pushes = u32::try_from(EVENT_WIDE_PUSHES.with(|c| c.get())).ok()?;
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
         void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log, intptr_t ff_delta) {\n",
    );
    src.push_str(&emit_reserve_prologue(narrow_pushes, wide_pushes));
    src.push_str(&body);
    src.push_str("}\n");
    Some(src)
}

/// Prologue for `veryl_aot_eval`: one bulk reserve covering the body's
/// worst-case push count, so every inline push below stays unchecked.
/// Calls the `reserve` fn pointer stored in the buffer header (a baked
/// symbol address would break the on-disk `.so` cache across ASLR).
fn emit_reserve_prologue(narrow: u32, wide: u32) -> String {
    use crate::ir::write_log::{
        WRITE_LOG_NARROW_OFFSET_CAPACITY, WRITE_LOG_NARROW_OFFSET_COUNT, WRITE_LOG_OFFSET_RESERVE,
        WRITE_LOG_WIDE_OFFSET_CAPACITY, WRITE_LOG_WIDE_OFFSET_COUNT,
    };
    if narrow == 0 && wide == 0 {
        return String::new();
    }
    // capacity - count is the free room; capacity >= count always holds.
    let mut conds: Vec<String> = Vec::new();
    if narrow > 0 {
        conds.push(format!(
            "*(unsigned int*)(_lb + {cap}) - *(unsigned int*)(_lb + {cnt}) < {narrow}u",
            cap = WRITE_LOG_NARROW_OFFSET_CAPACITY,
            cnt = WRITE_LOG_NARROW_OFFSET_COUNT,
        ));
    }
    if wide > 0 {
        conds.push(format!(
            "*(unsigned int*)(_lb + {cap}) - *(unsigned int*)(_lb + {cnt}) < {wide}u",
            cap = WRITE_LOG_WIDE_OFFSET_CAPACITY,
            cnt = WRITE_LOG_WIDE_OFFSET_COUNT,
        ));
    }
    format!(
        "    {{ unsigned char* _lb = (unsigned char*)write_log; \
            if (__builtin_expect({cond}, 0)) \
                ((void(*)(void*, unsigned int, unsigned int))*(void**)(_lb + {res}))\
                (_lb, {narrow}u, {wide}u); }}\n",
        cond = conds.join(" || "),
        res = WRITE_LOG_OFFSET_RESERVE,
    )
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

#[derive(Clone, Copy)]
enum ExpectHint {
    False,
    True,
    Off,
}

/// Split from `wrap_expect` (no env) so the emitted form is unit-testable.
fn wrap_expect_hint(c: &str, hint: ExpectHint) -> String {
    match hint {
        ExpectHint::False => format!("__builtin_expect(({c}) != 0, 0)"),
        ExpectHint::True => format!("__builtin_expect(({c}) != 0, 1)"),
        ExpectHint::Off => c.to_string(),
    }
}

/// Hint a narrow mux/if condition unlikely (`VERYL_AOT_C_PREDICT_FALSE`,
/// default-on).  RTL mux/guard conditions are overwhelmingly false-biased —
/// only one arm of a wide select wins, guards rarely fire — so predicting them
/// false straightens the hot fall-through without a profile.  Layout-only, so
/// results are unchanged.
fn wrap_expect(c: &str) -> String {
    static H: OnceLock<ExpectHint> = OnceLock::new();
    let hint = *H.get_or_init(
        || match std::env::var("VERYL_AOT_C_PREDICT_FALSE").as_deref() {
            Ok("0") => ExpectHint::Off,
            Ok("invert") => ExpectHint::True,
            _ => ExpectHint::False,
        },
    );
    wrap_expect_hint(c, hint)
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
    compile_source_in(&cache_dir, src)
}

/// `compile_source` with an explicit cache directory instead of resolving
/// it from `VERYL_AOT_CACHE_DIR`/`XDG_CACHE_HOME`/`HOME`.  Tests pass a
/// per-test dir here directly: the cache dir is a *process-global* env var,
/// so mutating it from one test perturbs every other test compiling
/// concurrently (libtest runs tests multi-threaded by default).  Passing it
/// as an argument keeps each test hermetic without touching shared state.
fn compile_source_in(cache_dir: &Path, src: &str) -> Result<EmittedModule, String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("create_dir_all: {e}"))?;

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
    // Event sources compile without SLP vectorization: on the single big
    // event function SLP's dependence checking (alias stmt walking) blows
    // up superlinearly, and its vectorized stores have not shown run-time
    // wins on event .so.  The header comment is emitted by this file, so
    // the match can't drift.  VERYL_AOT_C_EVENT_NOSLP=0 opts back in.
    let event_noslp = std::env::var("VERYL_AOT_C_EVENT_NOSLP").map_or(true, |v| v != "0");
    if event_noslp && src.starts_with("// AOT-C event") {
        flags.push("-fno-tree-slp-vectorize".to_string());
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
        // Identical sources hash to the same `so_path`, so a `cc -o so_path`
        // from one thread can be dlopened half-written by another. Compile to a
        // unique temp, then `rename` (atomic within the dir) to publish.
        use std::sync::atomic::{AtomicU64, Ordering};
        static TMP_CTR: AtomicU64 = AtomicU64::new(0);
        let uniq = format!(
            "{}.{}",
            std::process::id(),
            TMP_CTR.fetch_add(1, Ordering::Relaxed)
        );
        let c_path = cache_dir.join(format!("veryl_aot_{hash}.c"));
        let tmp_c = cache_dir.join(format!("veryl_aot_{hash}.{uniq}.c"));
        let tmp_so = cache_dir.join(format!("veryl_aot_{hash}.{uniq}.so"));
        std::fs::write(&tmp_c, src).map_err(|e| format!("write {}: {}", tmp_c.display(), e))?;

        let mut cmd = Command::new(&cc_name);
        cmd.args(&flags).arg("-o").arg(&tmp_so).arg(&tmp_c);

        let out = cmd
            .output()
            .map_err(|e| format!("spawn cc: {e} (set VERYL_AOT_CC to override)"))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&tmp_so);
            // Leave the temp .c for inspection.
            return Err(format!(
                "cc {} failed: {}\n{}",
                tmp_c.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr),
            ));
        }
        // A racing peer publishes an equally valid file (same source), so an
        // overwrite either way is fine.
        let _ = std::fs::rename(&tmp_c, &c_path);
        std::fs::rename(&tmp_so, &so_path)
            .map_err(|e| format!("rename {}: {}", tmp_so.display(), e))?;
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
/// *comb, uint64_t *log, intptr_t ff_delta)`.  Comb-target writes store
/// directly; FF-target writes push WriteLogEntries like the event path.
pub fn emit_function(stmts: &[ProtoStatement]) -> Option<String> {
    reset_wide_tmp();
    // Splitting the monolithic body into ~chunk_size-stmt static functions
    // gives gcc -O3 smaller register-allocation and stack-frame scopes per
    // chunk and bounds spill locality (the unsplit body regresses L1d
    // locality).  chunk_size=0 disables splitting (single-function emit).
    // Override via VERYL_AOT_C_CHUNK_SIZE.
    //
    // 128: smaller chunks shrink each function's live set and spill
    // traffic; below ~50 the call/boundary overhead starts to erode the gain.
    let chunk_size: usize = std::env::var("VERYL_AOT_C_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);

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
    // Chunk-local intermediate localization (VERYL_AOT_C_LOCALIZE): per chunk,
    // the comb offsets that are written by one clean top-level scalar Assign in
    // that chunk and read only there (and not blocklisted) become C locals
    // instead of comb_values round-trips.  Empty sets when the knob is off.
    LAST_LOCALIZED_BYTES.with(|b| b.borrow_mut().clear());
    let localize_sets: Vec<std::collections::HashSet<isize>> = if localize_armed() {
        let bl = LOCALIZE_BLOCKLIST.with(|b| b.borrow().clone());
        let rg = LOCALIZE_RANGES.with(|r| r.borrow().clone());
        let (sets, widths) = compute_localize_sets(&chunks, &bl, &rg);
        // Record the localized byte ranges so the validate dual-run can skip
        // them (these comb_values bytes are intentionally left stale).
        LAST_LOCALIZED_BYTES.with(|b| {
            let mut v = b.borrow_mut();
            for set in &sets {
                for &off in set {
                    v.push((off, *widths.get(&off).unwrap_or(&8)));
                }
            }
        });
        sets
    } else {
        vec![std::collections::HashSet::new(); chunks.len()]
    };
    clear_current_local();
    let mut chunk_bodies: Vec<String> = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        CURRENT_LOCAL.with(|c| *c.borrow_mut() = localize_sets[i].clone());
        let mut cb = String::new();
        if !localize_sets[i].is_empty() {
            // Declare the localized signals (sorted → deterministic source so
            // the AOT-C cache hash is stable).
            let mut offs: Vec<isize> = localize_sets[i].iter().copied().collect();
            offs.sort_unstable();
            for off in offs {
                cb.push_str(&format!("    uint64_t {} = 0;\n", local_name(off)));
            }
        }
        for stmt in *chunk {
            let s = emit_stmt(stmt)?;
            cb.push_str("    ");
            cb.push_str(&s);
            cb.push('\n');
        }
        chunk_bodies.push(cb);
    }
    clear_current_local();

    if chunks.len() == 1 {
        body.push_str(
            "__attribute__((visibility(\"default\")))\n\
             void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log, intptr_t ff_delta) {\n\
             \x20   (void)write_log;\n",
        );
        body.push_str(&chunk_bodies[0]);
        body.push_str("}\n");
    } else {
        // Each chunk → noinline static function so gcc isolates its
        // regalloc/spill domain.  -flto can still inline if it judges
        // the cost worthwhile.
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
             void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log, intptr_t ff_delta) {\n",
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
            // A rhs_select on a plain variable is a bit-select on that variable
            // (value.select(hi, lo)); fold it into the variable's own select so
            // the wide-var select paths (emit_expr / emit_wide_expr) handle a
            // >128-bit source that isn't a C scalar.  The scalar rhs_select
            // branch below only reaches ≤128-bit rhs values.  Never for FF (the
            // FF path handles rhs_select itself).
            let folded_expr = match (a.rhs_select, &a.expr) {
                (
                    Some((hi, lo)),
                    ProtoExpression::Variable {
                        var_offset,
                        select: None,
                        dynamic_select: None,
                        var_full_width,
                        expr_context,
                        ..
                    },
                ) if hi >= lo && !a.dst.is_ff() => Some(ProtoExpression::Variable {
                    var_offset: *var_offset,
                    select: Some((hi, lo)),
                    dynamic_select: None,
                    width: hi - lo + 1,
                    var_full_width: *var_full_width,
                    expr_context: *expr_context,
                }),
                _ => None,
            };
            let eff_expr: &ProtoExpression = folded_expr.as_ref().unwrap_or(&a.expr);
            let eff_rhs_select = if folded_expr.is_some() {
                None
            } else {
                a.rhs_select
            };
            // A bare signed RHS narrower than the destination sign-extends at
            // the store (ProtoExpression::store_sign_extend_from): the value is
            // sign-extended to dst_width BEFORE the (plain/select) store, so a
            // field reaching above the RHS's own width picks up sign bits.
            // Handled inline below for dst_width <= 128 by producing a
            // sign-extended rhs (`se_from`); wider signed stores stay on
            // Cranelift (none occur in practice).
            let se_from = if eff_rhs_select.is_none() {
                eff_expr.store_sign_extend_from(a.dst_width)
            } else {
                None
            };
            // Sign-extend is handled inline below only for a comb destination of
            // width <= 128.  FF stores (emit_event_ff_assign doesn't sign-extend)
            // and wider comb stores stay on Cranelift, which extends in-register.
            if se_from.is_some() && (a.dst_width > 128 || a.dst.is_ff()) {
                return None;
            }
            // Route every FF write through the shadow-slot + WriteLogEntry path
            // (matching Cranelift) — a bare shadow store is never committed, so
            // the value is lost.  Needed in the comb path too: the is_ff
            // refinement can land an FF write here (e.g. function output args).
            // emit_event_ff_assign returns None on uncovered patterns, safely
            // bailing the module to Cranelift.
            if a.dst.is_ff() {
                return emit_event_ff_assign(a);
            }
            // A runtime-indexed bit-slice store into a comb target is emitted
            // below (after the rhs is computed), for dst_width <= 64.  Wider
            // dynamic-select stores stay on Cranelift.
            if a.dynamic_select.is_some() && (a.dst_width > 64 || a.dst_width == 0) {
                return None;
            }
            // Wide comb store via the wide-op helper table.  Two cases route
            // here: (a) dst_width > 128 (never a C scalar); (b) a 65-128-bit
            // dst whose RHS `builds_wide_pointer` — a wide-pointer result (e.g.
            // a wide shift over a >128-bit operand truncated to 128) that the
            // `__uint128_t` scalar path below (emit_expr_root) can't produce.
            // A 65-128-bit dst with a plain C-scalar RHS still takes the scalar
            // path.  A (bit-)select store IS emitted here (scalar fast path for
            // <=64-bit fields, full wide RMW otherwise); only rhs_select stays
            // on Cranelift (S1).
            if a.dst_width > 128 || (a.dst_width > 64 && eff_expr.builds_wide_pointer()) {
                // A non-foldable rhs_select on a wide store (rhs isn't a plain
                // variable) stays on Cranelift; dynamic_select bailed above.
                if eff_rhs_select.is_some() {
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
                let dst = format!("(uint8_t*)(comb_values + {store_off:#x})");
                let dmask = wpack(nb, a.dst_width);
                if let Some((hi, lo)) = a.select {
                    let nbits = hi.checked_sub(lo)?.checked_add(1)?;
                    // <=64-bit field → scalar word RMW (see
                    // emit_wide_narrow_field_store); wider fields fall through.
                    if nbits <= 64 {
                        return emit_wide_narrow_field_store(eff_expr, hi, lo, a.dst_width, |k| {
                            format!(
                                "(veryl_u64_ua*)(comb_values + {:#x})",
                                store_off + (k as isize) * 8
                            )
                        });
                    }
                    // General multi-word field — full wide RMW (2-state):
                    //   new = (old & ~rangemask) | ((src << lo) & rangemask)
                    // where rangemask = fill_ones(nbits) << lo.  `old` is read
                    // from the destination BEFORE the final copy overwrites it.
                    // Mirrors Cranelift emit_wide_select_rmw.
                    let mut pre = String::new();
                    let r = emit_wide_operand(eff_expr, nb, &mut pre)?;
                    let src = r.addr;
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
                        src = src,
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
                let mut pre = String::new();
                let r = emit_wide_operand(eff_expr, nb, &mut pre)?;
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
            let rhs_raw = emit_expr_root(eff_expr)?;
            // Sign-extend a bare signed RHS to dst_width before the store
            // (`se_from` = the RHS width). dst_width <= 128 is guaranteed here
            // (wider bailed above). The extension fills bits [w..dst_width) with
            // the RHS sign, so a select field or plain store reaching those bits
            // reads the sign — matching value.expand(dst_width, true).
            let rhs_unselected = match se_from {
                Some(w) if w < 64 && a.dst_width <= 64 => {
                    let sh = 64 - w;
                    format!("((uint64_t)(((int64_t)((uint64_t)({rhs_raw}) << {sh})) >> {sh}))")
                }
                Some(w) if w < 128 => {
                    // dst_width 65..128: extend in __int128_t.
                    let sh = 128 - w;
                    format!(
                        "((__uint128_t)(((__int128_t)((__uint128_t)({rhs_raw}) << {sh})) >> {sh}))"
                    )
                }
                _ => rhs_raw,
            };
            let rhs_str = match eff_rhs_select {
                None => rhs_unselected,
                Some((rhs_hi, rhs_lo)) => {
                    let nbits = rhs_hi.checked_sub(rhs_lo)?.checked_add(1)?;
                    // A field wider than 128 bits isn't a C scalar; a wide-value
                    // rhs (emit_expr_root None) already bailed above.  Extract
                    // [rhs_lo..rhs_hi] mirroring value.select(rhs_hi, rhs_lo).
                    if nbits > 128 {
                        return None;
                    }
                    if nbits > 64 {
                        // 65..128-bit field → __uint128_t shift + mask.
                        let inner = format!(
                            "(((__uint128_t)({src})) >> {lo})",
                            src = rhs_unselected,
                            lo = rhs_lo
                        );
                        if nbits < 128 {
                            mask_u128(&inner, nbits)
                        } else {
                            inner
                        }
                    } else if nbits == 64 {
                        // Exactly 64: mask would overflow `1u64 << 64`.
                        format!(
                            "((uint64_t)(({src}) >> {lo}))",
                            src = rhs_unselected,
                            lo = rhs_lo
                        )
                    } else {
                        let mask = (1u64 << nbits) - 1;
                        format!(
                            "((({src}) >> {lo}) & 0x{m:x}ULL)",
                            src = rhs_unselected,
                            lo = rhs_lo,
                            m = mask,
                        )
                    }
                }
            };
            // FF targets returned via emit_event_ff_assign above, so the
            // destination here is always comb.
            let VarOffset::Comb(store_off) = a.dst else {
                return None;
            };
            let buf = "comb_values";
            // Runtime-indexed field store (dst_width <= 64 guaranteed above):
            // idx = clamp(index_expr), field = [idx*elem_width ..
            // +window-1], RMW value's low `window` bits there.  Mirrors
            // AssignStatement::eval_step's dynamic_select branch
            // (current.assign(value, beg=end+window-1, end=idx*elem_width)).
            if let Some(dyn_sel) = &a.dynamic_select {
                if dyn_sel.window == 0 || dyn_sel.window > 64 || dyn_sel.elem_width == 0 {
                    return None;
                }
                let idx_str = emit_expr(&dyn_sel.index_expr)?;
                let max_idx = dyn_sel.num_elements.saturating_sub(1);
                let vmask: u64 = if dyn_sel.window >= 64 {
                    !0u64
                } else {
                    (1u64 << dyn_sel.window) - 1
                };
                return Some(format!(
                    "{{ uint64_t _idx_raw = (uint64_t)({idx}); \
                        uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                        uint64_t _sh = _idx * {ew}; \
                        uint64_t _v = ((uint64_t)({rhs})) & 0x{vmask:x}ULL; \
                        {ct} _o = *(({ct}*)({b} + {o:#x})); \
                        *(({ct}*)({b} + {o:#x})) = \
                          ({ct})((_o & ({ct})(~(0x{vmask:x}ULL << _sh))) | ({ct})(_v << _sh)); }}",
                    idx = idx_str,
                    max = max_idx,
                    ew = dyn_sel.elem_width,
                    rhs = rhs_str,
                    vmask = vmask,
                    ct = cty,
                    b = buf,
                    o = store_off,
                ));
            }
            // Bit-select store is read-modify-write.
            if let Some((hi, lo)) = a.select {
                let nbits = hi.checked_sub(lo)?.checked_add(1)?;
                // Wide (65..128-bit) destination: the single-u64 RMW path
                // below can't reach a field at lo ≥ 64.
                if a.dst_width > 64 && a.dst_width <= 128 {
                    if nbits == 0 || lo + nbits > 128 {
                        return None;
                    }
                    let fmask: u128 = if nbits >= 128 {
                        !0u128
                    } else {
                        (1u128 << nbits) - 1
                    };
                    let pos: u128 = fmask << lo;
                    return Some(format!(
                        "{{ __uint128_t _v = ((__uint128_t)({rhs})) \
                              & (((__uint128_t)0x{fmhi:x}ULL << 64) | (__uint128_t)0x{fmlo:x}ULL); \
                            __uint128_t _o = *(({ct}*)({b} + {o:#x})); \
                            *(({ct}*)({b} + {o:#x})) = ({ct})((_o \
                              & ~(((__uint128_t)0x{phi:x}ULL << 64) | (__uint128_t)0x{plo:x}ULL)) \
                              | (_v << {lo})); }}",
                        rhs = rhs_str,
                        ct = cty,
                        b = buf,
                        o = store_off,
                        fmhi = (fmask >> 64) as u64,
                        fmlo = fmask as u64,
                        phi = (pos >> 64) as u64,
                        plo = pos as u64,
                        lo = lo,
                    ));
                }
                // A full-width [63:0] select on a 64-bit dst is a plain store;
                // the single-u64 mask math below would overflow (`1u64 << 64`).
                if nbits == 64 && lo == 0 {
                    return Some(format!(
                        "*(({ct}*)({b} + {o:#x})) = ({ct})({rhs});",
                        ct = cty,
                        b = buf,
                        o = store_off,
                        rhs = rhs_str,
                    ));
                }
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
            } else if is_localized(store_off) {
                // Localized comb intermediate: assign the (width-masked, zero-
                // extended) value to the C local instead of storing to the
                // comb buffer.  Only ≤64-bit select-less scalars reach here
                // (compute_localize_sets' candidate filter), so native_bits is
                // 32 or 64 and a uint64_t local holds the value exactly.
                let native_bits = nb * 8;
                let val = if a.dst_width < native_bits && a.dst_width > 0 {
                    let mask = (1u64 << a.dst_width) - 1;
                    format!(
                        "(((uint64_t)({rhs})) & 0x{m:x}ULL)",
                        rhs = rhs_str,
                        m = mask
                    )
                } else {
                    format!("((uint64_t)({ct})({rhs}))", ct = cty, rhs = rhs_str)
                };
                Some(format!("{nm} = {val};", nm = local_name(store_off)))
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
                        c = wrap_expect(&c),
                        t = true_body,
                        f = false_body,
                    ))
                }
            }
        }
        ProtoStatement::Case(case_stmt) => {
            // Build the `if / else if / ... / else` cascade iteratively so a
            // large `case` doesn't recurse in the emitter (only into arm bodies
            // via `emit_block`).
            let mut out = String::new();
            for (n, arm) in case_stmt.arms.iter().enumerate() {
                let c = wrap_expect(&emit_expr(&arm.cond)?);
                let body = emit_block(&arm.body)?;
                let kw = if n == 0 { "if" } else { " else if" };
                out.push_str(&format!("{kw} ({c}) {{ {body} }}"));
            }
            let default_body = emit_block(&case_stmt.default)?;
            if case_stmt.arms.is_empty() {
                out.push_str(&format!("{{ {default_body} }}"));
            } else {
                out.push_str(&format!(" else {{ {default_body} }}"));
            }
            Some(out)
        }
        ProtoStatement::SequentialBlock(body) => {
            let inner = emit_block(body)?;
            Some(format!("{{ {} }}", inner))
        }
        ProtoStatement::AssignDynamic(a) => {
            // Narrow signed bare RHS sign-extends at the store; bail to the
            // Cranelift/interpreter path (see the Assign arm above).
            if a.rhs_select.is_none() && a.expr.store_sign_extend_from(a.dst_width).is_some() {
                return None;
            }
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
            // Dynamic-indexed comb store, >64-bit element.  Such a `var` array
            // (ff_log_base_current_offset None) lives in the comb buffer, so
            // eval_step writes directly to `base + stride*idx` with no write-log
            // push; mirror that (RMW for a bit-select, whole write otherwise).
            if a.dst_width > 64 {
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
                // `_pa` is the element byte-address; declared in the block below
                // before the wide ops reference it.  `pre` (the RHS scratch)
                // does not reference `_pa`/`_idx`, so the ordering is sound.
                let store = if let Some((hi, lo)) = a.select {
                    let nbits = hi.checked_sub(lo)?.checked_add(1)?;
                    // <=64-bit field → scalar word RMW of the runtime-addressed
                    // element; see emit_wide_narrow_field_store.
                    if nbits <= 64 {
                        emit_wide_narrow_field_store(&a.expr, hi, lo, a.dst_width, |k| {
                            format!("(veryl_u64_ua*)(_pa + {})", k * 8)
                        })?
                    } else {
                        // General multi-word field — runtime-addressed wide RMW:
                        //   new = (old & ~rangemask) | ((src << lo) & rangemask)
                        // Mirrors the static wide-store RMW (Cranelift parity).
                        let mut pre = String::new();
                        let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
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
                    }
                } else if nb == 16 {
                    // A 65-128-bit element fits a native __uint128_t: masked
                    // store like the static path, no helper table (>128 below).
                    let mut pre = String::new();
                    let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
                    let m: u128 = if a.dst_width >= 128 {
                        !0u128
                    } else {
                        (1u128 << a.dst_width) - 1
                    };
                    format!(
                        "{pre}*((veryl_u128_ua*)_pa) = (*(const veryl_u128_ua*)({src})) \
                         & (((__uint128_t)0x{hi:x}ULL << 64) | (__uint128_t)0x{lo:x}ULL);",
                        src = r.addr,
                        hi = (m >> 64) as u64,
                        lo = m as u64,
                    )
                } else {
                    // Mask the destination, not the source (which may alias a
                    // flat-buffer read).
                    let mut pre = String::new();
                    let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
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
            if a.dst_num_elements == 0 || a.dst_width == 0 {
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
            // Inline the pre-chunk statements instead of calling `cb.func`, so
            // gcc keeps values in registers across the chunk boundary.
            // `original_stmts` already hold this instance's actual offsets (the
            // reuse paths pre-adjust them), so unlike Cranelift's relocated
            // `cb.func` the inlined C must NOT re-add ff/comb_delta_bytes —
            // that double-counts and corrupts memory under alias-off reuse.
            let mut s = String::from("{ ");
            for stmt in &cb.original_stmts {
                let inner = emit_stmt(stmt)?;
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

/// `ProtoStatement::For` → C `for` loop.  Covers Forward / Reverse ranges
/// with constant or dynamic (≤64-bit) bounds and a loop var ≤ 64 bits;
/// mirrors the Cranelift JIT gate (`ProtoForStatement::can_build_binary`).
/// Stepped ranges (arbitrary-op advance) stay on the interpreter.
fn emit_for(for_stmt: &ProtoForStatement) -> Option<String> {
    if for_stmt.var_width == 0 || for_stmt.var_width > 64 {
        return None;
    }
    // A loop bound as a C expression.  Const folds to a literal; Dynamic
    // (≤64-bit) emits its scalar expression.  `add_one` applies the inclusive
    // end bump (mirrors the interpreter / const path's `e += 1`).
    let bound_c = |b: &ProtoForBound, add_one: bool| -> Option<String> {
        match b {
            ProtoForBound::Const(v) => {
                let v = if add_one { v.checked_add(1)? } else { *v };
                Some(format!("{v}ULL"))
            }
            ProtoForBound::Dynamic(e) => {
                if e.width() > 64 {
                    return None;
                }
                let c = emit_expr(e)?;
                if add_one {
                    Some(format!("(({c}) + 1ULL)"))
                } else {
                    Some(format!("({c})"))
                }
            }
        }
    };
    // Const trip count (loop iterations), or None when a bound is dynamic.
    let const_trips =
        |start: &ProtoForBound, end: &ProtoForBound, inclusive: bool, step: u64| -> Option<u64> {
            let s = match start {
                ProtoForBound::Const(v) => *v,
                _ => return None,
            };
            let e0 = match end {
                ProtoForBound::Const(v) => *v,
                _ => return None,
            };
            let e = if inclusive { e0.checked_add(1)? } else { e0 };
            Some(if e > s { (e - s).div_ceil(step) } else { 0 })
        };

    // Loop-control fragments referencing hoisted bound temps `_lo`/`_hi`,
    // evaluated once (as the interpreter/Cranelift read the bounds a single
    // time before looping).  `int64_t` for Reverse so the signed `>= _lo`
    // guard terminates on underflow past `_lo`, matching the emitted SV
    // `for (int i = hi - 1; i >= lo; i -= step)`.
    let (var_ty, lo, hi, init, cond, incr, trips) = match &for_stmt.range {
        ProtoForRange::Forward {
            start,
            end,
            inclusive,
            step,
        } => {
            if *step == 0 {
                return None;
            }
            (
                "uint64_t",
                bound_c(start, false)?,
                bound_c(end, *inclusive)?,
                "uint64_t _it = _lo".to_string(),
                "_it < _hi".to_string(),
                format!("_it += {step}ULL"),
                const_trips(start, end, *inclusive, *step),
            )
        }
        ProtoForRange::Reverse {
            start,
            end,
            inclusive,
            step,
        } => {
            if *step == 0 {
                return None;
            }
            (
                "int64_t",
                bound_c(start, false)?,
                bound_c(end, *inclusive)?,
                "int64_t _it = _hi - 1".to_string(),
                "_it >= _lo".to_string(),
                format!("_it -= {step}ULL"),
                const_trips(start, end, *inclusive, *step),
            )
        }
        ProtoForRange::Stepped { .. } => return None,
    };

    let nb = native_bytes(for_stmt.var_width);
    let cty = native_c_type(nb)?;
    let (buf, off) = match for_stmt.var_offset {
        VarOffset::Ff(o) => ("ff_values", o),
        VarOffset::Comb(o) => ("comb_values", o),
    };

    // Body pushes (FF write-log entries) execute once per iteration; scale the
    // reserve counters by the trip count.  A dynamic bound has no compile-time
    // trip count, so a body that pushes must fall back to the interpreter.
    let narrow_before = EVENT_NARROW_PUSHES.with(|c| c.get());
    let wide_before = EVENT_WIDE_PUSHES.with(|c| c.get());
    let mut body = String::new();
    for s in &for_stmt.body {
        body.push_str(&emit_stmt(s)?);
        body.push(' ');
    }
    let narrow_body = EVENT_NARROW_PUSHES
        .with(|c| c.get())
        .saturating_sub(narrow_before);
    let wide_body = EVENT_WIDE_PUSHES
        .with(|c| c.get())
        .saturating_sub(wide_before);
    if narrow_body > 0 || wide_body > 0 {
        let trips = trips?;
        EVENT_NARROW_PUSHES.with(|c| c.set(narrow_before + narrow_body.saturating_mul(trips)));
        EVENT_WIDE_PUSHES.with(|c| c.set(wide_before + wide_body.saturating_mul(trips)));
    }

    Some(format!(
        "{{ {var_ty} _lo = {lo}, _hi = {hi}; \
         for ({init}; {cond}; {incr}) {{ \
            *(({cty}*)({buf} + {off:#x})) = ({cty})_it; \
            {body} \
         }} }}",
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
        ProtoExpression::HierVariable(_) => None,
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
                //   shift right by idx*elem_width, mask `window` bits.
                if *var_full_width == 0
                    || dyn_sel.elem_width == 0
                    || dyn_sel.window == 0
                    || dyn_sel.window >= 64
                    || dyn_sel.num_elements == 0
                {
                    return None;
                }
                let idx_str = emit_expr(&dyn_sel.index_expr)?;
                let max_idx = dyn_sel.num_elements.saturating_sub(1);
                let mask = (1u64 << dyn_sel.window) - 1;
                if *var_full_width <= 128 {
                    let load = emit_var_load(var_offset, *var_full_width)?;
                    // Result is <= 64 bits (window < 64); cast down so a
                    // __uint128_t load (65..128-bit var) still yields a scalar.
                    return Some(format!(
                        "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                            uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                            (uint64_t)((({load}) >> (_idx * {ew})) & 0x{mask:x}ULL); }})",
                        idx = idx_str,
                        max = max_idx,
                        load = load,
                        ew = dyn_sel.elem_width,
                        mask = mask,
                    ));
                }
                // Wide (>128-bit) underlying var: funnel-read a 64-bit window at
                // the runtime bit offset idx*elem_width from the flat buffer,
                // then mask to `window` bits.  Reads past the end (`_hi`) are
                // guarded to 0.  Mirrors (fullvar >> (idx*ew)) & mask.
                let (buf, off) = match var_offset {
                    VarOffset::Ff(o) => ("ff_values", *o),
                    VarOffset::Comb(o) => ("comb_values", *o),
                };
                if off < 0 {
                    return None;
                }
                let nw = native_bytes(*var_full_width) / 8;
                return Some(format!(
                    "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                        uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                        uint64_t _bit = _idx * {ew}; uint64_t _w = _bit >> 6; uint32_t _s = (uint32_t)(_bit & 63); \
                        const veryl_u64_ua* _p = (const veryl_u64_ua*)({b} + {off:#x}); \
                        uint64_t _lo = _w < {nw}ull ? _p[_w] : 0; \
                        uint64_t _hi = (_w + 1) < {nw}ull ? _p[_w + 1] : 0; \
                        uint64_t _vv = _s == 0 ? _lo : ((_lo >> _s) | (_hi << (64 - _s))); \
                        (_vv & 0x{mask:x}ULL); }})",
                    idx = idx_str,
                    max = max_idx,
                    ew = dyn_sel.elem_width,
                    b = buf,
                    off = off,
                    nw = nw,
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
                    if nbits <= 128 {
                        let (buf, off) = match var_offset {
                            VarOffset::Ff(o) => ("ff_values", *o),
                            VarOffset::Comb(o) => ("comb_values", *o),
                        };
                        if off < 0 {
                            return None;
                        }
                        if nbits <= 64 {
                            return Some(emit_wide_var_select_read(buf, off, *lo, nbits));
                        }
                        // 65..128-bit window → __uint128_t.
                        return Some(emit_wide_select_read_wide_at(
                            &format!("{buf} + {off:#x}"),
                            *lo,
                            nbits,
                        ));
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
                if nbits > 128 {
                    return None; // > 128-bit select → wide pointer, not a scalar
                }
                if nbits > 64 {
                    // 65..128-bit window from a ≤128-bit var (loaded as
                    // __uint128_t since load_width = hi+1 ≥ 65): shift down and
                    // mask to nbits.
                    let shifted = format!("(((__uint128_t)({load})) >> {lo})");
                    if nbits < 128 {
                        return Some(mask_u128(&shifted, nbits));
                    }
                    return Some(shifted);
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
                Op::BitNot | Op::Sub => {
                    if xw > 64 {
                        // 65..128-bit operand: compute in __uint128_t — the
                        // (uint64_t) rebuild below would drop bits 64..127.
                        let inner = if matches!(op, Op::BitNot) {
                            format!("(~((__uint128_t)({xs})))")
                        } else {
                            format!("((__uint128_t)0 - ((__uint128_t)({xs})))")
                        };
                        let w = expr_context.width;
                        if needs_clean && w > 0 && w < 128 {
                            return Some(mask_u128(&inner, w));
                        }
                        return Some(inner);
                    }
                    let inner = if matches!(op, Op::BitNot) {
                        format!("(~({}))", xv)
                    } else {
                        format!("(-({}))", xv)
                    };
                    // `~x`/`-x` leave dirty bits at/above the width; Cranelift and
                    // the interpreter mask to width, so an inlined consumer reading
                    // the high bits (Eq/Ne, unsigned compare, shift) must too.
                    if needs_clean && expr_context.width > 0 && expr_context.width < 64 {
                        let mask = (1u64 << expr_context.width) - 1;
                        Some(format!("(({}) & 0x{:x}ULL)", inner, mask))
                    } else if needs_clean && expr_context.width > 64 && expr_context.width < 128 {
                        // 65..128-bit context over a ≤64-bit operand: the int64
                        // result sign-extends through the __uint128_t promotion
                        // (the ones in [xw..width) for ~/-), then the mask trims
                        // [width..128).
                        Some(mask_u128(
                            &format!("((__uint128_t)(__int128_t)({inner}))"),
                            expr_context.width,
                        ))
                    } else {
                        Some(inner)
                    }
                }
                // Unary reductions over a ≤128-bit operand → a 1-bit result;
                // a >128-bit operand is handled by emit_wide_reduce_unary above.
                // OR = any-bit-set, AND = all-bits-set, XOR = parity; mirrors
                // expression.rs build_binary_wide_unary's reduction arm.
                Op::BitOr | Op::BitNor | Op::BitAnd | Op::BitNand | Op::BitXor | Op::BitXnor => {
                    if xw == 0 {
                        return None;
                    }
                    if xw <= 64 {
                        let mask = if xw >= 64 { u64::MAX } else { (1u64 << xw) - 1 };
                        let m = format!("(((uint64_t)({xs})) & 0x{mask:x}ULL)");
                        Some(match op {
                            Op::BitOr => format!("((uint64_t)(({m}) != 0))"),
                            Op::BitNor => format!("((uint64_t)(({m}) == 0))"),
                            Op::BitAnd => format!("((uint64_t)(({m}) == 0x{mask:x}ULL))"),
                            Op::BitNand => format!("((uint64_t)(({m}) != 0x{mask:x}ULL))"),
                            Op::BitXor => format!("((uint64_t)__builtin_parityll({m}))"),
                            Op::BitXnor => {
                                format!("((uint64_t)(__builtin_parityll({m}) ^ 1))")
                            }
                            _ => unreachable!(),
                        })
                    } else {
                        // 65..128-bit operand in __uint128_t.
                        let masked = if xw < 128 {
                            mask_u128(&format!("((__uint128_t)({xs}))"), xw)
                        } else {
                            format!("((__uint128_t)({xs}))")
                        };
                        let allones = if xw < 128 {
                            let m: u128 = (1u128 << xw) - 1;
                            format!(
                                "((((__uint128_t)0x{hi:x}ULL) << 64) | (__uint128_t)0x{lo:x}ULL)",
                                hi = (m >> 64) as u64,
                                lo = m as u64,
                            )
                        } else {
                            "(~(__uint128_t)0)".to_string()
                        };
                        let parity = "(__builtin_parityll((uint64_t)_m) \
                                      ^ __builtin_parityll((uint64_t)(_m >> 64)))";
                        Some(match op {
                            Op::BitOr => {
                                format!("({{ __uint128_t _m = {masked}; (uint64_t)(_m != 0); }})")
                            }
                            Op::BitNor => {
                                format!("({{ __uint128_t _m = {masked}; (uint64_t)(_m == 0); }})")
                            }
                            Op::BitAnd => format!(
                                "({{ __uint128_t _m = {masked}; (uint64_t)(_m == {allones}); }})"
                            ),
                            Op::BitNand => format!(
                                "({{ __uint128_t _m = {masked}; (uint64_t)(_m != {allones}); }})"
                            ),
                            Op::BitXor => {
                                format!("({{ __uint128_t _m = {masked}; (uint64_t){parity}; }})")
                            }
                            Op::BitXnor => format!(
                                "({{ __uint128_t _m = {masked}; (uint64_t)({parity} ^ 1); }})"
                            ),
                            _ => unreachable!(),
                        })
                    }
                }
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
                && matches!(
                    op,
                    Op::Less
                        | Op::Greater
                        | Op::LessEq
                        | Op::GreaterEq
                        | Op::Eq
                        | Op::Ne
                        | Op::EqWildcard
                        | Op::NeWildcard
                );
            // Op::Div / Op::Rem use the AND of operand signedness, as the
            // Cranelift backend does.  expr_context.signed alone is not
            // sufficient because merge() with an unsigned sibling can
            // strip the bit even when both operands ARE signed.
            // We approximate by trusting expr_context.signed for the
            // outer expression — div/rem are expr_context.signed
            // exactly when both operands are signed.
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
            // VERYL_AOT_C_BOOLFOLD: narrow LogicAnd/LogicOr as a branchless
            // bitwise reduce of the `!=0` predicates — force-evaluates the
            // short-circuited right arm to drop the data-dependent branch.
            // Logic ops are 0/1, so no width mask.
            let bf = boolfold_mode();
            if bf > 0
                && matches!(op, Op::LogicAnd | Op::LogicOr)
                && (bf == 2 || is_cheap_boolfold_arm(y, 3))
            {
                let bit = if matches!(op, Op::LogicAnd) { "&" } else { "|" };
                return Some(format!("((uint64_t)((({xs}) != 0) {bit} (({ys}) != 0)))"));
            }
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
                    Op::Eq | Op::EqWildcard => "==",
                    Op::Ne | Op::NeWildcard => "!=",
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
            // Pow (x ** y): binary exponentiation in native integer arithmetic
            // (modular via wraparound), then mask to width.  The native
            // mod-2^64/2^128 then a final mask to `width` is exact because
            // 2^width | 2^{64,128}.  Mirrors the Cranelift Op::Pow loop; wide
            // (>128) Pow stays on Cranelift/interpreter.
            if matches!(op, Op::Pow) {
                let w = expr_context.width;
                if w == 0 || w > 128 {
                    return None;
                }
                let id = next_wide_tmp();
                let (cty_p, one) = if w <= 64 {
                    ("uint64_t", "(uint64_t)1")
                } else {
                    ("__uint128_t", "(__uint128_t)1")
                };
                // Mask constant of `w` low bits, typed to match cty_p.
                let mask_c = if w >= 128 {
                    "(~(__uint128_t)0)".to_string()
                } else if w > 64 {
                    let m: u128 = (1u128 << w) - 1;
                    format!(
                        "(((__uint128_t)0x{hi:x}ULL << 64) | (__uint128_t)0x{lo:x}ULL)",
                        hi = (m >> 64) as u64,
                        lo = m as u64
                    )
                } else if w == 64 {
                    "(~(uint64_t)0)".to_string()
                } else {
                    format!("(uint64_t)0x{m:x}ULL", m = (1u64 << w) - 1)
                };
                // IEEE 1800 11.4.3.1: a negative signed exponent yields 0 (|base|
                // > 1) / 1 (base==1) / ±1 (base==-1); the unsigned loop would
                // treat it as a huge count.  Applied only for a signed exponent
                // of width 1..=64, mirroring the Cranelift Op::Pow table.
                let y_w = y.width();
                let neg_fixup = if y.expr_context().signed && y_w > 0 && y_w <= 64 {
                    let base_is_m1 = if expr_context.signed {
                        format!("_pb{id} == {mask_c}")
                    } else {
                        format!("_pb{id} == {one}")
                    };
                    // `base == 1` is the outermost select (as in the Cranelift
                    // reference): base 1 to any power is 1, overriding the
                    // `base_is_m1` arm, which for an unsigned base aliases to
                    // `== 1` and would otherwise yield all-ones for an odd
                    // exponent.
                    format!(
                        "_pb{id} = _pb{id} & {mask_c}; \
                         int _neg{id} = (int)((_pe0{id} >> {sh}) & 1); \
                         int _odd{id} = (int)(_pe0{id} & 1); \
                         {cty_p} _tab{id} = (_pb{id} == {one}) ? {one} \
                                        : (({base_is_m1}) ? (_odd{id} ? {mask_c} : {one}) : ({cty_p})0); \
                         _pr{id} = _neg{id} ? _tab{id} : _pr{id}; ",
                        sh = y_w - 1,
                    )
                } else {
                    String::new()
                };
                // A signed base is sign-extended to the op width before the
                // multiply (Verilog widens operands to the result signedness);
                // the u64/u128 wraparound + final mask then gives the right
                // low bits.  The exponent stays raw (the loop reads its bits;
                // a negative one is caught by neg_fixup).
                let x_w = x.width();
                let base = if expr_context.signed && x_w > 0 && w <= 64 && x_w < 64 {
                    let sh = 64 - x_w;
                    format!("((uint64_t)(((int64_t)((uint64_t)({xs}) << {sh})) >> {sh}))")
                } else if expr_context.signed && x_w > 0 && w > 64 && x_w < 128 {
                    let sh = 128 - x_w;
                    format!("((__uint128_t)(((__int128_t)((__uint128_t)({xs}) << {sh})) >> {sh}))")
                } else {
                    xs.clone()
                };
                let body = format!(
                    "({{ {cty_p} _pb{id}=({cty_p})({base}); {cty_p} _pe0{id}=({cty_p})({ys}); \
                        {cty_p} _we{id}=_pe0{id}; {cty_p} _wb{id}=_pb{id}; {cty_p} _pr{id}={one}; \
                        while(_we{id}){{ if(_we{id}&1) _pr{id}*=_wb{id}; _wb{id}*=_wb{id}; _we{id}>>=1; }} \
                        {neg_fixup}_pr{id}; }})"
                );
                return Some(if w < 64 {
                    format!("(({body}) & 0x{m:x}ULL)", m = (1u64 << w) - 1)
                } else if w > 64 && w < 128 {
                    mask_u128(&body, w)
                } else {
                    body
                });
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
                // mode (in 2-state `mask_xz` is always 0
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
                // Bitwise ops (And/Or/Xor) are sign-agnostic — the result bits
                // don't depend on operand signedness — so a signed 65..128-bit
                // result is fine in __uint128_t; only arithmetic/shift ops that
                // would need a 128-bit sign-extension bail on signedness.
                let signed_wide_bail =
                    expr_context.signed && !matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor);
                if expr_context.width > 64 && (wide_truncates || signed_wide_bail) {
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
                        "(((__uint128_t)({ys})) >= {w} ? (__uint128_t)0 : (({xs}) {c_op} ({ys})))"
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
                    let growing = matches!(
                        op,
                        Op::Add | Op::Sub | Op::Mul | Op::LogicShiftL | Op::ArithShiftL
                    );
                    if needs_clean && expr_context.width < 64 && growing {
                        let mask = (1u64 << expr_context.width) - 1;
                        match &overflow_cond {
                            Some(cond) => format!(
                                "({{ uint64_t _t = ({s}); \
                                 if (__builtin_expect(({cond}) != 0, 0)) {{ _t &= 0x{mask:x}ULL; \
                                 __asm__ volatile(\"\" : \"+r\"(_t)); }} _t; }})"
                            ),
                            None => format!("(({s}) & 0x{mask:x}ULL)"),
                        }
                    } else if needs_clean
                        && expr_context.width > 64
                        && expr_context.width < 128
                        && growing
                    {
                        // The op is computed in __uint128_t, so e.g. a 100-bit
                        // add keeps a real carry at bit 100 that corrupts an
                        // inlined comparison.
                        mask_u128(&s, expr_context.width)
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
                            "((((__uint128_t)({})) >= 64 ? 0 : ((uint64_t)(({}) & 0x{:x}ULL)) >> ({})))",
                            ye, xe, tmask, ye,
                        ));
                    }
                    if matches!(op, Op::LogicShiftL | Op::ArithShiftL | Op::LogicShiftR) {
                        // C shifts are UB for counts >= 64 (x86 wraps mod 64);
                        // SystemVerilog yields 0.
                        return Some(wmask(format!(
                            "(((__uint128_t)({ye})) >= 64 ? 0 : (({xe}) {c_op} ({ye})))"
                        )));
                    }
                    return Some(wmask(format!("(({}) {} ({}))", xe, c_op, ye)));
                }
                if matches!(op, Op::LogicShiftL | Op::ArithShiftL | Op::LogicShiftR) {
                    return Some(wmask(format!(
                        "(((__uint128_t)({ys})) >= 64 ? 0 : (({xs}) {c_op} ({ys})))"
                    )));
                }
                // C integer division by zero is UB (traps under -O3); yield 0
                // to match the interpreter and the Cranelift lowering.
                if matches!(op, Op::Div | Op::Rem) {
                    return Some(format!("(({ys}) == 0 ? 0 : (({xs}) {c_op} ({ys})))"));
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
                    if x_w == 0 {
                        return None; // zero-width signed shift
                    }
                    // Wide (>128-bit) operand with a scalar (≤128-bit) result:
                    // materialize the operand wide, vw_ashr/vw_lshr by the count
                    // (which handles count >= width), then read back the low
                    // result bits.  The whole thing is a GCC statement
                    // expression so the scratch decls stay inline.
                    if x_w > 128 {
                        let w = expr_context.width;
                        if w == 0 || w > 128 {
                            return None;
                        }
                        let src_nb = native_bytes(x_w);
                        let src_nw = src_nb / 8;
                        let mut pre = String::new();
                        let xr = emit_wide_operand(x, src_nb, &mut pre)?;
                        let count = emit_expr(y)?;
                        let shift_fn = if expr_context.signed {
                            "vw_ashr"
                        } else {
                            "vw_lshr"
                        };
                        let id = next_wide_tmp();
                        let read_raw = if w <= 64 {
                            format!("((veryl_u64_ua*)_r{id})[0]")
                        } else {
                            format!(
                                "(((__uint128_t)((veryl_u64_ua*)_r{id})[0]) \
                                 | ((__uint128_t)((veryl_u64_ua*)_r{id})[1] << 64))"
                            )
                        };
                        // Mask the low bits above `width` cleared by the shift's
                        // sign fill (vw_ashr fills to x_w, not `width`).
                        let read = if w < 64 {
                            format!("(({read_raw}) & 0x{m:x}ULL)", m = (1u64 << w) - 1)
                        } else if w > 64 && w < 128 {
                            mask_u128(&read_raw, w)
                        } else {
                            read_raw
                        };
                        let shift_arg = if expr_context.signed {
                            format!("{pk}u", pk = wpack(src_nb, x_w))
                        } else {
                            format!("{src_nb}u")
                        };
                        return Some(format!(
                            "({{ {pre} uint64_t _r{id}[{src_nw}]; \
                                {shift_fn}((uint8_t*)_r{id}, {src}, (uint64_t)({count}), {shift_arg}); \
                                {read}; }})",
                            src = xr.addr,
                        ));
                    }
                    if x_w > 64 {
                        // 65..128-bit operand in __uint128_t.  Count >= width
                        // yields all-sign (signed) / 0 (unsigned); C `>>` is UB
                        // past 127, so clamp.
                        if !expr_context.signed {
                            // `>>>` on an unsigned operand is a logical shift.
                            return Some(format!(
                                "(((__uint128_t)({ys})) >= {x_w} ? (__uint128_t)0 : (((__uint128_t)({xs})) >> ((uint64_t)({ys}))))"
                            ));
                        }
                        // Signed: sign-extend from x_w to 128 (shift the sign bit
                        // to bit 127, arithmetic-shift back), then arithmetic-
                        // shift right, clamping the count to x_w-1.
                        let lshift = 128 - x_w;
                        let sx = if lshift == 0 {
                            format!("((__int128_t)((__uint128_t)({xs})))")
                        } else {
                            format!(
                                "(((__int128_t)(((__uint128_t)({xs})) << {lshift})) >> {lshift})"
                            )
                        };
                        return Some(format!(
                            "((__uint128_t)(({sx}) >> (((__uint128_t)({ys})) >= {x_w} ? {clamp} : ((uint64_t)({ys})))))",
                            clamp = x_w - 1,
                        ));
                    }
                    if !expr_context.signed {
                        // `>>>` on an *unsigned* operand is a logical
                        // (zero-fill) shift — only a signed operand gets
                        // sign-extended.  e.g. `8'hf1 >>> 2` is 0x003c,
                        // not 0xfffc.
                        Some(format!(
                            "(((__uint128_t)({ys})) >= 64 ? 0 : ((uint64_t)({xs}) >> ({ys})))",
                            xs = xs,
                            ys = ys,
                        ))
                    } else if x_w == 64 {
                        // Clamp the count to 63: `>>>` by >= width fills
                        // with the sign bit, which a 63-shift produces.
                        Some(format!(
                            "((uint64_t)((int64_t)((uint64_t)({xs})) >> (((__uint128_t)({ys})) >= 64 ? 63 : ({ys}))))",
                            xs = xs,
                            ys = ys,
                        ))
                    } else {
                        let shift = 64 - x_w;
                        Some(format!(
                            "((uint64_t)((((int64_t)((uint64_t)({xs}) << {sh})) >> {sh}) >> (((__uint128_t)({ys})) >= 64 ? 63 : ({ys}))))",
                            xs = xs,
                            ys = ys,
                            sh = shift,
                        ))
                    }
                }
                // `~` sets every bit above the width; mask when a consumer
                // reads the high bits (mirrors the unary BitNot emission).
                Op::BitXnor | Op::BitNand | Op::BitNor => {
                    let inner = match op {
                        Op::BitXnor => format!("(~(({xs}) ^ ({ys})))"),
                        Op::BitNand => format!("(~(({xs}) & ({ys})))"),
                        Op::BitNor => format!("(~(({xs}) | ({ys})))"),
                        _ => unreachable!(),
                    };
                    let w = expr_context.width;
                    if needs_clean && w > 0 && w < 64 {
                        Some(format!("(({inner}) & 0x{:x}ULL)", (1u64 << w) - 1))
                    } else if needs_clean && w > 64 && w < 128 {
                        Some(mask_u128(&inner, w))
                    } else {
                        Some(inner)
                    }
                }
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
            width,
            ..
        } => {
            // The condition is a truthy test, so its high bits must be clean;
            // the selected branch becomes this result, so the branches inherit
            // `needs_clean`.
            let c = emit_expr(cond)?;
            // Both-signed branches sign-extend to the result width
            // (LRM 11.4.11); the plain C ternary would zero-extend the
            // narrower one.  The sign-extension dirties the high bits, so
            // re-mask the result to `width`.
            let t_w = true_expr.width();
            let f_w = false_expr.width();
            let both_signed = true_expr.expr_context().signed
                && false_expr.expr_context().signed
                && t_w > 0
                && f_w > 0;
            if both_signed && (t_w < *width || f_w < *width) {
                if *width == 0 || *width > 64 || t_w > 64 || f_w > 64 {
                    return None;
                }
                let t = emit_expr_inner(true_expr, true)?;
                let f = emit_expr_inner(false_expr, true)?;
                let sext = |s: &str, w: usize| -> String {
                    if w == 64 {
                        format!("((int64_t)((uint64_t)({})))", s)
                    } else {
                        let shift = 64 - w;
                        format!("(((int64_t)((uint64_t)({}) << {})) >> {})", s, shift, shift)
                    }
                };
                let inner = format!(
                    "(({}) ? ({}) : ({}))",
                    wrap_expect(&c),
                    sext(&t, t_w),
                    sext(&f, f_w)
                );
                if *width < 64 {
                    let mask = (1u64 << *width) - 1;
                    return Some(format!("(((uint64_t)({inner})) & 0x{mask:x}ULL)"));
                }
                return Some(format!("((uint64_t)({inner}))"));
            }
            let t = emit_expr_inner(true_expr, needs_clean)?;
            let f = emit_expr_inner(false_expr, needs_clean)?;
            Some(format!("(({}) ? ({}) : ({}))", wrap_expect(&c), t, f))
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
                if sub_width == 0 || sub_width > 128 {
                    return None;
                }
                let sub_str = emit_expr(sub)?;
                if sub_width > 64 {
                    // Wide (65..128-bit) element: total width > 64 ⇒ `acc` is
                    // __uint128_t.  A full-128-bit shift is UB, so it clears
                    // `acc` (every prior bit moves past bit 127).
                    let masked = if sub_width < 128 {
                        mask_u128(&format!("((__uint128_t)({sub_str}))"), sub_width)
                    } else {
                        format!("((__uint128_t)({sub_str}))")
                    };
                    for _ in 0..*repeat {
                        let shifted = if sub_width >= 128 {
                            "((__uint128_t)0)".to_string()
                        } else {
                            format!("(({acc}) << {sub_width})")
                        };
                        acc = format!("({shifted} | ({masked}))");
                    }
                    continue;
                }
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
            // Falls back to Cranelift for width > 64.
            if let Some(dyn_sel) = dynamic_select {
                // Dynamic bit-select off a dynamically indexed element
                // (`arr[i][j]`): read the FULL element, then extract `window`
                // bits at bit offset clamp(sel_idx)*elem_width.  eval ignores a
                // static `select` when dynamic_select is present — mirror that.
                // Wide (>8-byte) elements stay on Cranelift.
                if *element_native_bytes > 8 || *num_elements == 0 {
                    return None;
                }
                if dyn_sel.elem_width == 0 || dyn_sel.elem_width >= 64 {
                    return None;
                }
                if dyn_sel.window == 0 || dyn_sel.window >= 64 {
                    return None;
                }
                if dyn_sel.num_elements == 0 {
                    return None;
                }
                let cty = native_c_type(*element_native_bytes)?;
                let (buf, base_off) = match base_offset {
                    VarOffset::Ff(o) => ("ff_values", *o),
                    VarOffset::Comb(o) => ("comb_values", *o),
                };
                let idx_str = emit_expr(index_expr)?;
                let sel_str = emit_expr(&dyn_sel.index_expr)?;
                let max_idx = num_elements.saturating_sub(1);
                let max_sel = dyn_sel.num_elements.saturating_sub(1);
                let mask = (1u64 << dyn_sel.window) - 1;
                return Some(format!(
                    "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                        uint64_t _idx = _idx_raw < {maxi} ? _idx_raw : {maxi}; \
                        uint64_t _el = (uint64_t)*((const {ct}*)({b} + {off:#x} + (intptr_t){stride} * (intptr_t)_idx)); \
                        uint64_t _bsel_raw = (uint64_t)({bsel}); \
                        uint64_t _bsel = _bsel_raw < {maxs} ? _bsel_raw : {maxs}; \
                        ((_el >> (_bsel * {ew})) & 0x{mask:x}ULL); }})",
                    idx = idx_str,
                    maxi = max_idx,
                    ct = cty,
                    b = buf,
                    off = base_off,
                    stride = stride,
                    bsel = sel_str,
                    maxs = max_sel,
                    ew = dyn_sel.elem_width,
                    mask = mask,
                ));
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
            // No-select read of a 65..128-bit array element as `__uint128_t`.
            // The >16-byte (>128-bit) element case is handled above; a
            // bit-select on a 65..128-bit element falls through to the narrow
            // path / Cranelift.
            if select.is_none() && *width > 64 && *width <= 128 {
                if *num_elements == 0 {
                    return None;
                }
                let (buf, base_off) = match base_offset {
                    VarOffset::Ff(o) => ("ff_values", *o),
                    VarOffset::Comb(o) => ("comb_values", *o),
                };
                let idx_str = emit_expr(index_expr)?;
                let max_idx = num_elements.saturating_sub(1);
                let load = format!(
                    "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                        uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                        (__uint128_t)*((const veryl_u128_ua*)({b} + {off:#x} + (intptr_t){stride} * (intptr_t)_idx)); }})",
                    idx = idx_str,
                    max = max_idx,
                    b = buf,
                    off = base_off,
                    stride = stride,
                );
                if needs_clean && *width < 128 {
                    return Some(mask_u128(&load, *width));
                }
                return Some(load);
            }
            // Static bit-select on a 65..128-bit array element
            // (element_native_bytes == 16): load the full __uint128_t element
            // and extract [lo..hi].  A field whose top bit sits at/above bit 64
            // can't be read by the ≤64 load below (which reads only the low 8
            // bytes); handle it here.  Result nbits <= 64 in practice.
            if *element_native_bytes == 16
                && *num_elements != 0
                && let Some((hi, lo)) = select
            {
                let nbits = hi.checked_sub(*lo)?.checked_add(1)?;
                if nbits > 128 {
                    return None;
                }
                let (buf, base_off) = match base_offset {
                    VarOffset::Ff(o) => ("ff_values", *o),
                    VarOffset::Comb(o) => ("comb_values", *o),
                };
                let idx_str = emit_expr(index_expr)?;
                let max_idx = num_elements.saturating_sub(1);
                let load = format!(
                    "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                        uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                        (__uint128_t)*((const veryl_u128_ua*)({b} + {off:#x} + (intptr_t){stride} * (intptr_t)_idx)); }})",
                    idx = idx_str,
                    max = max_idx,
                    b = buf,
                    off = base_off,
                    stride = stride,
                );
                let shifted = format!("(((__uint128_t)({load})) >> {lo})");
                if nbits >= 128 {
                    return Some(shifted);
                }
                if nbits > 64 {
                    return Some(mask_u128(&shifted, nbits));
                }
                let mask = if nbits == 64 {
                    u64::MAX
                } else {
                    (1u64 << nbits) - 1
                };
                return Some(format!("((uint64_t)(({shifted}) & 0x{mask:x}ULL))"));
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
        // 0-width loads occur (zero-width sentinels and
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
    // Localized signal: read the C local holding the (width-masked, zero-
    // extended) value.  Only ≤64-bit comb signals are localized, so the local
    // is uint64_t and the requested width is ≤64 → result_ty is uint64_t.
    if matches!(var_offset, VarOffset::Comb(_)) && is_localized(off) {
        return Some(format!(
            "(({rt}){nm})",
            rt = result_ty,
            nm = local_name(off)
        ));
    }
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
            // width=0 occurs (zero-width sentinels and
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
        Value::BigUint(v) => {
            // 65..128-bit constant (num-bigint payload, little-endian u64
            // words).  2-state: the X/Z mask is ignored, mirroring
            // emit_wide_const and the rest of the AOT-C path.  width > 128 is
            // rejected by the guard above.
            if width == 0 {
                return Some("0ULL".to_string());
            }
            let digits = v.payload.to_u64_digits();
            let lo = digits.first().copied().unwrap_or(0);
            let hi = digits.get(1).copied().unwrap_or(0);
            if width <= 64 {
                let masked = if width >= 64 {
                    lo
                } else {
                    lo & ((1u64 << width) - 1)
                };
                return Some(format!("0x{masked:x}ULL"));
            }
            let mut val: u128 = ((hi as u128) << 64) | (lo as u128);
            if width < 128 {
                val &= (1u128 << width) - 1;
            }
            Some(format!(
                "(((__uint128_t)0x{hi:x}ULL << 64) | (__uint128_t)0x{lo:x}ULL)",
                hi = (val >> 64) as u64,
                lo = val as u64,
            ))
        }
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
    use crate::ir::{
        ExpressionContext, ProtoAssignStatement, ProtoDynamicBitSelect, ProtoIfStatement,
        ProtoSystemFunctionCall,
    };
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
        // width=0 Values appear (zero-width sentinels);
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
    fn emit_expr_binary_pow() {
        // 32-bit x ** y: binary-exponentiation loop in native u64 then mask.
        let e = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0x10), 32)),
            op: Op::Pow,
            y: Box::new(const_expr(3, 32)),
            width: 32,
            expr_context: ctx(32, false),
        };
        let s = emit_expr(&e).unwrap();
        // Binary exponentiation: while(exp){ if(exp&1) r*=b; b*=b; exp>>=1; }
        assert!(s.contains("while"));
        assert!(s.contains("*="));
        assert!(s.contains(">>= 1") || s.contains(">>=1"));
        // Result masked to 32 bits.
        assert!(s.contains("0xffffffffULL"));
    }

    #[test]
    fn emit_stmt_assign_comb_dynamic_select_store() {
        // Runtime-indexed field store: dst[idx*4 +: 4] = value.  idx clamps to
        // num_elements-1, then RMW the 4-bit window at bit idx*4.
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x20),
            dst_width: 40,
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(var_expr(VarOffset::Comb(0x8), 8)),
                elem_width: 4,
                window: 4,
                num_elements: 10,
            }),
            rhs_select: None,
            expr: const_expr(0xa, 4),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        assert!(s.contains("comb_values + 0x20"));
        // Clamp to num_elements-1 = 9, runtime shift by idx*elem_width.
        assert!(s.contains("< 9"));
        assert!(s.contains("_sh = _idx * 4"));
        // 4-bit window mask.
        assert!(s.contains("0xfULL"));
        assert!(s.contains("<< _sh"));
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

    fn signed_var_expr(var_offset: VarOffset, width: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset,
            select: None,
            dynamic_select: None,
            width,
            var_full_width: width,
            expr_context: ctx(width, true),
        }
    }

    #[test]
    fn emit_stmt_assign_comb_signext_plain_64() {
        // Signed 32-bit RHS stored into a 64-bit comb dst sign-extends to 64
        // (mirrors value.expand(64, true)): the store must arithmetic-shift so
        // the high 32 bits carry the sign, not zero.
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x30),
            dst_width: 64,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: signed_var_expr(VarOffset::Comb(0x8), 32),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        assert!(s.contains("comb_values + 0x30"));
        // Sign-extend 32 -> 64: shift up by 32, arithmetic shift down by 32.
        assert!(s.contains("(int64_t)"));
        assert!(s.contains("<< 32"));
        assert!(s.contains(">> 32"));
    }

    #[test]
    fn emit_stmt_assign_comb_signext_select_fills_sign() {
        // Signed 8-bit RHS stored into field [15:4] of a 64-bit dst: the value
        // is sign-extended to dst_width BEFORE the field store, so bits above
        // the RHS's width (8) in the 12-bit field carry the sign bit.
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x40),
            dst_width: 64,
            select: Some((15, 4)),
            dynamic_select: None,
            rhs_select: None,
            expr: signed_var_expr(VarOffset::Comb(0x8), 8),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        assert!(s.contains("comb_values + 0x40"));
        // Sign-extend 8 -> 64 before masking the 12-bit field.
        assert!(s.contains("(int64_t)"));
        assert!(s.contains("<< 56"));
        // 12-bit field value mask then position shift by 4.
        assert!(s.contains("0xfffULL"));
        assert!(s.contains("<< 4"));
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
                window: 4,
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
    fn emit_expr_variable_dynamic_select_wide_var_65_128() {
        // 65..128-bit var: dynamic-select loads the __uint128_t value, shifts
        // by idx*elem_width, masks to the window.
        use crate::ir::ProtoDynamicBitSelect;
        let idx = const_expr(0, 4);
        let e = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0),
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(idx),
                elem_width: 4,
                window: 4,
                num_elements: 4,
            }),
            width: 4,
            var_full_width: 96,
            expr_context: ctx(4, false),
        };
        let s = emit_expr(&e).unwrap();
        // idx clamped to num_elements-1 = 3, shift by idx*4, mask to 4 bits.
        assert!(s.contains("< 3"));
        assert!(s.contains("_idx * 4"));
        assert!(s.contains("0xfULL"));
    }

    #[test]
    fn emit_expr_variable_dynamic_select_wide_var_over_128() {
        // >128-bit var: funnel-read a 64-bit window at the runtime bit offset.
        use crate::ir::ProtoDynamicBitSelect;
        let idx = const_expr(0, 4);
        let e = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0),
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(idx),
                elem_width: 8,
                window: 8,
                num_elements: 16,
            }),
            width: 8,
            var_full_width: 256,
            expr_context: ctx(8, false),
        };
        let s = emit_expr(&e).unwrap();
        // Funnel read: word index, sub-word shift, guarded hi read, window mask.
        assert!(s.contains("_bit"));
        assert!(s.contains("veryl_u64_ua"));
        assert!(s.contains("0xffULL"));
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
    fn emit_expr_dynamic_variable_with_dynamic_select() {
        // arr[i][j]: 8-element u32 array at comb[0x200], stride=4, then a
        // 1-bit dynamic select over 32 one-bit lanes of the element.
        let e = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x200),
            stride: 4,
            element_native_bytes: 4,
            index_expr: Box::new(const_expr(2, 4)),
            num_elements: 8,
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(const_expr(5, 8)),
                elem_width: 1,
                window: 1,
                num_elements: 32,
            }),
            width: 1,
            expr_context: ctx(1, false),
        };
        let s = emit_expr(&e).unwrap();
        // Element index clamps to num_elements-1 == 7.
        assert!(s.contains("_idx_raw < 7 ?"));
        assert!(s.contains("comb_values + 0x200"));
        // Bit-select index clamps to dyn_sel.num_elements-1 == 31, then
        // shifts by elem_width and masks the 1-bit window.
        assert!(s.contains("_bsel_raw < 31 ?"));
        assert!(s.contains("(_el >> (_bsel * 1)) & 0x1ULL"));
    }

    #[test]
    fn emit_expr_dynamic_variable_with_dynamic_select_wide_elem_rejects() {
        // >8-byte elements stay on Cranelift.
        let e = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x200),
            stride: 16,
            element_native_bytes: 16,
            index_expr: Box::new(const_expr(0, 4)),
            num_elements: 4,
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(const_expr(0, 8)),
                elem_width: 1,
                window: 1,
                num_elements: 128,
            }),
            width: 1,
            expr_context: ctx(1, false),
        };
        assert!(emit_expr(&e).is_none());
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

    // ---- 65..128-bit `__uint128_t` scalar coverage ----
    // One __uint128_t per value; the >128-bit wide-pointer path is separate.

    #[test]
    fn emit_expr_unary_reduction_65_to_128() {
        let red = |op| ProtoExpression::Unary {
            op,
            x: Box::new(var_expr(VarOffset::Comb(0), 96)),
            width: 1,
            expr_context: ctx(1, false),
        };
        let s = emit_expr(&red(Op::BitOr)).unwrap();
        assert!(s.contains("__uint128_t _m"));
        assert!(s.contains("_m != 0"));
        // 96-bit all-ones split constant: hi word is 32 bits (0xffffffff).
        let s = emit_expr(&red(Op::BitAnd)).unwrap();
        assert!(s.contains("_m =="));
        assert!(s.contains("0xffffffffULL"));
        let s = emit_expr(&red(Op::BitXor)).unwrap();
        assert!(s.contains("__builtin_parityll"));
        assert!(s.contains("_m >> 64"));
    }

    #[test]
    fn emit_value_biguint_65_to_128() {
        use num_bigint::BigUint;
        use veryl_analyzer::value::ValueBigUint;
        let val: u128 = 0x1234_5678_9abc_def0_fedc_ba98_7654_3210;
        let v = Value::BigUint(ValueBigUint::new_biguint(BigUint::from(val), 128, false));
        let s = emit_value(&v, 128).unwrap();
        assert!(s.contains("__uint128_t"));
        assert!(s.contains("0x123456789abcdef0ULL"));
        assert!(s.contains("0xfedcba9876543210ULL"));
        assert!(s.contains("<< 64"));
        // 72-bit mask: hi word keeps only its low 8 bits (0xf0).
        let s = emit_value(&v, 72).unwrap();
        assert!(s.contains("0xf0ULL << 64"));
        assert!(s.contains("0xfedcba9876543210ULL"));
    }

    #[test]
    fn emit_expr_arith_shift_right_65_to_128() {
        let ashr = |signed| ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0), 96)),
            op: Op::ArithShiftR,
            y: Box::new(const_expr(4, 32)),
            width: 96,
            expr_context: ctx(96, signed),
        };
        // sign-extend 96→128 shifts by 128-96=32; count clamps to width-1=95.
        let s = emit_expr(&ashr(true)).unwrap();
        assert!(s.contains("__int128_t"));
        assert!(s.contains("<< 32"));
        assert!(s.contains(">> 32"));
        assert!(s.contains(">= 96 ? 95"));
        let s = emit_expr(&ashr(false)).unwrap();
        assert!(s.contains(">= 96 ? (__uint128_t)0"));
    }

    #[test]
    fn emit_expr_dynamic_variable_128bit_element() {
        let elem = |width| ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x300),
            stride: 16,
            element_native_bytes: 16,
            index_expr: Box::new(const_expr(5, 8)),
            num_elements: 32,
            select: None,
            dynamic_select: None,
            width,
            expr_context: ctx(width, false),
        };
        let s = emit_expr(&elem(128)).unwrap();
        assert!(s.contains("veryl_u128_ua"));
        assert!(s.contains("_idx_raw < 31 ?"));
        assert!(s.contains("comb_values + 0x300"));
        assert!(s.contains("(intptr_t)16 * (intptr_t)_idx"));
        // width < 128 masks to the declared width.
        let s = emit_expr(&elem(100)).unwrap();
        assert!(s.contains("& (((__uint128_t)"));
    }

    #[test]
    fn emit_expr_variable_select_65_to_128_from_narrow_var() {
        let e = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x10),
            select: Some((103, 8)),
            dynamic_select: None,
            width: 96,
            var_full_width: 128,
            expr_context: ctx(96, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("veryl_u128_ua"));
        assert!(s.contains(">> 8"));
        assert!(s.contains("& (((__uint128_t)"));
    }

    #[test]
    fn emit_expr_variable_select_65_to_128_from_wide_var() {
        // >128-bit var → funnel-shift `emit_wide_select_read_wide_at`.
        let e = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x20),
            select: Some((200, 100)),
            dynamic_select: None,
            width: 101,
            var_full_width: 256,
            expr_context: ctx(101, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("veryl_u64_ua"));
        // lo=100 → bit 36, window straddles into word 3.
        assert!(s.contains(">> 36"));
        assert!(s.contains("[3]"));
    }

    #[test]
    fn emit_wide_select_read_wide_at_funnel_cases() {
        // word-aligned (lo=128 → word 2): two words, no third.
        let s = emit_wide_select_read_wide_at("comb_values + 0x10", 128, 100);
        assert!(s.contains("veryl_u64_ua"));
        assert!(s.contains("[2]"));
        assert!(s.contains("[3]"));
        assert!(s.contains("<< 64"));
        assert!(!s.contains("[4]"));
        // unaligned, 2 words (bit+nbits = 110 ≤ 128).
        let s = emit_wide_select_read_wide_at("comb_values + 0x0", 10, 100);
        assert!(s.contains(">> 10"));
        assert!(s.contains("<< 54")); // 64 - 10
        assert!(!s.contains("[2]"));
        // unaligned, third word (bit+nbits = 140 > 128).
        let s = emit_wide_select_read_wide_at("comb_values + 0x0", 40, 100);
        assert!(s.contains(">> 40"));
        assert!(s.contains("<< 24")); // 64 - 40
        assert!(s.contains("<< 88")); // 128 - 40
        assert!(s.contains("[2]"));
    }

    #[test]
    fn emit_expr_concatenation_wide_element_65_to_128() {
        // 96-bit element exercises the wide (sub_width > 64) arm.
        let a = const_expr(0xa, 8);
        let b = var_expr(VarOffset::Comb(0), 96);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(a), 1, 8), (Box::new(b), 1, 96)],
            width: 104,
            expr_context: ctx(104, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("__uint128_t"));
        assert!(s.contains("<< 96"));
        assert!(s.contains("comb_values + 0x0"));
    }

    #[test]
    fn emit_stmt_wide_bit_select_store_65_to_128() {
        // lo ≥ 64 is unreachable by the single-u64 RMW path → __uint128_t branch.
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(0x40),
            dst_width: 128,
            select: Some((71, 64)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xab, 8),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        let s = emit_stmt(&ProtoStatement::Assign(a)).unwrap();
        assert!(s.contains("veryl_u128_ua"));
        assert!(s.contains("__uint128_t _o"));
        assert!(s.contains("_v << 64"));
        assert!(s.contains("comb_values + 0x40"));
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
    fn emit_stmt_assign_dynamic_comb_wide_65_128() {
        // Regression: a 65-128-bit full-element dynamic store must emit, not bail.
        use crate::ir::ProtoAssignDynamicStatement;
        let a = ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(0x200),
            dst_stride: 16,
            dst_num_elements: 8,
            dst_index_expr: const_expr(3, 4),
            dst_width: 96,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0x1234, 96),
            dst_ff_current_base_offset: 0,
        };
        let s = emit_stmt(&ProtoStatement::AssignDynamic(a)).unwrap();
        assert!(s.contains("_idx_raw"));
        assert!(s.contains("_idx_raw < 7 ?"));
        assert!(s.contains("comb_values + 0x200"));
        assert!(s.contains("veryl_u128_ua"));
        // 96-bit mask: low 64 bits all ones, high 32 bits set.
        assert!(s.contains("0xffffffffULL << 64"));
    }

    #[test]
    fn emit_stmt_assign_dynamic_comb_wide_65_128_field_le64() {
        // Regression: newly-reachable ≤64-bit bit-select into a 65-128-bit element.
        use crate::ir::ProtoAssignDynamicStatement;
        let a = ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(0x200),
            dst_stride: 16,
            dst_num_elements: 8,
            dst_index_expr: const_expr(3, 4),
            dst_width: 96,
            select: Some((79, 64)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xabcd, 16),
            dst_ff_current_base_offset: 0,
        };
        let s = emit_stmt(&ProtoStatement::AssignDynamic(a)).unwrap();
        assert!(s.contains("_idx_raw"));
        assert!(s.contains("_pa"));
        assert!(s.contains("veryl_u64_ua"));
    }

    #[test]
    fn emit_stmt_assign_dynamic_comb_wide_65_128_field_gt64() {
        // Regression: newly-reachable >64-bit bit-select into a 65-128-bit element.
        use crate::ir::ProtoAssignDynamicStatement;
        let a = ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(0x200),
            dst_stride: 16,
            dst_num_elements: 8,
            dst_index_expr: const_expr(3, 4),
            dst_width: 119,
            select: Some((110, 20)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0x5, 91),
            dst_ff_current_base_offset: 0,
        };
        let s = emit_stmt(&ProtoStatement::AssignDynamic(a)).unwrap();
        assert!(s.contains("_idx_raw"));
        assert!(s.contains("_pa"));
        assert!(s.contains("vw_fill_ones"));
        assert!(s.contains("vw_bor"));
        assert!(s.contains("vw_apply_mask"));
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
    fn emit_stmt_compiled_block_emits_original_offsets_verbatim() {
        use crate::ir::CompiledBlockStatement;
        // original_stmts already hold actual offsets, so the cc inline path must
        // emit them verbatim and NOT re-add ff/comb_delta_bytes (a Cranelift-only
        // relocation hint); re-adding double-counts the delta.
        let inner = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x110),
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
            // Present (a Cranelift relocation hint) but must be ignored by cc.
            comb_delta_bytes: 0x100,
            input_offsets: vec![],
            output_offsets: vec![],
            ff_canonical_offsets: vec![],
            stmt_deps: vec![],
            original_stmts: vec![inner],
        };
        let s = emit_stmt(&ProtoStatement::CompiledBlock(cb)).unwrap();
        assert!(s.contains("comb_values + 0x110")); // actual offset, verbatim
        assert!(!s.contains("comb_values + 0x210")); // delta must NOT be re-added
    }

    fn bogus_artifact() -> Arc<ChunkArtifact> {
        // Never actually called — emit_stmt for CompiledBlock bypasses
        // the artifact entirely.  We just need a valid handle for the
        // struct field.
        unsafe extern "system" fn stub(_: *const u8, _: *const u8, _: *mut u8, _: isize) {}
        Arc::new(ChunkArtifact {
            func: stub,
            keepalive: None,
            content_fp: None,
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
        assert!(s.contains("_lo = 0ULL, _hi = 8ULL"));
        assert!(s.contains("uint64_t _it = _lo"));
        assert!(s.contains("_it < _hi"));
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
        assert!(s.contains("_hi = 8ULL"));
    }

    #[test]
    fn emit_stmt_for_dynamic_bound_forward() {
        // A dynamic end bound is now covered: the bound expression is hoisted
        // to `_hi`, evaluated once before the loop.
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
        let s = emit_stmt(&ProtoStatement::For(for_stmt)).unwrap();
        assert!(s.contains("_lo = 0ULL"));
        assert!(s.contains("uint64_t _it = _lo"));
        assert!(s.contains("_it < _hi"));
        assert!(s.contains("_it += 1ULL"));
    }

    #[test]
    fn emit_stmt_for_reverse() {
        // Reverse: signed loop var, init hi-1, `>= _lo` guard, decrementing.
        let for_stmt = ProtoForStatement {
            var_offset: VarOffset::Comb(0),
            var_width: 32,
            var_native_bytes: 4,
            var_signed: false,
            range: ProtoForRange::Reverse {
                start: ProtoForBound::Const(0),
                end: ProtoForBound::Const(8),
                inclusive: false,
                step: 1,
            },
            body: vec![],
        };
        let s = emit_stmt(&ProtoStatement::For(for_stmt)).unwrap();
        assert!(s.contains("int64_t _it = _hi - 1"));
        assert!(s.contains("_it >= _lo"));
        assert!(s.contains("_it -= 1ULL"));
    }

    #[test]
    fn emit_stmt_for_stepped_rejects() {
        let for_stmt = ProtoForStatement {
            var_offset: VarOffset::Comb(0),
            var_width: 32,
            var_native_bytes: 4,
            var_signed: false,
            range: ProtoForRange::Stepped {
                start: ProtoForBound::Const(1),
                end: ProtoForBound::Const(64),
                inclusive: false,
                step: 2,
                op: veryl_analyzer::ir::Op::Mul,
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
    fn compile_for_test(cache_dir: &Path, src: &str, what: &str) -> Option<EmittedModule> {
        match compile_source_in(cache_dir, src) {
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

        // Per-test cache dir passed explicitly (no VERYL_AOT_CACHE_DIR env
        // mutation): the env var is process-global, so set/remove from a
        // concurrently-running test would point this compile at the wrong
        // dir — and a peer test's remove_dir_all could delete the dir mid-cc
        // (observed as `ld: open() failed, errno=2` flakes in CI).
        let tmp = std::env::temp_dir().join(format!("veryl_aot_dv_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "emit_function_dynamic_variable_compiles")
        else {
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
                0,
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
                0,
            );
        }
        let written = u32::from_le_bytes(comb[20..24].try_into().unwrap());
        assert_eq!(
            written, 0xdddd,
            "out-of-range idx should clamp to last element"
        );

        let _ = std::fs::remove_dir_all(&tmp);
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
            void veryl_aot_eval(uint8_t *ff, uint8_t *comb, uint64_t *log, intptr_t ff_delta) {\n\
                (void)ff; (void)log; (void)ff_delta;\n\
                *(uint32_t*)(comb + 0) = 0xdeadbeef;\n\
            }\n";
        // Per-test cache dir passed explicitly so we don't pollute the
        // user's shared cache and the test stays hermetic.  Passing it as
        // an argument (rather than via the process-global VERYL_AOT_CACHE_DIR
        // env var) avoids racing every other AOT-C test that compiles
        // concurrently — libtest runs tests multi-threaded by default.
        let tmp = std::env::temp_dir().join(format!("veryl_aot_test_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, src, "compile_source_round_trip") else {
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
                0,
            );
        }
        let written = u32::from_le_bytes(comb[0..4].try_into().unwrap());
        assert_eq!(written, 0xdeadbeef, "comb[0..4] should be 0xdeadbeef");
        // Best-effort cleanup; ignore failures.
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- Chunk-local localization (compute_localize_sets) ---
    // A wrongly-localized comb offset is a silent miscompile, so each
    // disqualification path gets a direct unit test rather than relying only on
    // the slow, opt-in validate dual-run.

    fn comb_assign(
        off: isize,
        width: usize,
        select: Option<(usize, usize)>,
        rhs: ProtoExpression,
    ) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: width,
            select,
            dynamic_select: None,
            rhs_select: None,
            expr: rhs,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        })
    }

    fn localize_sets(
        chunks: &[&[ProtoStatement]],
        blocklist: &[isize],
        ranges: &[(isize, usize, isize)],
    ) -> Vec<std::collections::HashSet<isize>> {
        let bl: std::collections::HashSet<isize> = blocklist.iter().copied().collect();
        compute_localize_sets(chunks, &bl, ranges).0
    }

    #[test]
    fn localize_happy_path() {
        // O is written by a clean top-level scalar Assign and read only in the
        // same chunk → it is safe to keep in a C local.
        let c0 = vec![
            comb_assign(0x10, 32, None, const_expr(0, 32)),
            comb_assign(0x20, 32, None, var_expr(VarOffset::Comb(0x10), 32)),
        ];
        let sets = localize_sets(&[&c0], &[], &[]);
        assert!(
            sets[0].contains(&0x10),
            "single-chunk scalar should localize"
        );
    }

    #[test]
    fn localize_skips_conditional_write() {
        // A write inside an `if` is conditional: the persisted comb_values byte
        // carries the latch/hold value when the branch is not taken, so it must
        // never become a chunk-local.
        let c0 = vec![ProtoStatement::If(ProtoIfStatement {
            cond: Some(const_expr(1, 1)),
            true_side: vec![comb_assign(0x10, 32, None, const_expr(0, 32))],
            false_side: vec![],
        })];
        let sets = localize_sets(&[&c0], &[], &[]);
        assert!(
            !sets[0].contains(&0x10),
            "conditional write must not localize"
        );
    }

    #[test]
    fn localize_skips_cross_chunk_read() {
        // Written in chunk 0 but read in chunk 1: a chunk-local in chunk 0 is
        // invisible to chunk 1's noinline function, which reads comb_values.
        let c0 = vec![comb_assign(0x10, 32, None, const_expr(0, 32))];
        let c1 = vec![comb_assign(
            0x20,
            32,
            None,
            var_expr(VarOffset::Comb(0x10), 32),
        )];
        let sets = localize_sets(&[&c0, &c1], &[], &[]);
        assert!(
            !sets[0].contains(&0x10),
            "cross-chunk read must not localize"
        );
    }

    #[test]
    fn localize_skips_blocklisted() {
        // Blocklisted = an event (or a port / user-var / clock) reads it from
        // comb_values across the comb→event boundary → load-bearing, keep it.
        let c0 = vec![
            comb_assign(0x10, 32, None, const_expr(0, 32)),
            comb_assign(0x20, 32, None, var_expr(VarOffset::Comb(0x10), 32)),
        ];
        let sets = localize_sets(&[&c0], &[0x10], &[]);
        assert!(
            !sets[0].contains(&0x10),
            "blocklisted offset must not localize"
        );
    }

    #[test]
    fn localize_skips_dynamic_array_range() {
        // 0x108 is element 1 of a runtime-indexed array (base 0x100, 4 elems,
        // stride 8): a dynamic index elsewhere may read it, so exclude it.
        let c0 = vec![
            comb_assign(0x108, 32, None, const_expr(0, 32)),
            comb_assign(0x200, 32, None, var_expr(VarOffset::Comb(0x108), 32)),
        ];
        let sets = localize_sets(&[&c0], &[], &[(0x100, 4, 8)]);
        assert!(
            !sets[0].contains(&0x108),
            "offset inside a dynamic array range must not localize"
        );
    }

    #[test]
    fn localize_skips_partial_write() {
        // A bit-select write only updates part of the word; the rest comes from
        // the persisted comb_values byte → not a full-scalar candidate.
        let c0 = vec![comb_assign(0x10, 32, Some((3, 0)), const_expr(0, 4))];
        let sets = localize_sets(&[&c0], &[], &[]);
        assert!(
            !sets[0].contains(&0x10),
            "partial (select) write must not localize"
        );
    }

    #[test]
    fn localize_skips_wide_write() {
        // >64-bit writes go through the wide path, not a uint64_t local.
        let c0 = vec![comb_assign(0x10, 128, None, const_expr(0, 128))];
        let sets = localize_sets(&[&c0], &[], &[]);
        assert!(
            !sets[0].contains(&0x10),
            "wide (>64-bit) write must not localize"
        );
    }

    #[test]
    fn localize_skips_multi_chunk_write() {
        // The same offset written in two chunks: neither chunk's local can hold
        // the cross-chunk value, so it must stay in comb_values.
        let c0 = vec![comb_assign(0x10, 32, None, const_expr(0, 32))];
        let c1 = vec![comb_assign(0x10, 32, None, const_expr(1, 32))];
        let sets = localize_sets(&[&c0, &c1], &[], &[]);
        assert!(
            !sets[0].contains(&0x10),
            "multi-chunk write must not localize"
        );
        assert!(
            !sets[1].contains(&0x10),
            "multi-chunk write must not localize"
        );
    }

    #[test]
    fn wrap_expect_hint_forms() {
        assert_eq!(
            wrap_expect_hint("x & 1", ExpectHint::False),
            "__builtin_expect((x & 1) != 0, 0)"
        );
        assert_eq!(
            wrap_expect_hint("x & 1", ExpectHint::True),
            "__builtin_expect((x & 1) != 0, 1)"
        );
        assert_eq!(wrap_expect_hint("x & 1", ExpectHint::Off), "x & 1");
    }
}
