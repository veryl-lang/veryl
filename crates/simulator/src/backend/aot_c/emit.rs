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
use crate::{HashMap, HashSet};
use std::cell::RefCell;
use std::ffi::c_void;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
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
/* The low dnb bytes of (a >> amount), where a holds anb bytes: a wide
   bit-select needs only its own result window, not the whole shifted
   source. */
static inline void vw_lshr_win(uint8_t* d,const uint8_t* a,uint64_t amount,uint32_t dnb,uint32_t anb){
  unsigned dn=dnb/8, an=anb/8; unsigned ws=(unsigned)(amount/64); uint32_t bs=(uint32_t)(amount%64);
  for(unsigned i=0;i<dn;i++){ unsigned si=i+ws;
    uint64_t lo = si<an ? VW_RD(a,si) : 0;
    uint64_t hi = si+1<an ? VW_RD(a,si+1) : 0;
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
/// 64-bit word count for a `native_bytes` size class — the length of the
/// `uint64_t _wN[]` scratch that holds a value of that size.  Must round UP:
/// `native_bytes` returns 4 for widths <= 32, and a truncating `/ 8` would
/// declare a zero-length array whose word-0 store is out of bounds.
fn wide_words(nb: usize) -> usize {
    nb.div_ceil(8)
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
// Chunk-local comb intermediate localization (on by default;
// `VERYL_AOT_C_LOCALIZE=0` or `force_disable_localize` opts out).  A comb
// scalar written and read only within its emit chunk is kept in a C local
// instead of round-tripping `comb_values` (gcc can't drop the store —
// escaping restrict param — but the emitter's global read-set can).
// Soundness: localize only a signal (a) written by one top-level unconditional
// full-width scalar (≤64-bit) Assign, (b) read only in that chunk, (c) not
// blocklisted (event-read / array-range / partial-write / port).  Blocklist
// built in `module.rs`.

/// Set by [`force_disable_localize`]; latches on and is never cleared, so
/// localization can only ever be turned off, never back on.
static LOCALIZE_FORCED_OFF: AtomicBool = AtomicBool::new(false);

/// Whether chunk-local localization runs: on unless `VERYL_AOT_C_LOCALIZE=0`
/// or a caller turned it off for the process.
pub fn localize_enabled() -> bool {
    if LOCALIZE_FORCED_OFF.load(Ordering::Relaxed) {
        return false;
    }
    std::env::var("VERYL_AOT_C_LOCALIZE").as_deref() != Ok("0")
}

/// Latch the process-wide off switch; `super::force_disable_localize` is the
/// public entry point and carries the rationale.
pub fn force_disable_localize() {
    LOCALIZE_FORCED_OFF.store(true, Ordering::Relaxed);
}

thread_local! {
    /// Comb offsets the caller marked unsafe to localize (read outside the
    /// comb function / dynamically / partial-written / port-visible).
    static LOCALIZE_BLOCKLIST: RefCell<HashSet<isize>> =
        RefCell::new(HashSet::default());
    /// Comb offsets localized in the chunk currently being emitted.
    static CURRENT_LOCAL: RefCell<HashSet<isize>> =
        RefCell::new(HashSet::default());
    /// Runtime-indexed comb array ranges (base, num_elements, stride) — a
    /// candidate offset inside any of these is excluded (a constant-indexed
    /// element could be read dynamically by an event / another statement).
    static LOCALIZE_RANGES: RefCell<Vec<(isize, usize, isize)>> =
        const { RefCell::new(Vec::new()) };
    /// Byte ranges (offset, native_bytes) localized in the just-emitted comb —
    /// these comb_values bytes are intentionally left stale, so the validate
    /// dual-run must skip them.  Read by `prepare_comb` right after emit.
    static LAST_LOCALIZED_BYTES: RefCell<Vec<(isize, usize)>> =
        const { RefCell::new(Vec::new()) };
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
pub fn set_localize_blocklist(set: HashSet<isize>, ranges: Vec<(isize, usize, isize)>) {
    LOCALIZE_BLOCKLIST.with(|b| *b.borrow_mut() = set);
    LOCALIZE_RANGES.with(|r| *r.borrow_mut() = ranges);
    LOCALIZE_ARMED.with(|a| a.set(true));
}

pub fn clear_localize_blocklist() {
    LOCALIZE_BLOCKLIST.with(|b| b.borrow_mut().clear());
    LOCALIZE_RANGES.with(|r| r.borrow_mut().clear());
    LOCALIZE_ARMED.with(|a| a.set(false));
}

thread_local! {
    /// Comb offsets written by EVENT statements (misclassified-FF: ICG
    /// enables and other event-written comb).  Reading or writing one
    /// disqualifies a statement from the const cone; the split only arms
    /// once this sound set has been installed.
    static CONST_UNSAFE_COMB: RefCell<HashSet<isize>> = RefCell::new(HashSet::default());
    static CONST_SKIP_ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Install the event-written comb offsets and arm the const-cone split for
/// the next comb emit.  Paired with `clear_const_unsafe`.
pub fn set_const_unsafe(set: HashSet<isize>) {
    CONST_UNSAFE_COMB.with(|b| *b.borrow_mut() = set);
    CONST_SKIP_ARMED.with(|a| a.set(true));
}

pub fn clear_const_unsafe() {
    CONST_UNSAFE_COMB.with(|b| b.borrow_mut().clear());
    CONST_SKIP_ARMED.with(|a| a.set(false));
}

#[inline]
fn const_skip_armed() -> bool {
    CONST_SKIP_ARMED.with(|a| a.get())
        && std::env::var("VERYL_AOT_C_CONST_SKIP").as_deref() != Ok("0")
}

thread_local! {
    /// Cone-gate segments for the next whole-comb emit.
    /// Non-empty: the emitter keeps the caller's statement order (no const
    /// split, no field gather), forces chunk boundaries at the segment edges,
    /// and guards the dispatcher's calls with the segment compares.
    static CONE_SEGMENTS: std::cell::RefCell<Vec<crate::ir::opt::cone_gate::ConeSegment>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Install the cone-gate segments for the next comb emit.  Paired with
/// `clear_cone_segments`.
pub fn set_cone_segments(segs: Vec<crate::ir::opt::cone_gate::ConeSegment>) {
    CONE_SEGMENTS.with(|s| *s.borrow_mut() = segs);
}

pub fn clear_cone_segments() {
    CONE_SEGMENTS.with(|s| s.borrow_mut().clear());
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
    bad: HashSet<isize>,
    /// off -> Some(chunk) while read in one chunk only; None if read in 2+.
    read_chunk: HashMap<isize, Option<usize>>,
    /// Offsets read before their (only) write in schedule order: the reader
    /// consumes the PREVIOUS settle's value from `comb_values` (a certified
    /// backward edge), which a fresh zero-initialized local cannot supply.
    read_before_write: HashSet<isize>,
    /// off -> native storage byte width (for the validate skip-range).
    width: HashMap<isize, usize>,
}

impl LocalAnalysis {
    fn note_read(&mut self, off: isize, i: usize) {
        if !self.write_chunk.contains_key(&off) {
            self.read_before_write.insert(off);
        }
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
    blocklist: &HashSet<isize>,
    ranges: &[(isize, usize, isize)],
) -> (Vec<HashSet<isize>>, HashMap<isize, usize>) {
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
    let mut sets: Vec<HashSet<isize>> = vec![HashSet::default(); chunks.len()];
    for (off, wc) in &a.write_chunk {
        let Some(i) = wc else { continue };
        if a.bad.contains(off)
            || a.read_before_write.contains(off)
            || blocklist.contains(off)
            || in_range(*off)
        {
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
            let count = wide_words(native_bytes(target));
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
    let nw = wide_words(nb);
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
            // Dynamic element read with a >128-bit window.  A ≤128-bit
            // window is a scalar (emit_expr's funnel paths) and a combined
            // static select stays on Cranelift, so only this arm is left.
            if let Some(dyn_sel) = dynamic_select {
                if select.is_some()
                    || dyn_sel.window <= 128
                    || dyn_sel.elem_width == 0
                    || dyn_sel.num_elements == 0
                {
                    return None;
                }
                let idx = emit_expr(&dyn_sel.index_expr)?;
                let max_idx = dyn_sel.num_elements - 1;
                let src_nb = native_bytes(*var_full_width);
                let res_nb = native_bytes(dyn_sel.window);
                let res_nw = wide_words(res_nb);
                let t = next_wide_tmp();
                pre.push_str(&format!(
                    "uint64_t _w{t}[{res_nw}]; \
                     {{ uint64_t _di_raw = (uint64_t)({idx}); \
                        uint64_t _di = _di_raw < {max_idx}ull ? _di_raw : {max_idx}ull; \
                        vw_lshr_win((uint8_t*)_w{t}, (const uint8_t*)({buf} + {off:#x}), _di * {ew}ull, {res_nb}u, {src_nb}u); }} \
                     vw_apply_mask((uint8_t*)_w{t}, (const uint8_t*)0, {mask}u); ",
                    ew = dyn_sel.elem_width,
                    mask = wpack(res_nb, dyn_sel.window),
                ));
                return Some(WideRef {
                    addr: format!("((uint8_t*)_w{t})"),
                    nb: native_bytes(dyn_sel.window),
                    width: dyn_sel.window,
                });
            }
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
                let res_nb = native_bytes(nbits);
                let res_nw = wide_words(res_nb);
                let t = next_wide_tmp();
                pre.push_str(&format!(
                    "uint64_t _w{t}[{res_nw}]; \
                     vw_lshr_win((uint8_t*)_w{t}, (const uint8_t*)({buf} + {off:#x}), {lo}ull, {res_nb}u, {src_nb}u); \
                     vw_apply_mask((uint8_t*)_w{t}, (const uint8_t*)0, {mask}u); ",
                    mask = wpack(res_nb, nbits),
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
    let tnw = wide_words(target_nb);
    // A wide-pointer NODE whose wide emit has no arm may still be scalar-
    // emittable when its RESULT is ≤128 bits (e.g. a dynamic select on a
    // >128-bit var, which reads a 64..128-bit element into a register):
    // fall through to the scalar promotion below instead of bailing.
    if expr.builds_wide_pointer()
        && let Some(r) = emit_wide_expr(expr, pre)
    {
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
/// Scalar (≤64-bit) view of a wide-pointer sub-expression via a GCC
/// statement expression — for concat elements whose VALUE is narrow but
/// whose emit routes through the wide machinery (e.g. a narrow dynamic
/// select on a >128-bit var).
fn emit_scalar_sub_via_wide(sub: &ProtoExpression) -> Option<String> {
    if !sub.builds_wide_pointer() || sub.width() == 0 || sub.width() > 64 {
        return None;
    }
    let mut pre = String::new();
    let r = emit_wide_operand(sub, native_bytes(sub.width()).max(8), &mut pre)?;
    Some(format!(
        "({{ {pre}(uint64_t)VW_RD({addr}, 0); }})",
        addr = r.addr
    ))
}

/// Extract `value.select(rhs_hi, rhs_lo)` from a wide RHS.  The scratch
/// physically spans the SOURCE size class while the returned `nb`/`width`
/// describe the field.  Mirrors AssignStatement::eval_step's `value.select`.
fn emit_wide_rhs_field(
    expr: &ProtoExpression,
    rhs_hi: usize,
    rhs_lo: usize,
    pre: &mut String,
) -> Option<WideRef> {
    let nbits = rhs_hi.checked_sub(rhs_lo)?.checked_add(1)?;
    let src_w = expr.width();
    if src_w == 0 {
        return None;
    }
    let src_nb = native_bytes(src_w);
    let src_nw = wide_words(src_nb);
    let r = emit_wide_operand(expr, src_nb, pre)?;
    let fld = next_wide_tmp();
    pre.push_str(&format!(
        "uint64_t _w{fld}[{src_nw}]; \
         vw_copy((uint8_t*)_w{fld}, {src}, {src_nb}u); \
         vw_apply_mask((uint8_t*)_w{fld}, (const uint8_t*)0, {pks}u); \
         vw_lshr((uint8_t*)_w{fld}, (const uint8_t*)_w{fld}, {rhs_lo}ull, {src_nb}u); \
         vw_apply_mask((uint8_t*)_w{fld}, (const uint8_t*)0, {pkf}u); ",
        src = r.addr,
        pks = wpack(src_nb, src_w),
        pkf = wpack(src_nb, nbits),
    ));
    Some(WideRef {
        addr: format!("((uint8_t*)_w{fld})"),
        // vw_* helpers move whole 8-byte words (vw_copy with nb=4 copies
        // NOTHING), so never advertise a sub-word size class.
        nb: native_bytes(nbits).max(8).min(src_nb),
        width: nbits,
    })
}

/// Full wide RMW of a dst bit-select (2-state):
///   new = ((old & ~rangemask) | ((src << lo) & rangemask)) & widthmask
/// where rangemask = fill_ones(nbits) << lo and widthmask = the
/// `vw_apply_mask(dst_width)` clamp.  `lo`, `nbits` and `dst_width` are
/// static, so both masks fold to per-word immediates and only the words the
/// field (or the width clamp) actually touches get a statement.  Words are
/// written HIGH to LOW: a self-aliasing store (`x[hi:lo] = f(x)` routed here
/// with `src` pointing at the destination) reads source words at index
/// `k - lo/64 <= k`, so descending order only ever reads not-yet-written
/// words, preserving the read-everything-then-copy behaviour of the
/// temporary form.  `src` must span `nb` bytes.
#[allow(clippy::too_many_arguments)]
fn emit_wide_select_rmw_store(
    src: &str,
    pre: String,
    dst: &str,
    nw: usize,
    lo: usize,
    nbits: usize,
    dst_width: usize,
) -> String {
    let ws = lo / 64;
    let bs = lo % 64;
    // rangemask word k: bits of [lo, lo+nbits-1] falling in [64k, 64k+63].
    let range_mask = |k: usize| -> u64 {
        let word_lo = 64 * k;
        let f_lo = lo.max(word_lo);
        let f_hi = (lo + nbits - 1).min(word_lo + 63);
        if f_lo > f_hi {
            return 0;
        }
        let n = f_hi - f_lo + 1;
        let base = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
        base << (f_lo - word_lo)
    };
    // vw_apply_mask(dst_width) word k — width 0 means "no clamp" there.
    let clamp_mask = |k: usize| -> u64 {
        if dst_width == 0 {
            return u64::MAX;
        }
        let word_lo = 64 * k;
        if dst_width >= word_lo + 64 {
            u64::MAX
        } else if dst_width <= word_lo {
            0
        } else {
            (1u64 << (dst_width - word_lo)) - 1
        }
    };
    let t = next_wide_tmp();
    let mut body = String::new();
    for k in (0..nw).rev() {
        let rm = range_mask(k);
        let wm = clamp_mask(k);
        let keep = !rm & wm;
        let em = rm & wm;
        if em == 0 {
            // Untouched by the field; the width clamp may still bite.
            if keep != u64::MAX {
                if keep == 0 {
                    body.push_str(&format!("_d{t}[{k}] = 0; "));
                } else {
                    body.push_str(&format!("_d{t}[{k}] &= {keep:#x}ULL; "));
                }
            }
            continue;
        }
        // (src << lo) word k.  rm != 0 implies k >= ws, so sk is in range.
        let sk = k - ws;
        let sexpr = if bs == 0 {
            format!("_s{t}[{sk}]")
        } else if sk > 0 {
            format!(
                "((_s{t}[{sk}] << {bs}) | (_s{t}[{prev}] >> {rsh}))",
                prev = sk - 1,
                rsh = 64 - bs,
            )
        } else {
            format!("(_s{t}[{sk}] << {bs})")
        };
        if keep == 0 {
            body.push_str(&format!("_d{t}[{k}] = {sexpr} & {em:#x}ULL; "));
        } else {
            body.push_str(&format!(
                "_d{t}[{k}] = (_d{t}[{k}] & {keep:#x}ULL) | ({sexpr} & {em:#x}ULL); "
            ));
        }
    }
    format!(
        "{{ {pre}const veryl_u64_ua* _s{t} = (const veryl_u64_ua*)({src}); \
            veryl_u64_ua* _d{t} = (veryl_u64_ua*)({dst}); {body}}}"
    )
}

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
        let tnw = wide_words(target_nb);
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
    let nw = wide_words(op_nb);
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
    let nw = wide_words(nb);
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

/// Fallback for a ≤128-bit expression the scalar emitter declines because an
/// INTERMEDIATE node is wider than 128 bits (e.g. `concat(184-bit) & mask`).
/// Wide buffers are invariantly masked to their width, so the loaded scalar
/// is clean regardless of `needs_clean`.
fn emit_scalar_via_wide(e: &ProtoExpression) -> Option<String> {
    let w = e.width();
    if w == 0 || w > 128 {
        return None;
    }
    let nb = native_bytes(w);
    let mut pre = String::new();
    let r = emit_wide_operand(e, nb, &mut pre)?;
    Some(if nb <= 8 {
        format!(
            "({{ {pre}(uint64_t)*((const veryl_u64_ua*)({addr})); }})",
            addr = r.addr
        )
    } else {
        format!(
            "({{ {pre}(__uint128_t)*((const veryl_u128_ua*)({addr})); }})",
            addr = r.addr
        )
    })
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
    let nw = wide_words(nb);
    let c = emit_expr(cond)?;
    let mut t_ref = emit_wide_operand(true_expr, nb, pre)?;
    let mut f_ref = emit_wide_operand(false_expr, nb, pre)?;
    // Both-signed branches narrower than the result sign-extend to it
    // (LRM 11.4.11), but arrive zero-extended in their nb-byte buffer.
    // Extend into a FRESH temporary — the operand ref may alias canonical
    // storage.
    let t_w = true_expr.width();
    let f_w = false_expr.width();
    let needs_sext = true_expr.expr_context().signed
        && false_expr.expr_context().signed
        && t_w > 0
        && f_w > 0
        && (t_w < width || f_w < width);
    if needs_sext {
        for (r, w) in [(&mut t_ref, t_w), (&mut f_ref, f_w)] {
            if w >= width {
                continue;
            }
            let s = next_wide_tmp();
            pre.push_str(&format!(
                "uint64_t _w{s}[{nw}]; vw_sext_copy((uint8_t*)_w{s}, {src}, {w}u, {nb}u); ",
                src = r.addr,
            ));
            r.addr = format!("((uint8_t*)_w{s})");
        }
    }
    let t = next_wide_tmp();
    pre.push_str(&format!(
        "uint64_t _w{t}[{nw}]; int _c{t} = (({c}) != 0); \
         for (int _i{t} = 0; _i{t} < {nw}; _i{t}++) \
         _w{t}[_i{t}] = _c{t} ? ((const veryl_u64_ua*)({tp}))[_i{t}] \
                              : ((const veryl_u64_ua*)({fp}))[_i{t}]; ",
        tp = t_ref.addr,
        fp = f_ref.addr,
    ));
    if needs_sext {
        // Sign extension filled the bits above `width`; wide buffers are
        // invariantly masked.
        pre.push_str(&format!(
            "vw_apply_mask((uint8_t*)_w{t}, (const uint8_t*)0, {p}u); ",
            p = wpack(nb, width),
        ));
    }
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
    let nw = wide_words(nb);
    let acc = next_wide_tmp();
    let name = format!("_w{acc}");
    pre.push_str(&format!("uint64_t {name}[{nw}] = {{0}}; "));
    emit_wide_concat_body(
        elements,
        width,
        nb,
        &format!("(uint8_t*){name}"),
        &name,
        pre,
    )?;
    Some(WideRef {
        addr: format!("((uint8_t*)_w{acc})"),
        nb,
        width,
    })
}

/// Assemble a wide concat DIRECTLY into `dst` (a `uint8_t*` C expression
/// over zeroed or to-be-overwritten storage): one `|=` per element word
/// instead of marshaling through a `_w` temporary and copying it over.
/// The caller must guarantee no element reads the destination (self-reads
/// need the temporary form's read-before-write ordering).
fn emit_wide_concat_into(
    elements: &[(Box<ProtoExpression>, usize, usize)],
    width: usize,
    nb: usize,
    dst: &str,
    pre: &mut String,
) -> Option<()> {
    pre.push_str(&format!("__builtin_memset({dst}, 0, {nb}); "));
    let words = format!("((veryl_u64_ua*)({dst}))");
    emit_wide_concat_body(elements, width, nb, dst, &words, pre)
}

/// Shared assembly for `emit_wide_concat{,_into}`: `bytes` addresses the
/// buffer as `uint8_t*` (for the vw_* helpers), `words` as an lvalue u64
/// array (for the narrow `|=` inserts).
fn emit_wide_concat_body(
    elements: &[(Box<ProtoExpression>, usize, usize)],
    width: usize,
    nb: usize,
    bytes: &str,
    words: &str,
    pre: &mut String,
) -> Option<()> {
    let nw = nb / 8;

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

        if ew == 1 && repeat > 1 {
            // Replicated single bit (`{N{x}}` sign/mask extension): broadcast
            // the bit to a full word (`0 - bit`) and OR it under a per-word
            // immediate span mask instead of one `|=` per copy.
            let v = emit_expr(elem)?;
            let e = next_wide_tmp();
            pre.push_str(&format!(
                "uint64_t _e{e} = (uint64_t)0 - (((uint64_t)({v})) & 0x1ULL); ",
            ));
            let span_hi = hi; // exclusive
            hi -= repeat;
            let span_lo = hi; // inclusive
            for w in span_lo / 64..=(span_hi - 1) / 64 {
                if w >= nw {
                    break; // past the result width
                }
                let word_lo = w * 64;
                let f_lo = span_lo.max(word_lo);
                let f_hi = (span_hi - 1).min(word_lo + 63);
                let n = f_hi - f_lo + 1;
                let base = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
                let m = base << (f_lo - word_lo);
                if m == u64::MAX {
                    pre.push_str(&format!("{words}[{w}] |= _e{e}; "));
                } else {
                    pre.push_str(&format!("{words}[{w}] |= _e{e} & {m:#x}ULL; "));
                }
            }
            continue;
        }
        if ew <= 64 {
            // Mask to `ew`: the reference zero-extends the element from elem_width.
            let Some(v) = emit_expr(elem) else {
                if diag_enabled() {
                    // `{:.400}` truncates by chars — a byte slice could cut a
                    // UTF-8 sequence and panic.
                    eprintln!(
                        "[aot-c] wide-concat elem emit failed (ew={ew}): {:.400}",
                        format!("{elem:?}")
                    );
                }
                return None;
            };
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
                pre.push_str(&format!("{words}[{w}] |= _e{e} << {b}; "));
                // `b+ew>64` ⇒ `b>0`, so `64-b ∈ 1..=63` (no shift UB).
                if b + ew > 64 && w + 1 < nw {
                    pre.push_str(&format!(
                        "{words}[{w1}] |= _e{e} >> {sh}; ",
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
                     vw_bor({bytes}, (const uint8_t*){bytes2}, (const uint8_t*)_w{sh}, {nb}u); ",
                    e = e_ref.addr,
                    bytes2 = bytes,
                ));
            }
        }
    }

    pre.push_str(&format!(
        "vw_apply_mask({bytes}, (const uint8_t*)0, {p}u); ",
        p = wpack(nb, width),
    ));
    Some(())
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

/// How a bit-field store may treat the sub-word it writes into, when the
/// whole sub-word is redefined by a group of disjoint stores (see
/// `plan_field_groups`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FieldRole {
    /// First store of the group: define the window outright.
    Init,
    /// Later store: the bit is known clear, so OR it in.
    OrIn,
}

thread_local! {
    /// `(window address, bit mask)` -> role, for the comb list being emitted.
    /// Keyed by the field rather than by position because `emit_stmt` sees one
    /// statement at a time and the group's members are disjoint by
    /// construction.  Empty unless `plan_field_groups` armed it.
    static FIELD_ROLES: RefCell<HashMap<(isize, u64), FieldRole>> =
        RefCell::new(HashMap::default());
}

fn field_role(addr: isize, pmask: u64) -> Option<FieldRole> {
    FIELD_ROLES.with(|r| r.borrow().get(&(addr, pmask)).copied())
}

/// How far apart a window's stores may sit and still be gathered
/// (`VERYL_AOT_C_GATHER_SPAN`, 0 disables gathering entirely).
fn gather_span_limit() -> usize {
    env_span("VERYL_AOT_C_GATHER_SPAN", 512)
}

/// How far a single-reader def may be sunk to reach its reader
/// (`VERYL_AOT_C_SINK_SPAN`, 0 disables sinking entirely).
fn sink_span_limit() -> usize {
    env_span("VERYL_AOT_C_SINK_SPAN", usize::MAX)
}

fn env_span(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Clears `FIELD_ROLES` on the way out, including the `?` returns in
/// `emit_function` — a plan left behind would mis-role another module's
/// stores.
struct FieldRolesGuard;

impl Drop for FieldRolesGuard {
    fn drop(&mut self) {
        FIELD_ROLES.with(|r| r.borrow_mut().clear());
    }
}

/// Comb byte ranges a statement reads and writes, at field granularity — a
/// bit-select touches only the bytes its bits live in, because the wider load
/// the emit uses around it masks away everything else.  `false` means the
/// statement is opaque (a pre-compiled child, a testbench call) and the caller
/// must treat it as touching every byte.
///
/// FF offsets are ignored on purpose: comb-list FF writes land in the next
/// slot (NBA) and are invisible to same-eval readers, so they cannot order
/// against these windows.
fn comb_touches(
    s: &ProtoStatement,
    reads: &mut Vec<(isize, usize)>,
    writes: &mut Vec<(isize, usize)>,
) -> bool {
    /// Bytes spanned by `[lo .. lo+nbits)` of a variable based at `off`.
    fn field(off: isize, select: Option<(usize, usize)>, width: usize) -> (isize, usize) {
        match select {
            Some((hi, lo)) if hi >= lo => (off + (lo / 8) as isize, (hi / 8) - (lo / 8) + 1),
            _ => (off, native_bytes(width).max(1)),
        }
    }
    fn expr(e: &ProtoExpression, out: &mut Vec<(isize, usize)>) -> bool {
        match e {
            ProtoExpression::HierVariable(_) => false,
            ProtoExpression::Variable {
                var_offset,
                select,
                dynamic_select,
                width,
                var_full_width,
                ..
            } => {
                if let VarOffset::Comb(o) = var_offset {
                    let full = (*var_full_width).max(*width);
                    // A runtime bit-select lands anywhere in the variable.
                    let sel = if dynamic_select.is_some() {
                        None
                    } else {
                        *select
                    };
                    out.push(field(*o, sel, full));
                }
                dynamic_select
                    .as_ref()
                    .is_none_or(|ds| expr(&ds.index_expr, out))
            }
            ProtoExpression::Value { .. } => true,
            ProtoExpression::Unary { x, .. } => expr(x, out),
            ProtoExpression::Binary { x, y, .. } => expr(x, out) && expr(y, out),
            ProtoExpression::Concatenation { elements, .. } => {
                elements.iter().all(|(e, _, _)| expr(e, out))
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => expr(cond, out) && expr(true_expr, out) && expr(false_expr, out),
            ProtoExpression::DynamicVariable {
                base_offset,
                stride,
                element_native_bytes,
                index_expr,
                num_elements,
                ..
            } => {
                if let VarOffset::Comb(o) = base_offset {
                    // The index is a runtime value, so the whole array is live.
                    let span = stride.unsigned_abs() * num_elements.saturating_sub(1)
                        + element_native_bytes;
                    let base = if *stride < 0 {
                        o + stride * (num_elements.saturating_sub(1) as isize)
                    } else {
                        *o
                    };
                    out.push((base, span));
                }
                expr(index_expr, out)
            }
        }
    }
    match s {
        ProtoStatement::Assign(a) => {
            if let VarOffset::Comb(o) = a.dst {
                // A runtime bit-select writes anywhere in the variable.
                let sel = if a.dynamic_select.is_some() {
                    None
                } else {
                    a.select
                };
                writes.push(field(o, sel, a.dst_width));
            }
            a.dynamic_select
                .as_ref()
                .is_none_or(|ds| expr(&ds.index_expr, reads))
                && expr(&a.expr, reads)
        }
        ProtoStatement::AssignDynamic(a) => {
            if let VarOffset::Comb(o) = a.dst_base {
                let span = a.dst_stride.unsigned_abs() * a.dst_num_elements.saturating_sub(1)
                    + native_bytes(a.dst_width).max(1);
                writes.push((
                    o.min(o + a.dst_stride * (a.dst_num_elements as isize - 1)),
                    span,
                ));
            }
            expr(&a.dst_index_expr, reads)
                && expr(&a.expr, reads)
                && a.dynamic_select
                    .as_ref()
                    .is_none_or(|ds| expr(&ds.index_expr, reads))
        }
        ProtoStatement::If(x) => {
            x.cond.as_ref().is_none_or(|c| expr(c, reads))
                && x.true_side.iter().all(|s| comb_touches(s, reads, writes))
                && x.false_side.iter().all(|s| comb_touches(s, reads, writes))
        }
        ProtoStatement::Case(x) => {
            x.arms.iter().all(|arm| {
                expr(&arm.cond, reads) && arm.body.iter().all(|s| comb_touches(s, reads, writes))
            }) && x.default.iter().all(|s| comb_touches(s, reads, writes))
        }
        ProtoStatement::For(x) => {
            if let VarOffset::Comb(o) = x.var_offset {
                writes.push((o, 8));
            }
            let (start, end) = match &x.range {
                ProtoForRange::Forward { start, end, .. }
                | ProtoForRange::Reverse { start, end, .. }
                | ProtoForRange::Stepped { start, end, .. } => (start, end),
            };
            [start, end].into_iter().all(|b| match b {
                ProtoForBound::Dynamic(e) => expr(e, reads),
                _ => true,
            }) && x.body.iter().all(|s| comb_touches(s, reads, writes))
        }
        ProtoStatement::SequentialBlock(body) => {
            body.iter().all(|s| comb_touches(s, reads, writes))
        }
        ProtoStatement::SystemFunctionCall(x) => match x {
            ProtoSystemFunctionCall::Display { args, .. }
            | ProtoSystemFunctionCall::Write { args, .. } => args.iter().all(|a| expr(a, reads)),
            ProtoSystemFunctionCall::Assert {
                condition, args, ..
            } => expr(condition, reads) && args.iter().all(|a| expr(a, reads)),
            ProtoSystemFunctionCall::Readmemh { .. } | ProtoSystemFunctionCall::Finish => true,
        },
        // A pre-compiled child reads and writes comb_values directly, and a
        // testbench call can reach anything.
        ProtoStatement::CompiledBlock(_) | ProtoStatement::TbMethodCall { .. } => false,
        ProtoStatement::Break => true,
    }
}

/// Find the sub-words that a run of bit-field stores redefines completely, and
/// assign each store its role.
///
/// A window qualifies when disjoint top-level stores cover every one of its
/// bits and nothing between the first and the last of them touches the window:
/// then the first store may define the window instead of merging into it, and
/// the rest may OR their bit in.  Statements outside that span are unaffected —
/// before it they see the previous settle's value, after it the window is
/// fully defined either way.
fn plan_field_groups(stmts: &[ProtoStatement]) -> FieldPlan {
    /// `(window address, bytes, bit mask)` of the narrowed store a statement
    /// emits, if it emits one.
    fn narrowed_store(s: &ProtoStatement) -> Option<(isize, usize, u64)> {
        let ProtoStatement::Assign(a) = s else {
            return None;
        };
        let (VarOffset::Comb(off), Some((hi, lo))) = (a.dst, a.select) else {
            return None;
        };
        if a.dynamic_select.is_some() || a.dst_width <= 64 || a.dst_width > 128 || hi < lo {
            return None;
        }
        let (start, bytes) = narrow_field_window(lo, hi - lo + 1)?;
        Some((
            off + start as isize,
            bytes,
            width_mask(hi - lo + 1) << (lo - start * 8),
        ))
    }
    /// Every narrowed store in the tree, nested ones included.  Roles are keyed
    /// by field, so a nested store sharing a member's key would pick up that
    /// member's role; counting them lets the group be dropped instead.
    fn all_narrowed(s: &ProtoStatement, out: &mut HashMap<(isize, u64), usize>) {
        if let Some((addr, _, pmask)) = narrowed_store(s) {
            *out.entry((addr, pmask)).or_default() += 1;
        }
        let mut nested = |body: &[ProtoStatement]| {
            for s in body {
                all_narrowed(s, out);
            }
        };
        match s {
            ProtoStatement::If(x) => {
                nested(&x.true_side);
                nested(&x.false_side);
            }
            ProtoStatement::Case(x) => {
                for arm in &x.arms {
                    nested(&arm.body);
                }
                nested(&x.default);
            }
            ProtoStatement::For(x) => nested(&x.body),
            ProtoStatement::SequentialBlock(b) => nested(b),
            ProtoStatement::CompiledBlock(x) => nested(&x.original_stmts),
            _ => {}
        }
    }

    struct Member {
        idx: usize,
        pmask: u64,
    }
    let mut occurrences: HashMap<(isize, u64), usize> = HashMap::default();
    for s in stmts {
        all_narrowed(s, &mut occurrences);
    }
    let mut groups: HashMap<(isize, usize), Vec<Member>> = HashMap::default();
    for (idx, s) in stmts.iter().enumerate() {
        let Some((addr, bytes, pmask)) = narrowed_store(s) else {
            continue;
        };
        if occurrences.get(&(addr, pmask)) != Some(&1) {
            continue;
        }
        groups
            .entry((addr, bytes))
            .or_default()
            .push(Member { idx, pmask });
    }

    // Windows whose bits are covered exactly once each.
    groups.retain(|&(_, bytes), members| {
        let full = width_mask(bytes * 8);
        let mut seen = 0u64;
        members.len() > 1
            && members.iter().all(|m| {
                let fresh = seen & m.pmask == 0;
                seen |= m.pmask;
                fresh
            })
            && seen == full
    });

    // Byte -> statements touching it, for the ranges narrow enough to index
    // that way.  A memory array's range spans its whole storage, so those go
    // in a list that is scanned instead of expanded — otherwise one dynamic
    // write to a 128 KB array would cost 128 K map insertions, and this pass
    // runs on every emit.
    const EXPAND_LIMIT: usize = 16;
    #[derive(Default)]
    struct Index {
        /// byte -> statement indices, for ranges narrow enough to expand
        by_byte: HashMap<isize, Vec<usize>>,
        /// (start, end, statement) for the rest
        wide: Vec<(isize, isize, usize)>,
    }
    impl Index {
        fn add(&mut self, idx: usize, ranges: &[(isize, usize)]) {
            for &(base, len) in ranges {
                let end = base.saturating_add(len as isize);
                if len > EXPAND_LIMIT {
                    self.wide.push((base, end, idx));
                    continue;
                }
                for b in base..end {
                    let e = self.by_byte.entry(b).or_default();
                    if e.last() != Some(&idx) {
                        e.push(idx);
                    }
                }
            }
        }
        /// Any indexed statement overlapping `[start, end)` that `f` accepts.
        fn any(&self, start: isize, end: isize, f: impl Fn(usize) -> bool) -> bool {
            self.wide
                .iter()
                .any(|&(s, e, i)| s < end && e > start && f(i))
                || (start..end).any(|b| {
                    self.by_byte
                        .get(&b)
                        .is_some_and(|list| list.iter().any(|&i| f(i)))
                })
        }
        /// Every indexed statement overlapping `[start, end)`, ascending per
        /// byte; a statement spanning several of them is visited once each.
        fn for_each(&self, start: isize, end: isize, mut f: impl FnMut(usize)) {
            for &(s, e, i) in &self.wide {
                if s < end && e > start {
                    f(i);
                }
            }
            for b in start..end {
                if let Some(list) = self.by_byte.get(&b) {
                    list.iter().for_each(|&i| f(i));
                }
            }
        }
    }

    let mut touched = Index::default();
    let mut written = Index::default();
    let mut readers = Index::default();
    let mut read_ranges: Vec<Vec<(isize, usize)>> = vec![Vec::new(); stmts.len()];
    let mut barriers: Vec<usize> = Vec::new();
    for (idx, s) in stmts.iter().enumerate() {
        let (mut reads, mut writes) = (Vec::new(), Vec::new());
        if !comb_touches(s, &mut reads, &mut writes) {
            barriers.push(idx);
            continue;
        }
        touched.add(idx, &reads);
        touched.add(idx, &writes);
        written.add(idx, &writes);
        readers.add(idx, &reads);
        read_ranges[idx] = reads;
    }

    // Bytes owned by some candidate window.  A member that reads or writes a
    // window other than its own could be carried into that window's span by
    // its own group's move, so those windows are dropped rather than
    // re-checked against every possible destination.
    let mut window_bytes: HashMap<isize, (isize, usize)> = HashMap::default();
    for &(addr, bytes) in groups.keys() {
        for b in addr..addr + bytes as isize {
            window_bytes.insert(b, (addr, bytes));
        }
    }
    let mut foreign: HashSet<(isize, usize)> = HashSet::default();
    for (&(addr, bytes), members) in groups.iter() {
        for m in members {
            for &(base, len) in &read_ranges[m.idx] {
                for b in base..base.saturating_add(len as isize) {
                    if let Some(&w) = window_bytes.get(&b)
                        && w != (addr, bytes)
                    {
                        foreign.insert(w);
                    }
                }
            }
        }
    }
    // Overlapping windows would move into each other's spans for the same
    // reason.
    let mut overlapping: HashSet<(isize, usize)> = HashSet::default();
    for &(addr, bytes) in groups.keys() {
        for b in addr..addr + bytes as isize {
            if window_bytes.get(&b) != Some(&(addr, bytes)) {
                overlapping.insert((addr, bytes));
            }
        }
    }

    let mut roles = HashMap::default();
    let mut moves: Vec<(usize, usize)> = Vec::new(); // (statement, destination)
    for ((addr, bytes), mut members) in groups {
        members.sort_by_key(|m| m.idx);
        let (first, last) = (members[0].idx, members[members.len() - 1].idx);
        let window = addr..addr + bytes as isize;
        let member_at = |i: usize| members.iter().any(|m| m.idx == i);
        if barriers.iter().any(|&b| b > first && b < last) {
            continue;
        }
        // Nothing outside the group may touch the window while it is being
        // built.
        if touched.any(window.start, window.end, |i| {
            i > first && i < last && !member_at(i)
        }) {
            continue;
        }
        // A member reading its own window would see Init's zeroed bits where
        // the interpreter reads the previous settle's value.
        if members.iter().any(|m| {
            read_ranges[m.idx].iter().any(|&(base, len)| {
                base < window.end && base.saturating_add(len as isize) > window.start
            })
        }) {
            continue;
        }
        // Everything below decides only whether to GATHER; dropping the merge
        // is sound for any group that got this far.  The span cap keeps a
        // group from being dragged far out of its neighbourhood.
        let mut gather = last - first <= gather_span_limit()
            && !foreign.contains(&(addr, bytes))
            && !overlapping.contains(&(addr, bytes));
        // Sinking a member past a write to anything it reads would let it
        // capture the new value.
        gather = gather
            && members[..members.len() - 1].iter().all(|m| {
                read_ranges[m.idx].iter().all(|&(base, len)| {
                    !written.any(base, base.saturating_add(len as isize), |i| {
                        i > m.idx && i <= last && !member_at(i)
                    })
                })
            });
        for (n, m) in members.iter().enumerate() {
            roles.insert(
                (addr, m.pmask),
                if n == 0 {
                    FieldRole::Init
                } else {
                    FieldRole::OrIn
                },
            );
            if gather && m.idx != last {
                moves.push((m.idx, last));
            }
        }
    }

    // A def next to its only reader shares its chunk, so chunk-local
    // localization can turn it into a C local.
    let blocklist = LOCALIZE_BLOCKLIST.with(|b| b.borrow().clone());
    let ranges = LOCALIZE_RANGES.with(|r| r.borrow().clone());
    let in_range = |off: isize| -> bool {
        ranges.iter().any(|&(base, num, stride)| {
            stride != 0
                && num != 0
                && (off - base) >= 0
                && (off - base) % stride == 0
                && (off - base) / stride < num as isize
        })
    };
    let moved: HashSet<usize> = moves.iter().map(|&(s, _)| s).collect();
    let mut sinks: Vec<(usize, usize)> = Vec::new();
    if localize_armed() {
        for (d, s) in stmts.iter().enumerate() {
            if moved.contains(&d) {
                continue; // already a field-group member
            }
            let ProtoStatement::Assign(a) = s else {
                continue;
            };
            let VarOffset::Comb(off) = a.dst else {
                continue;
            };
            // Localization's own candidate shape: a clean full-width scalar
            // store nothing outside the comb schedule can observe.
            if a.select.is_some()
                || a.dynamic_select.is_some()
                || a.rhs_select.is_some()
                || a.dst_width == 0
                || a.dst_width > 64
                || blocklist.contains(&off)
                || in_range(off)
            {
                continue;
            }
            let (start, end) = (off, off + native_bytes(a.dst_width).max(1) as isize);
            // Exactly one writer (this one) and exactly one reader, both found
            // through the byte index so a wider access that overlaps counts.
            if written.any(start, end, |i| i != d) {
                continue;
            }
            // The same statement is indexed once per byte it covers, so the
            // fanout has to be counted over distinct statements.
            let mut seen: Vec<usize> = Vec::new();
            readers.for_each(start, end, |i| {
                if !seen.contains(&i) {
                    seen.push(i);
                }
            });
            let [r] = seen[..] else { continue };
            if r <= d || barriers.iter().any(|&b| b > d && b < r) {
                continue;
            }
            sinks.push((d, r));
        }
    }

    // Where a statement ends up once every move is applied: a def whose reader
    // is itself moved travels with it, so legality has to be checked against
    // that final position, not the reader's current one.
    let mut dest: HashMap<usize, usize> = moves.iter().copied().collect();
    for &(d, r) in &sinks {
        dest.insert(d, r);
    }
    fn terminal(mut i: usize, dest: &HashMap<usize, usize>, cap: usize) -> usize {
        for _ in 0..cap {
            match dest.get(&i) {
                Some(&n) => i = n,
                None => break,
            }
        }
        i
    }
    let limit = sink_span_limit();
    let mut drop: Vec<usize> = Vec::new();
    for &(d, r) in &sinks {
        let t = terminal(r, &dest, stmts.len());
        let ok = t - d <= limit
            && read_ranges[d].iter().all(|&(base, len)| {
                !written.any(base, base.saturating_add(len as isize), |i| i > d && i <= t)
            });
        if !ok {
            drop.push(d);
        }
    }
    for d in drop {
        dest.remove(&d);
    }

    // Arrivals emit just ahead of their destination, recursively — a chain
    // of single-reader defs lands as one run.
    let mut pending: HashMap<usize, Vec<usize>> = HashMap::default();
    for (&src, &dst) in &dest {
        pending.entry(dst).or_default().push(src);
    }
    for v in pending.values_mut() {
        v.sort_unstable();
    }
    let mut order = Vec::with_capacity(stmts.len());
    let mut spans: Vec<(usize, usize)> = Vec::new();
    // Explicit stack: sink chains nest arbitrarily deep.
    let mut stack: Vec<(usize, bool)> = Vec::new();
    for i in 0..stmts.len() {
        if dest.contains_key(&i) {
            continue; // travels to its destination instead
        }
        let before = order.len();
        stack.push((i, false));
        while let Some((n, expanded)) = stack.pop() {
            if expanded {
                order.push(n);
                continue;
            }
            stack.push((n, true));
            if let Some(arrivals) = pending.get(&n) {
                for &a in arrivals.iter().rev() {
                    stack.push((a, false));
                }
            }
        }
        if order.len() - before > 1 {
            spans.push((before, order.len() - before));
        }
    }
    debug_assert_eq!(order.len(), stmts.len());
    FieldPlan {
        roles,
        order,
        atoms: spans,
    }
}

/// What `plan_field_groups` decided: the per-field roles, the statement order
/// that puts each group's members together, and where those groups sit in it.
struct FieldPlan {
    roles: HashMap<(isize, u64), FieldRole>,
    order: Vec<usize>,
    /// `(start, len)` runs in `order` a chunk boundary must not split.
    atoms: Vec<(usize, usize)>,
}

/// `VERYL_AOT_C_SINK_DIAG=1`: counts single-writer / single-reader comb
/// bytes, how many the blocklist frees for localization, and how many
/// already share a chunk with their reader.
fn sink_census(stmts: &[ProtoStatement], chunks: &[&[ProtoStatement]]) {
    let mut chunk_of = Vec::with_capacity(stmts.len());
    for (c, chunk) in chunks.iter().enumerate() {
        chunk_of.extend(std::iter::repeat_n(c, chunk.len()));
    }
    let blocklist = LOCALIZE_BLOCKLIST.with(|b| b.borrow().clone());
    let mut readers: HashMap<isize, Vec<usize>> = HashMap::default();
    let mut writers: HashMap<isize, Vec<usize>> = HashMap::default();
    let mut opaque = 0usize;
    for (i, s) in stmts.iter().enumerate() {
        let (mut r, mut w) = (Vec::new(), Vec::new());
        if !comb_touches(s, &mut r, &mut w) {
            opaque += 1;
            continue;
        }
        for &(base, len) in &r {
            for b in base..base.saturating_add(len as isize) {
                readers.entry(b).or_default().push(i);
            }
        }
        for &(base, len) in &w {
            for b in base..base.saturating_add(len as isize) {
                writers.entry(b).or_default().push(i);
            }
        }
    }
    let (mut defs, mut single, mut free, mut same_chunk) = (0, 0, 0, 0);
    for (&off, w) in &writers {
        if w.len() != 1 {
            continue;
        }
        defs += 1;
        let Some(r) = readers.get(&off) else { continue };
        if r.len() != 1 {
            continue;
        }
        single += 1;
        if blocklist.contains(&off) {
            continue;
        }
        free += 1;
        if chunk_of.get(w[0]) == chunk_of.get(r[0]) {
            same_chunk += 1;
        }
    }
    eprintln!(
        "[sink_census] stmts={} chunks={chunks_n} opaque={opaque} | single-writer bytes={defs} \
         single-reader={single} not-blocklisted={free} already-same-chunk={same_chunk} \
         => sinkable={sinkable}",
        stmts.len(),
        chunks_n = chunks.len(),
        sinkable = free - same_chunk,
    );
}

/// Const-cone partition: reorder `stmts` so that statements whose inputs are
/// constants all the way down come first, returning the reordered list and
/// the prefix length.  Those statements compute the same value on every
/// settle, so the module exports them as a separate `veryl_aot_eval_const`
/// entry the runtime runs ONCE per simulator instance instead of every
/// settle.  Soundness rests on five exclusions (each of the first four
/// was once missing, and VALIDATE caught the divergence):
/// - every statement is walked to the END even once disqualified, so its
///   writes still register for co-writer demotion (no short-circuit);
/// - a CompiledBlock's ORIGINAL statements are walked (its input/output
///   offset lists compress dynamic accesses to base + last element and
///   would hide interior writes);
/// - a reader whose writer does not PRECEDE it (back-edge / self-RMW)
///   reads last settle's value and is never const;
/// - any offset covered by a dynamic-write range (AssignDynamic) demotes
///   const candidates that touch it;
/// - a statement reading OR writing an event-written offset
///   (`unsafe_comb`) is never const — the per-settle rerun is what keeps
///   clobbering the event's value.
///
/// `None` disarms the split entirely (a Readmemh / TB-method statement,
/// or a CompiledBlock without original statements, writes storage this
/// walker cannot bound).
fn const_cone_partition(
    stmts: &[ProtoStatement],
    unsafe_comb: &HashSet<isize>,
) -> Option<(Vec<ProtoStatement>, usize, Vec<bool>)> {
    #[derive(Default)]
    struct Io {
        /// (is_ff, base offset) of every scalar read.
        reads: Vec<(bool, isize)>,
        /// Comb base offsets written (incl. CompiledBlock outputs and For
        /// loop counters).
        writes: Vec<isize>,
        /// The statement itself can never be const (but its I/O above still
        /// participates in demotion).
        opaque: bool,
    }
    /// Dynamic comb WRITE ranges (base, num, stride).
    type WRanges = Vec<(isize, usize, isize)>;
    fn expr_io(e: &ProtoExpression, io: &mut Io) {
        match e {
            ProtoExpression::HierVariable(_) => io.opaque = true,
            ProtoExpression::Value { .. } => {}
            ProtoExpression::Variable {
                var_offset,
                dynamic_select,
                ..
            } => {
                match var_offset {
                    VarOffset::Ff(o) => io.reads.push((true, *o)),
                    VarOffset::Comb(o) => io.reads.push((false, *o)),
                }
                if let Some(d) = dynamic_select {
                    io.opaque = true;
                    expr_io(&d.index_expr, io);
                }
            }
            ProtoExpression::Unary { x, .. } => expr_io(x, io),
            ProtoExpression::Binary { x, y, .. } => {
                expr_io(x, io);
                expr_io(y, io);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (e, _, _) in elements {
                    expr_io(e, io);
                }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                expr_io(cond, io);
                expr_io(true_expr, io);
                expr_io(false_expr, io);
            }
            ProtoExpression::DynamicVariable { index_expr, .. } => {
                // Disqualifies only the reader: the (run-once) writers it
                // might read hold the same values either way.
                io.opaque = true;
                expr_io(index_expr, io);
            }
        }
    }
    /// Returns false when the module must disarm entirely.
    fn stmt_io(s: &ProtoStatement, io: &mut Io, wranges: &mut WRanges) -> bool {
        match s {
            ProtoStatement::Assign(a) => {
                if a.dynamic_select.is_some() || a.dst.is_ff() {
                    io.opaque = true;
                }
                if !a.dst.is_ff() {
                    io.writes.push(a.dst.raw());
                }
                if let Some(d) = &a.dynamic_select {
                    expr_io(&d.index_expr, io);
                }
                expr_io(&a.expr, io);
                true
            }
            ProtoStatement::AssignDynamic(a) => {
                io.opaque = true;
                if !a.dst_base.is_ff() && a.dst_stride != 0 {
                    wranges.push((a.dst_base.raw(), a.dst_num_elements, a.dst_stride));
                }
                expr_io(&a.dst_index_expr, io);
                expr_io(&a.expr, io);
                true
            }
            ProtoStatement::If(x) => {
                if let Some(c) = &x.cond {
                    expr_io(c, io);
                }
                x.true_side
                    .iter()
                    .chain(x.false_side.iter())
                    .all(|s| stmt_io(s, io, wranges))
            }
            ProtoStatement::Case(x) => {
                x.arms.iter().all(|arm| {
                    expr_io(&arm.cond, io);
                    arm.body.iter().all(|s| stmt_io(s, io, wranges))
                }) && x.default.iter().all(|s| stmt_io(s, io, wranges))
            }
            ProtoStatement::SequentialBlock(inner) => inner.iter().all(|s| stmt_io(s, io, wranges)),
            ProtoStatement::CompiledBlock(cb) => {
                // `input_offsets`/`output_offsets` compress a dynamic array
                // access to base + last element, hiding interior elements
                // from the co-writer and wrange rules — walk the original
                // statements instead.  No originals = unboundable: disarm.
                io.opaque = true;
                if cb.original_stmts.is_empty() {
                    return false;
                }
                cb.original_stmts.iter().all(|s| stmt_io(s, io, wranges))
            }
            ProtoStatement::For(f) => {
                io.opaque = true;
                if !f.var_offset.is_ff() {
                    io.writes.push(f.var_offset.raw());
                }
                f.body.iter().all(|s| stmt_io(s, io, wranges))
            }
            ProtoStatement::Break => true,
            ProtoStatement::SystemFunctionCall(c) => {
                io.opaque = true;
                // Readmemh writes storage this walker does not model.
                !matches!(c, crate::ir::ProtoSystemFunctionCall::Readmemh { .. })
            }
            // TB-method writes are not modeled either.
            ProtoStatement::TbMethodCall { .. } => false,
        }
    }
    let mut wranges: WRanges = Vec::new();
    let mut ios: Vec<Io> = Vec::with_capacity(stmts.len());
    for s in stmts {
        let mut io = Io::default();
        if !stmt_io(s, &mut io, &mut wranges) {
            return None;
        }
        ios.push(io);
    }
    let in_wrange = |off: isize| -> bool {
        wranges.iter().any(|&(base, num, stride)| {
            let d = off - base;
            d >= 0 && d % stride == 0 && (d / stride) < num as isize
        })
    };
    let mut writers: HashMap<isize, Vec<usize>> = HashMap::default();
    for (i, io) in ios.iter().enumerate() {
        for &w in &io.writes {
            writers.entry(w).or_default().push(i);
        }
    }
    let mut is_const: Vec<bool> = ios
        .iter()
        .map(|io| {
            !io.opaque
                && io.reads.iter().all(|&(ff, o)| {
                    !ff && !unsafe_comb.contains(&o) && writers.contains_key(&o) && !in_wrange(o)
                })
                // An event co-writes `w`: rerunning the (const) comb write
                // every settle is what keeps clobbering the event's value,
                // so freezing it after one run changes what readers see.
                && io
                    .writes
                    .iter()
                    .all(|&w| !in_wrange(w) && !unsafe_comb.contains(&w))
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..ios.len() {
            if !is_const[i] {
                continue;
            }
            let ok = ios[i].reads.iter().all(|&(_, o)| {
                writers
                    .get(&o)
                    .is_none_or(|ws| ws.iter().all(|&j| is_const[j] && j < i))
            }) && ios[i]
                .writes
                .iter()
                .all(|&w| writers[&w].iter().all(|&j| is_const[j]));
            if !ok {
                is_const[i] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let n = is_const.iter().filter(|&&c| c).count();
    if n == 0 {
        return None;
    }
    let mut out: Vec<ProtoStatement> = Vec::with_capacity(stmts.len());
    for (i, s) in stmts.iter().enumerate() {
        if is_const[i] {
            out.push(s.clone());
        }
    }
    for (i, s) in stmts.iter().enumerate() {
        if !is_const[i] {
            out.push(s.clone());
        }
    }
    Some((out, n, is_const))
}

/// The aligned `(start_byte, width_bytes)` sub-word of a 16-byte container
/// that holds the field `[lo .. lo+nbits)`, narrowest first.  `None` when the
/// field is wider than 64 bits or straddles every candidate window's
/// boundary, leaving the caller on the 128-bit path.
///
/// Windows are aligned to their own width and 16 is a multiple of each, so
/// `start + width <= 16` holds for every result — the access cannot reach
/// past the container.
fn narrow_field_window(lo: usize, nbits: usize) -> Option<(usize, usize)> {
    if nbits == 0 || nbits > 64 || lo + nbits > 128 {
        return None;
    }
    let byte_lo = lo / 8;
    let byte_hi = (lo + nbits - 1) / 8;
    [1usize, 2, 4, 8]
        .into_iter()
        .map(|w| ((byte_lo / w) * w, w))
        .find(|&(start, w)| byte_hi < start + w)
}

/// Read-modify-write the field `[lo .. lo+nbits)` of a 65..128-bit comb slot
/// through `narrow_field_window`'s sub-word instead of the whole 16-byte
/// container.  `None` leaves the caller on the 128-bit form.
///
/// Bytes outside the field keep their old values, which is what the wide form
/// does too, so `comb_values` ends up byte-identical.  Slot offsets are only
/// 4-byte aligned in general, hence the unaligned typedefs.
fn narrow_field_store(
    rhs: &str,
    buf: &str,
    store_off: isize,
    lo: usize,
    nbits: usize,
) -> Option<String> {
    let (start, bytes) = narrow_field_window(lo, nbits)?;
    let cty = match bytes {
        1 => "uint8_t",
        2 => "veryl_u16_ua",
        4 => "veryl_u32_ua",
        _ => "veryl_u64_ua",
    };
    let shift = lo - start * 8;
    // `nbits == 64` implies bytes == 8 and shift == 0, so the masks below are
    // the full word and `1u64 << 64` is never evaluated.
    let vmask: u64 = width_mask(nbits);
    let pmask: u64 = vmask << shift;
    let addr = store_off + start as isize;
    let value = format!("({cty})((((uint64_t)({rhs})) & 0x{vmask:x}ULL) << {shift})");
    match field_role(addr, pmask) {
        Some(FieldRole::Init) => Some(format!("*(({cty}*)({buf} + {addr:#x})) = {value};")),
        Some(FieldRole::OrIn) => Some(format!("*(({cty}*)({buf} + {addr:#x})) |= {value};")),
        None => Some(format!(
            "{{ uint64_t _v = ((uint64_t)({rhs})) & 0x{vmask:x}ULL; \
                {cty} _o = *(({cty}*)({buf} + {addr:#x})); \
                *(({cty}*)({buf} + {addr:#x})) = \
                  ({cty})((_o & ({cty})(~(uint64_t)0x{pmask:x}ULL)) | ({cty})(_v << {shift})); }}",
        )),
    }
}

/// Clean-bits mask elision knob (`VERYL_AOT_C_CLEAN`, default on, `=0` to
/// opt out — the A/B lever for the redundant-mask removal).
fn clean_elide() -> bool {
    std::env::var("VERYL_AOT_C_CLEAN").as_deref() != Ok("0")
}

/// Conservative cleanliness analysis: true when the C value that
/// `emit_expr_inner(e, false)` emits provably has no bits above `e.width()`.
/// Lets the consumer-side re-masks (store width mask, concat slot mask,
/// reduction pre-mask, `x & C` wrappers) be elided.  gcc folds only some of
/// them itself: it cannot see the storage canonicality invariant — every
/// store masks to the variable's declared width — that this analysis starts
/// from.
///
/// Soundness contract: an elided mask must leave the emitted C value
/// BIT-IDENTICAL to the masked form (the VERYL_AOT_C_VALIDATE dual-run
/// compares storage bytes against the JIT).  Every rule below mirrors the
/// corresponding emitter arm's needs_clean=false emission; anything
/// uncertain — width 0 / >64 results, dirty producers (~, unary/binary
/// minus, <<, xnor/nand/nor, sign-extending forms), Pow, HierVariable —
/// answers false.
fn expr_emits_clean(e: &ProtoExpression) -> bool {
    let w = e.width();
    if w == 0 || w > 64 {
        return false;
    }
    match e {
        // emit_value masks the payload to the node width.
        ProtoExpression::Value { .. } => true,
        ProtoExpression::Variable {
            select,
            dynamic_select,
            width,
            var_full_width,
            ..
        } => {
            if dynamic_select.is_some() || select.is_some() {
                // Static selects emit `(v >> lo) & mask`; dynamic selects
                // mask the funnel window — both canonical by construction.
                true
            } else {
                // Full load: storage is canonical (every store path masks
                // to the declared width), so the load carries no bits
                // above the variable width.
                *width == *var_full_width
            }
        }
        // Element loads read canonical storage; select forms mask.
        ProtoExpression::DynamicVariable { .. } => true,
        ProtoExpression::Unary { op, .. } => match op {
            // Predicates and reductions produce 0/1.
            Op::LogicNot
            | Op::BitOr
            | Op::BitNor
            | Op::BitAnd
            | Op::BitNand
            | Op::BitXor
            | Op::BitXnor => true,
            // `~x` / `-x` emitted with needs_clean=false stay dirty.
            _ => false,
        },
        ProtoExpression::Binary {
            x,
            op,
            y,
            expr_context,
            ..
        } => {
            let sub_clean = |s: &ProtoExpression| s.width() <= w && expr_emits_clean(s);
            match op {
                // Comparisons and logical connectives produce 0/1.  (The
                // signed forms sign-extend OPERANDS, not the 0/1 result.)
                Op::Less
                | Op::Greater
                | Op::LessEq
                | Op::GreaterEq
                | Op::Eq
                | Op::Ne
                | Op::EqWildcard
                | Op::NeWildcard
                | Op::LogicAnd
                | Op::LogicOr => true,
                // High bits of x&y are zero when either side's are; x|y and
                // x^y when both sides'.  NOT under a signed context: the
                // signed emission sign-extends narrow operands to the
                // context width and the bitwise result is never re-masked.
                Op::BitAnd if !expr_context.signed => sub_clean(x) || sub_clean(y),
                Op::BitOr | Op::BitXor if !expr_context.signed => sub_clean(x) && sub_clean(y),
                // Logical right shift of a clean value stays clean.  The
                // arithmetic form sign-fills unless the context is
                // unsigned (then it emits the logical form).
                Op::LogicShiftR => sub_clean(x),
                Op::ArithShiftR => !expr_context.signed && sub_clean(x),
                // Unsigned quotient/remainder never exceed the dividend.
                Op::Div | Op::Rem => !expr_context.signed && sub_clean(x),
                // The cast op passes its operand through unchanged.
                Op::As => sub_clean(x),
                // Add/Sub/Mul (carry/borrow), Shl, xnor/nand/nor (~) are
                // dirty under needs_clean=false; Pow is unsupported.
                _ => false,
            }
        }
        ProtoExpression::Ternary {
            true_expr,
            false_expr,
            ..
        } => {
            // The both-signed-narrower special case re-masks its result;
            // the plain form selects one arm verbatim.
            let t_w = true_expr.width();
            let f_w = false_expr.width();
            let both_signed = true_expr.expr_context().signed
                && false_expr.expr_context().signed
                && t_w > 0
                && f_w > 0;
            if both_signed && (t_w < w || f_w < w) {
                true
            } else {
                t_w <= w && f_w <= w && expr_emits_clean(true_expr) && expr_emits_clean(false_expr)
            }
        }
        // Concat assembly bounds every element into its slot (and after
        // elision only provably-clean elements drop their slot mask), so
        // the ≤64-bit accumulator never carries bits above the total width.
        ProtoExpression::Concatenation { .. } => true,
        ProtoExpression::HierVariable(_) => false,
    }
}

/// Event-path WIDE FF write (static dst, `dst_width > 64`): materialize the
/// masked RHS into a scratch and push it through the 64-byte WriteLogWideEntry
/// pool (≤56-byte payload chunks).  Covers 65-128 bit (scalar promoted) and
/// `>128` bit (helper-table value).  A static slice covering whole words
/// reduces to the full-width form at a shifted offset: the commit is a plain
/// byte-range copy, so a byte-exact subrange needs no read-modify-write
/// against uncommitted state.  Other select / dynamic_select / rhs_select
/// wide FFs stay on Cranelift (the module bails).  2-state only.
fn emit_event_ff_assign_wide(a: &ProtoAssignStatement) -> Option<String> {
    if let Some(ds) = &a.dynamic_select
        && a.select.is_none()
        && a.rhs_select.is_none()
        && let Some(s) = emit_event_ff_assign_wide_dynsel(a, ds)
    {
        return Some(s);
    }
    let aligned_slice = match a.select {
        None => Some(None),
        Some((hi, lo)) => {
            let nbits = hi.checked_sub(lo).and_then(|d| d.checked_add(1));
            match nbits {
                Some(n) if lo % 64 == 0 && n % 64 == 0 && n > 64 => Some(Some((lo, n))),
                _ => None,
            }
        }
    };
    let (Some(slice), false, None) = (aligned_slice, a.dynamic_select.is_some(), &a.rhs_select)
    else {
        ev_diag(&format!(
            "wide FF: select={:?} dynsel={} rhssel={:?} width={}",
            a.select,
            a.dynamic_select.is_some(),
            a.rhs_select,
            a.dst_width
        ));
        return None;
    };
    let (byte_off, eff_width) = match slice {
        Some((lo, n)) => ((lo / 8) as isize, n),
        None => (0, a.dst_width),
    };
    let dst_raw = match a.dst {
        VarOffset::Ff(o) => o,
        VarOffset::Comb(_) => return None,
    };
    let cur_off = a.dst_ff_current_offset;
    if cur_off < 0 || dst_raw < 0 {
        return None;
    }
    let packed = dst_raw == cur_off;
    let (dst_raw, cur_off) = (dst_raw + byte_off, cur_off + byte_off);
    let nb = native_bytes(eff_width);
    let nw = wide_words(nb);
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
        p = wpack(nb, eff_width),
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

/// Runtime whole-element write into a packed wide FF (`ff[idx] <= v` on an
/// array whose element spans whole words — a `+:` part-select on a flat
/// register instead strides by one bit and is declined below).  Same
/// byte-exact reduction as above, at a runtime offset.  The index clamps to
/// the last element, mirroring `AssignStatement::eval_step`.
fn emit_event_ff_assign_wide_dynsel(
    a: &ProtoAssignStatement,
    ds: &crate::ir::ProtoDynamicBitSelect,
) -> Option<String> {
    let ew = ds.elem_width;
    let ne = ds.num_elements;
    if ew < 64 || !ew.is_multiple_of(64) || ds.window != ew || ne == 0 {
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
    let nb = ew / 8;
    let nw = wide_words(nb);
    let mut pre = String::new();
    let idx = emit_expr(&ds.index_expr)?;
    let r = emit_wide_operand(&a.expr, nb, &mut pre)?;
    let d = next_wide_tmp();
    pre.push_str(&format!(
        "uint64_t _di{d} = (uint64_t)({idx}); \
         if (_di{d} > {max}ull) _di{d} = {max}ull; \
         uint64_t _w{d}[{nw}]; vw_copy((uint8_t*)_w{d}, {src}, {nb}u); \
         vw_apply_mask((uint8_t*)_w{d}, (const uint8_t*)0, {p}u); ",
        max = ne - 1,
        src = r.addr,
        p = wpack(nb, ew),
    ));
    let store = if packed {
        String::new()
    } else {
        format!(
            "vw_copy((uint8_t*)(ff_values + {dst:#x}) + _di{d} * {nb}u, \
             (const uint8_t*)_w{d}, {nb}u); ",
            dst = dst_raw,
        )
    };
    let push = emit_wide_log_chunks(
        &format!("(uint8_t*)_w{d}"),
        &format!("({cur_off:#x}u + (unsigned)(_di{d} * {nb}u))"),
        nb,
    );
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
    let nw = wide_words(nb);
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
    /// Run-once constant-cone entry (`veryl_aot_eval_const`); absent when the
    /// module has no const prefix.  Same ABI as `func`.
    pub const_func: Option<FuncPtr>,
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
            thread::available_parallelism()
                .map(|n| (n.get() / 4).max(2))
                .unwrap_or(2)
        })
}

/// Background-compile niceness (`VERYL_AOT_C_NICE`, default 10; 0 keeps the
/// parent's priority).  The compile is off the critical path — the
/// simulation runs on Cranelift / the incremental sweep while it proceeds —
/// so `cc` must yield to the sim threads instead of competing with them.
fn compile_nice() -> i32 {
    std::env::var("VERYL_AOT_C_NICE")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(10)
}

/// Linux applies `PRIO_PROCESS` with `who == 0` to the calling THREAD, and
/// `cc` children inherit it — so this renices the pool, not the simulation.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn renice_compile_thread() {
    let n = compile_nice();
    if n != 0 {
        // SAFETY: plain syscall wrapper.  Failure is ignored: the priority
        // is an optimization, not a correctness input.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, n) };
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn renice_compile_thread() {}

/// Lazily-started global pool of `compile_jobs()` workers draining a shared
/// queue; returns the job sender.
///
/// In async mode each whole-module compile used to get its own detached
/// `thread::spawn` → `cc`.  The simulator never blocks on them (it stays
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
            let _ = thread::Builder::new()
                .name("veryl-aot-cc".into())
                .spawn(move || {
                    renice_compile_thread();
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
         typedef uint32_t veryl_u32_ua __attribute__((__aligned__(1)));\n\
         typedef uint16_t veryl_u16_ua __attribute__((__aligned__(1)));\n\
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

fn gc_age() -> Option<Duration> {
    let hours = std::env::var("VERYL_AOT_C_GC_HOURS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(24);
    (hours != 0).then(|| Duration::from_secs(hours * 3600))
}

/// A compile's own temp files, `veryl_aot_<hash>.<pid>.<n>.<ext>`; published
/// artifacts carry no such infix.  A failed compile's `.log` is left alone:
/// it is the only record of why it failed.
fn is_temp_artifact(name: &str) -> bool {
    let mut parts = name.split('.');
    let (Some(stem), Some(pid), Some(ctr), Some(ext), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    stem.starts_with("veryl_aot_")
        && pid.parse::<u64>().is_ok()
        && ctr.parse::<u64>().is_ok()
        && matches!(ext, "c" | "so")
}

fn sweep_temp_artifacts(dir: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_temp_artifact(name) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|m| m < cutoff);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Sweep the temp files of compiles that never published.
///
/// The compiler is detached so it can publish after the run exits; nothing
/// observes it dying either, so its temp files would stay forever.  Their
/// pid cannot separate a dead compile from a running one — it belongs to
/// whatever namespace the writer ran in — so age does, with a default
/// cutoff well past the longest compile still plausibly running.
fn gc_orphan_temps(cache_dir: &Path) {
    static SWEPT: OnceLock<()> = OnceLock::new();
    SWEPT.get_or_init(|| {
        if let Some(age) = gc_age()
            && let Some(cutoff) = std::time::SystemTime::now().checked_sub(age)
        {
            sweep_temp_artifacts(cache_dir, cutoff);
        }
    });
}

/// The chunked emit writes this a few hundred lines above, so the split below
/// cannot drift from it.
const CHUNK_FN_MARKER: &str = "static __attribute__((noinline)) void veryl_aot_chunk_";

/// Bytes of C per translation unit, and the ceiling on how many to make.
///
/// One `cc` on a multi-megabyte source is the whole cold-start latency: the
/// simulation runs on Cranelift until the `.so` lands, and Cranelift is
/// several times slower than the compiled code.  Splitting shortens that
/// window without changing how many compiles run at once (the pool decides
/// that).  The size is what the ceiling is for — parts much smaller than this
/// spend their time on the header every unit repeats.
const TU_SPLIT_BYTES: usize = 1536 * 1024;
const TU_SPLIT_MAX: usize = 8;

/// How many translation units to compile `src` as.
///
/// Derived from the source alone, never from machine load: a split that varied
/// run to run would give one cache key sources that compile to different
/// binaries, and would make a split-only bug reproduce only sometimes.
/// `VERYL_AOT_C_TU_SPLIT` pins it (0/1 disables splitting).
fn tu_split_count(src: &str) -> usize {
    #[cfg(test)]
    if let Some(v) = TEST_TU_SPLIT.with(|c| c.get()) {
        return v;
    }
    if let Ok(v) = std::env::var("VERYL_AOT_C_TU_SPLIT") {
        return v.parse::<usize>().unwrap_or(1).max(1);
    }
    (src.len() / TU_SPLIT_BYTES).clamp(1, TU_SPLIT_MAX)
}

// Thread-local so a test that pins the split cannot perturb the tests running
// beside it — the env var is process-global and libtest is multi-threaded.
#[cfg(test)]
thread_local! {
    static TEST_TU_SPLIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Split one emitted comb source into `n` compilable units.
///
/// The chunk functions are already separate and `noinline`, so nothing that
/// was inlined stops being inlined; what each unit repeats is the header of
/// `static inline` helpers, which every unit still inlines from.  `None` when
/// the source does not have the shape below — the caller then compiles it
/// whole, which is always correct.
fn split_translation_units(src: &str, n: usize) -> Option<Vec<String>> {
    if n < 2 {
        return None;
    }
    let lines: Vec<&str> = src.split('\n').collect();
    let chunk_starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with(CHUNK_FN_MARKER))
        .map(|(i, _)| i)
        .collect();
    // Fewer chunks than units would leave empty ones; the entry functions must
    // follow the last chunk for the tail split below to hold.
    if chunk_starts.len() < n * 2 {
        return None;
    }
    let last_chunk = *chunk_starts.last()?;
    let entry_start = lines
        .iter()
        .enumerate()
        .position(|(i, l)| i > last_chunk && l.starts_with(ENTRY_ATTR))?;

    let header = &lines[..chunk_starts[0]];
    let entries = &lines[entry_start..];
    // Only the wide-op table may cross units, and only because the split gives
    // it one definition and the rest a declaration.  Anything else exported
    // from the header would lose its definition in the non-entry units, so
    // leave such a source whole.
    if header
        .iter()
        .any(|l| l.starts_with(ENTRY_ATTR) && !l.contains(WIDEOPS_DEF) && !l.contains(WIDEOPS_SET))
    {
        return None;
    }
    // Every unit defines the chunks it holds and declares the rest, so the
    // entry unit can call across.  `hidden` keeps them out of the dynamic
    // symbol table exactly as `static` did.
    let decls: Vec<String> = chunk_starts
        .iter()
        .filter_map(|&i| {
            let name = lines[i]
                .strip_prefix(CHUNK_FN_MARKER)?
                .split('(')
                .next()?
                .trim();
            Some(format!("{CHUNK_DECL_ATTR} void veryl_aot_chunk_{name}(uint8_t *__restrict__, uint8_t *__restrict__, uint64_t *__restrict__);"))
        })
        .collect();
    if decls.len() != chunk_starts.len() {
        return None;
    }

    let mut units = Vec::with_capacity(n);
    for part in 0..n {
        let lo = chunk_starts[chunk_starts.len() * part / n];
        let hi = if part + 1 == n {
            entry_start
        } else {
            chunk_starts[chunk_starts.len() * (part + 1) / n]
        };
        let mut out: Vec<String> = Vec::with_capacity(header.len() + decls.len() + (hi - lo) + 16);
        for l in header {
            // The wide-op table has one definition, in the entry unit; the
            // others reach it through the dynamic symbol the setter publishes.
            if part == 0 || !l.starts_with(ENTRY_ATTR) {
                out.push((*l).to_string());
            } else if l.contains(WIDEOPS_DEF) {
                out.push(format!("extern {WIDEOPS_DEF}"));
            }
        }
        out.extend(decls.iter().cloned());
        out.extend(
            lines[lo..hi]
                .iter()
                .map(|l| l.replace(CHUNK_FN_MARKER, CHUNK_DEF_PREFIX)),
        );
        if part == 0 {
            out.extend(entries.iter().map(|l| (*l).to_string()));
        }
        units.push(out.join("\n"));
    }
    Some(units)
}

const ENTRY_ATTR: &str = "__attribute__((visibility(\"default\")))";
const WIDEOPS_DEF: &str = "veryl_wideops_t veryl_wideops;";
const WIDEOPS_SET: &str = "void veryl_set_wideops(";
const CHUNK_DECL_ATTR: &str = "__attribute__((noinline,visibility(\"hidden\")))";
const CHUNK_DEF_PREFIX: &str =
    "__attribute__((noinline,visibility(\"hidden\"))) void veryl_aot_chunk_";

/// `compile_source` with an explicit cache directory instead of resolving
/// it from `VERYL_AOT_CACHE_DIR`/`XDG_CACHE_HOME`/`HOME`.  Tests pass a
/// per-test dir here directly: the cache dir is a *process-global* env var,
/// so mutating it from one test perturbs every other test compiling
/// concurrently (libtest runs tests multi-threaded by default).  Passing it
/// as an argument keeps each test hermetic without touching shared state.
fn compile_source_in(cache_dir: &Path, src: &str) -> Result<EmittedModule, String> {
    fs::create_dir_all(cache_dir).map_err(|e| format!("create_dir_all: {e}"))?;
    gc_orphan_temps(cache_dir);

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
    // Comb sources carry the (noslp) header marker when their wide-op
    // density is below the static threshold (see the SLP policy in
    // emit_function).
    if src.starts_with("// AOT-C generated (noslp)") {
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

    // One compiler per artifact hash, across processes and pool workers alike.
    // `Published` means someone else landed it while we waited; the second
    // `exists` check closes the window between the first one and the lock.
    let ticket = if so_path.exists() {
        CompileTicket::Published
    } else {
        acquire_compile_lock(cache_dir, &hash, &so_path)
    };
    // Only the unix path hands the lock to the shell; elsewhere `Drop` alone
    // releases it.
    #[cfg(unix)]
    let lock_path = match &ticket {
        CompileTicket::Owned(l) => Some(l.path.clone()),
        CompileTicket::Published | CompileTicket::Unlocked => None,
    };
    if !matches!(ticket, CompileTicket::Published) && !so_path.exists() {
        // Identical sources hash to the same `so_path`, so a `cc -o so_path`
        // from one thread can be dlopened half-written by another. Compile to a
        // unique temp, then `rename`/`mv` (atomic within the dir) to publish.
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
        // Where the compiler's own output goes: removed on success, left
        // beside the kept `.c` on failure.
        let log_path = cache_dir.join(format!("veryl_aot_{hash}.{uniq}.log"));
        fs::write(&tmp_c, src).map_err(|e| format!("write {}: {}", tmp_c.display(), e))?;

        // Split units are compiled in parallel and linked; `tmp_c` above stays
        // the source that gets published, so the cache entry keeps naming the
        // whole module however it was built.
        #[cfg(unix)]
        let unit_paths: Vec<PathBuf> = split_translation_units(src, tu_split_count(src))
            .map(|units| {
                units
                    .iter()
                    .enumerate()
                    .map(|(i, u)| {
                        let p = cache_dir.join(format!("veryl_aot_{hash}.{uniq}.u{i}.c"));
                        fs::write(&p, u).map(|_| p)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(|e| format!("write split unit: {e}"))?
            .unwrap_or_default();

        // The compile AND the publish run through one shell so the cache
        // entry lands even when this process exits first: a short run
        // finishes before cc does and the pool worker dies with it, so a
        // rename on the Rust side would discard the orphaned cc's output
        // and leave every rerun on the JIT path.  Orphan reparenting keeps
        // the shell running.  Positional parameters keep the paths out of
        // shell-quoting territory.
        #[cfg(unix)]
        let out = {
            let paths = CompileScriptPaths {
                cc: &cc_name,
                tmp_so: &tmp_so,
                tmp_c: &tmp_c,
                published_c: &c_path,
                published_so: &so_path,
                lock: lock_path.as_deref(),
                log: &log_path,
            };
            let mut cmd = Command::new("/bin/sh");
            if unit_paths.is_empty() {
                cmd.arg("-c")
                    .arg(COMPILE_SCRIPT)
                    .args(compile_script_args(&paths, &flags));
            } else {
                cmd.arg("-c")
                    .arg(SPLIT_COMPILE_SCRIPT)
                    .args(split_script_args(&paths, &flags, &unit_paths));
            }
            // Own process group: a group-delivered signal (Ctrl-C on the
            // run, a harness killing its group) must not take the publish
            // down with it.
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let child = cmd
                .spawn()
                .map_err(|e| format!("spawn sh/cc: {e} (set VERYL_AOT_CC to override)"))?;
            // The shell, not us, owns the publish, so its pid is what tells
            // a waiter whether the lock is still live (see `owner_alive`).
            if let Some(lp) = &lock_path {
                let _ = fs::write(lp, format!("{}\n", child.id()));
            }
            child
                .wait_with_output()
                .map_err(|e| format!("wait sh/cc: {e}"))?
        };
        #[cfg(not(unix))]
        let out = {
            let mut cmd = Command::new(&cc_name);
            cmd.args(&flags).arg("-o").arg(&tmp_so).arg(&tmp_c);
            let out = cmd
                .output()
                .map_err(|e| format!("spawn cc: {e} (set VERYL_AOT_CC to override)"))?;
            if out.status.success() {
                // A racing peer publishes an equally valid file (same
                // source), so an overwrite either way is fine.
                let _ = fs::rename(&tmp_c, &c_path);
                fs::rename(&tmp_so, &so_path)
                    .map_err(|e| format!("rename {}: {}", tmp_so.display(), e))?;
            }
            out
        };
        if !out.status.success() {
            // The compiler's output went to `log_path`, not to our pipes, so
            // read it back; fall back to whatever the shell itself said.
            let diag = fs::read_to_string(&log_path)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).into_owned());
            // Leave the temp .c for inspection.
            return Err(format!(
                "cc {} failed: {}\n{}",
                tmp_c.display(),
                out.status,
                diag,
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
    let const_func: Option<FuncPtr> = unsafe { lib.get::<FuncPtr>(b"veryl_aot_eval_const\0") }
        .ok()
        .map(|s| *s);
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
    Ok(EmittedModule {
        func,
        const_func,
        _lib: lib,
    })
}

/// Compile one source and publish it, releasing the compile lock (`$lk`) at
/// the end — from the script, not just from `Drop`, because the shell outlives
/// a killed run and only it knows when the publish finished.  It runs on the
/// failure path too, and an empty `$lk` (an `Unlocked` ticket) skips it without
/// disturbing the exit status.
///
/// It also redirects its whole output to `$lg` up front.  The shell outlives
/// this process by design, but our pipes do not: once we exit, a `cc` that
/// writes a diagnostic gets SIGPIPE and dies, taking the artifact with it.
///
/// Read the parameters with [`compile_script_args`]: the `shift` count here and
/// that argument list must agree, or a compiler flag silently lands in the
/// slot after it and never reaches the compile.
#[cfg(unix)]
const COMPILE_SCRIPT: &str = r#"cc="$1"; tso="$2"; tc="$3"; pc="$4"; pso="$5"; lk="$6"; lg="$7"; mem="$8"; shift 8; exec > "$lg" 2>&1; if [ "$mem" != 0 ]; then ulimit -v "$mem" 2>/dev/null || true; fi; if "$cc" "$@" -o "$tso" "$tc"; then mv -f "$tc" "$pc"; rm -f "$lg"; mv -f "$tso" "$pso"; rc=0; else rm -f "$tso"; rc=1; fi; if [ -n "$lk" ]; then rm -f "$lk"; fi; exit $rc"#;

/// [`COMPILE_SCRIPT`] for a source split into units: compile them at once,
/// link, publish.  Same contract otherwise — one shell owns the publish and
/// the lock, and the whole source (`$tc`) is what lands beside the `.so`.
///
/// The compile flags arrive as one word-split parameter because the unit paths
/// take the variadic slot.  `-shared` belongs to the link, so the caller sends
/// two lists (see [`split_script_args`]).
///
/// The units are backgrounded together rather than fed to one `cc`: a single
/// invocation compiles them in sequence, which is the latency this exists to
/// remove.
#[cfg(unix)]
const SPLIT_COMPILE_SCRIPT: &str = r#"cc="$1"; tso="$2"; tc="$3"; pc="$4"; pso="$5"; lk="$6"; lg="$7"; mem="$8"; cf="$9"; shift 9; exec > "$lg" 2>&1; if [ "$mem" != 0 ]; then ulimit -v "$mem" 2>/dev/null || true; fi; ps=""; os=""; for u in "$@"; do "$cc" $cf -c "$u" -o "$u.o" & ps="$ps $!"; os="$os $u.o"; done; ok=1; for p in $ps; do wait "$p" || ok=0; done; if [ "$ok" = 1 ] && "$cc" -shared -fPIC -o "$tso" $os; then mv -f "$tc" "$pc"; rm -f "$lg" "$@" $os; mv -f "$tso" "$pso"; rc=0; else rm -f "$tso" $os; rc=1; fi; if [ -n "$lk" ]; then rm -f "$lk"; fi; exit $rc"#;

/// Address-space ceiling for the compiler, in KiB (`VERYL_AOT_C_MAX_MEM_MB`
/// gives it in MB; 0 disables).  The compile is detached on purpose — it has
/// to outlive the run to publish its `.so` — so nothing reclaims it if it
/// misbehaves.  Set well above a healthy whole-module compile.
fn compile_mem_limit_kb() -> u64 {
    std::env::var("VERYL_AOT_C_MAX_MEM_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(16 * 1024)
        * 1024
}

/// Paths [`COMPILE_SCRIPT`] works on.  Named rather than positional so the
/// four `.c`/`.so` slots cannot be transposed at the call site.
#[cfg(unix)]
struct CompileScriptPaths<'a> {
    cc: &'a str,
    tmp_so: &'a Path,
    tmp_c: &'a Path,
    published_c: &'a Path,
    published_so: &'a Path,
    lock: Option<&'a Path>,
    log: &'a Path,
}

/// Positional arguments for [`COMPILE_SCRIPT`], in the order it reads them.
/// `$0` is the shell's own name; the flags follow the seven it shifts away.
#[cfg(unix)]
fn compile_script_args(p: &CompileScriptPaths, flags: &[String]) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "sh".into(),
        p.cc.into(),
        p.tmp_so.as_os_str().to_os_string(),
        p.tmp_c.as_os_str().to_os_string(),
        p.published_c.as_os_str().to_os_string(),
        p.published_so.as_os_str().to_os_string(),
        p.lock
            .map(|q| q.as_os_str().to_os_string())
            .unwrap_or_default(),
        p.log.as_os_str().to_os_string(),
        compile_mem_limit_kb().to_string().into(),
    ];
    args.extend(flags.iter().map(std::ffi::OsString::from));
    args
}

/// [`compile_script_args`] for [`SPLIT_COMPILE_SCRIPT`]: the compile flags
/// become one word-split parameter and the unit paths take the variadic tail.
///
/// `-shared` is dropped here because these invocations produce objects; the
/// script passes it to the link itself.
#[cfg(unix)]
fn split_script_args(
    p: &CompileScriptPaths,
    flags: &[String],
    units: &[PathBuf],
) -> Vec<std::ffi::OsString> {
    let compile_flags: Vec<&str> = flags
        .iter()
        .map(String::as_str)
        .filter(|f| *f != "-shared")
        .collect();
    let mut args: Vec<std::ffi::OsString> = vec![
        "sh".into(),
        p.cc.into(),
        p.tmp_so.as_os_str().to_os_string(),
        p.tmp_c.as_os_str().to_os_string(),
        p.published_c.as_os_str().to_os_string(),
        p.published_so.as_os_str().to_os_string(),
        p.lock
            .map(|q| q.as_os_str().to_os_string())
            .unwrap_or_default(),
        p.log.as_os_str().to_os_string(),
        compile_mem_limit_kb().to_string().into(),
        compile_flags.join(" ").into(),
    ];
    args.extend(units.iter().map(|u| u.as_os_str().to_os_string()));
    args
}

/// Per-artifact compile lock, released on drop and by the compile script's
/// trailing `rm` (whichever happens first — the script outlives us when the
/// process is killed mid-compile).
struct CompileLock {
    path: PathBuf,
}

impl Drop for CompileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Outcome of trying to take the lock for one artifact hash.
enum CompileTicket {
    /// We own the lock: compile, then release.
    Owned(CompileLock),
    /// Another compiler published the artifact while we waited.
    Published,
    /// The lock file is unusable (unwritable cache dir, filesystem without
    /// `O_EXCL` semantics): compile unlocked, exactly as before the lock.
    Unlocked,
}

/// Backstop for a lock whose owner cannot be identified (the pid is not
/// written yet, or this is not unix); set above any real compile — the
/// largest source measured, 87 MB, takes ~4 min.  A provably dead owner is
/// taken over at once instead — see [`owner_alive`].
const COMPILE_LOCK_STALE: Duration = Duration::from_secs(600);

/// Is the compile shell recorded in a lock file still running?
///
/// The publishing shell outlives us on purpose, so its pid — not ours — is
/// what makes a lock meaningful.  Without this check a run killed mid-compile
/// would hold the artifact hostage for [`COMPILE_LOCK_STALE`].
/// `None` = no pid recorded yet; fall back to the age rule.
fn owner_alive(lock_path: &Path) -> Option<bool> {
    let pid: u32 = fs::read_to_string(lock_path).ok()?.trim().parse().ok()?;
    // Linux: procfs is authoritative and free.  Guarded by /proc/self so a
    // system without procfs doesn't read every pid as dead.
    if Path::new("/proc/self").exists() {
        return Some(Path::new(&format!("/proc/{pid}")).exists());
    }
    // Other unix: `kill -0` through the shell we already use, so this needs
    // no libc dependency.
    #[cfg(unix)]
    let alive = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -0 {pid} 2>/dev/null"))
        .status()
        .ok()
        .map(|s| s.success());
    #[cfg(not(unix))]
    let alive = None;
    alive
}

/// Take the compile lock for `hash`, waiting for the owner rather than
/// duplicating its work.
///
/// Identical sources hash to one `.so`, so every process and pool worker
/// reaching the compile would otherwise build the same translation unit
/// independently — four concurrent `cc1` jobs over one 87 MB source have been
/// observed within a single run, and concurrent runs duplicate that again.
/// Waiting costs nothing: `compile_source_in` blocks for the full `cc` either
/// way, so this only removes redundant work.
///
/// Declining instead of waiting would not be neutral: the caller's cell is a
/// `OnceLock`, so a worker that gives up leaves that handle on Cranelift for
/// the rest of the process.
fn acquire_compile_lock(cache_dir: &Path, hash: &str, so_path: &Path) -> CompileTicket {
    let path = cache_dir.join(format!("veryl_aot_{hash}.lock"));
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return CompileTicket::Owned(CompileLock { path }),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return CompileTicket::Unlocked,
        }
        if so_path.exists() {
            return CompileTicket::Published;
        }
        let dead = match owner_alive(&path) {
            Some(alive) => !alive,
            // Owner unknown (pid not written yet, or not unix): age it out.
            None => fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(|t| t.elapsed().unwrap_or_default() > COMPILE_LOCK_STALE)
                // A lock we cannot stat is gone or unreadable; retrying the
                // create resolves both.
                .unwrap_or(true),
        };
        if dead {
            if diag_enabled() {
                eprintln!(
                    "[aot_c] taking over abandoned compile lock {}",
                    path.display()
                );
            }
            let _ = fs::remove_file(&path);
            continue;
        }
        thread::sleep(Duration::from_millis(250));
    }
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

/// Ceiling on one statement's emitted C (`VERYL_AOT_C_MAX_STMT_MB`, 0
/// disables); over it the statement counts as uncovered.
///
/// A statement's emitted text is unbounded: a chain of conditionals inlines
/// into a single expression, and version-splitting a deeply predicated design
/// can grow one past what the compiler can hold.  Splitting the translation
/// unit or the chunk functions cannot bound that — it is ONE statement — so
/// refusing it is the only lever.  The default sits orders of magnitude above
/// what healthy designs emit per statement.
fn max_stmt_bytes() -> usize {
    std::env::var("VERYL_AOT_C_MAX_STMT_MB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(16)
        * 1024
        * 1024
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
         typedef uint64_t veryl_u64_ua __attribute__((__aligned__(1)));\n\
         typedef uint32_t veryl_u32_ua __attribute__((__aligned__(1)));\n\
         typedef uint16_t veryl_u16_ua __attribute__((__aligned__(1)));\n",
    );
    body.push_str(WIDEOPS_C_DECLS);
    body.push_str(WIDEOPS_C_INLINE);
    body.push('\n');

    // Const-cone split (default ON, VERYL_AOT_C_CONST_SKIP=0 opts out):
    // partition constant-input statements to the front BEFORE field-group
    // planning so the gather's atom ranges are computed on the final order.
    // The gather may still move statements across the prefix boundary
    // (a sink pairs a const def with its non-const reader); its moves are
    // dependency-checked and never earlier, so the prefix stays executable
    // in order — the re-count below just shrinks it to the run that is
    // still contiguously const, and a boundary-split sink pair reverts to
    // buffer traffic (the localize sets are recomputed per final chunk).
    let const_part: Option<(Vec<ProtoStatement>, usize, Vec<bool>)> = if const_skip_armed() {
        let unsafe_comb = CONST_UNSAFE_COMB.with(|b| b.borrow().clone());
        const_cone_partition(stmts, &unsafe_comb)
    } else {
        None
    };
    if std::env::var("VERYL_AOT_C_CONST_DIAG").as_deref() == Ok("1") {
        eprintln!(
            "[const_skip] armed={} stmts={} n_const={}",
            const_skip_armed(),
            stmts.len(),
            const_part.as_ref().map_or(0, |(_, n, _)| *n),
        );
    }
    let mut n_const = const_part.as_ref().map_or(0, |(_, n, _)| *n);
    let stmts: &[ProtoStatement] = const_part.as_ref().map_or(stmts, |(s, _, _)| s.as_slice());

    // Cone-gate segments and the const split compose: the split is a STABLE
    // partition, so a segment's non-const members stay contiguous in the
    // tail and its ranges just shift by the const counts.  A const statement
    // extracted OUT of a segment is sound to leave ungated — its run-once
    // output never changes, so neither the skip (which preserves it) nor the
    // replay (which rewrites the same value) can disturb it.  The field
    // gather below still cannot run: it permutes arbitrarily.
    let mut cone_segments: Vec<crate::ir::opt::cone_gate::ConeSegment> =
        CONE_SEGMENTS.with(|s| s.borrow().clone());
    if let Some((_, _, is_const)) = &const_part
        && !cone_segments.is_empty()
    {
        let mut cb = vec![0u32; is_const.len() + 1];
        for (i, &c) in is_const.iter().enumerate() {
            cb[i + 1] = cb[i] + c as u32;
        }
        let n_const_total = cb[is_const.len()] as usize;
        for s in &mut cone_segments {
            let lo_c = cb[s.stmt_lo.min(is_const.len())] as usize;
            let hi_c = cb[s.stmt_hi.min(is_const.len())] as usize;
            s.stmt_lo = n_const_total + s.stmt_lo - lo_c;
            s.stmt_hi = n_const_total + s.stmt_hi - hi_c;
        }
        cone_segments.retain(|s| s.stmt_lo < s.stmt_hi);
    }

    // Field-group roles and gathering: see plan_field_groups.  With cone
    // segments, the gather runs PER REGION (each segment and each gap
    // independently) so no statement crosses a segment edge: the region
    // permutations keep every segment's [stmt_lo, stmt_hi) intact, and the
    // roles merge by their position-independent (window, mask) keys — a key
    // two regions disagree on is dropped back to plain emission.
    let _field_roles = FieldRolesGuard;
    let plan = if cone_segments.is_empty() {
        plan_field_groups(stmts)
    } else {
        let mut bounds: Vec<usize> = cone_segments
            .iter()
            .flat_map(|s| [s.stmt_lo, s.stmt_hi])
            .filter(|&b| b <= stmts.len())
            .collect();
        bounds.push(0);
        bounds.push(stmts.len());
        bounds.sort_unstable();
        bounds.dedup();
        let mut roles: HashMap<(isize, u64), FieldRole> = HashMap::default();
        let mut conflicted: HashSet<(isize, u64)> = HashSet::default();
        let mut order: Vec<usize> = Vec::with_capacity(stmts.len());
        let mut atoms: Vec<(usize, usize)> = Vec::new();
        for w in bounds.windows(2) {
            let (lo, hi) = (w[0], w[1]);
            if lo >= hi {
                continue;
            }
            let p = plan_field_groups(&stmts[lo..hi]);
            for (k, v) in p.roles {
                match roles.get(&k) {
                    Some(&prev) if prev != v => {
                        conflicted.insert(k);
                    }
                    _ => {
                        roles.insert(k, v);
                    }
                }
            }
            atoms.extend(p.atoms.iter().map(|&(s, l)| (order.len() + s, l)));
            order.extend(p.order.iter().map(|&i| lo + i));
        }
        for k in &conflicted {
            roles.remove(k);
        }
        FieldPlan {
            roles,
            order,
            atoms,
        }
    };
    FIELD_ROLES.with(|r| *r.borrow_mut() = plan.roles.clone());
    let gathered: Option<Vec<ProtoStatement>> =
        (!plan.atoms.is_empty()).then(|| plan.order.iter().map(|&i| stmts[i].clone()).collect());
    if gathered.is_some() && n_const > 0 {
        // The gather may move statements across the prefix boundary in
        // either direction (a sink pairs a const def with its later
        // non-const reader).  Rather than reorder anything further, shrink
        // the prefix to the statements still contiguously const at the
        // front of the FINAL order — a pure re-count, so it is always
        // sound; statements pushed past an intruder just stay in the main
        // entry (a missed opportunity, not an error).
        let boundary = n_const;
        let mut k = 0usize;
        while k < plan.order.len() && plan.order[k] < boundary {
            k += 1;
        }
        n_const = k;
    }
    let stmts: &[ProtoStatement] = gathered.as_deref().unwrap_or(stmts);

    // Emit each chunk's stmts now so we can fail fast on unsupported.
    //
    // The chunk budget counts approximate CODE WEIGHT, not statements:
    // statement-level fusion concentrates many former statements' work into
    // one Assign, and a count-based split packs several of those into one
    // function whose register pressure defeats gcc.  A plain statement
    // costs 1, so the knob's meaning is unchanged for unfused code.
    fn expr_nodes(e: &ProtoExpression) -> usize {
        match e {
            ProtoExpression::Variable { .. } | ProtoExpression::Value { .. } => 1,
            ProtoExpression::Unary { x, .. } => 1 + expr_nodes(x),
            ProtoExpression::Binary { x, y, .. } => 1 + expr_nodes(x) + expr_nodes(y),
            ProtoExpression::Concatenation { elements, .. } => {
                1 + elements
                    .iter()
                    .map(|(e, ..)| 1 + expr_nodes(e))
                    .sum::<usize>()
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => 1 + expr_nodes(cond) + expr_nodes(true_expr) + expr_nodes(false_expr),
            ProtoExpression::DynamicVariable { index_expr, .. } => 2 + expr_nodes(index_expr),
            _ => 4,
        }
    }
    fn stmt_cost(s: &ProtoStatement) -> usize {
        match s {
            ProtoStatement::Assign(a) => 1 + expr_nodes(&a.expr) / 8,
            _ => 1,
        }
    }
    // Cone-gate segment edges force chunk boundaries so the dispatcher can
    // guard a segment as a whole number of chunk calls.
    let seg_bounds: Vec<usize> = {
        let mut v: Vec<usize> = cone_segments
            .iter()
            .flat_map(|s| [s.stmt_lo, s.stmt_hi])
            .filter(|&b| b > 0 && b < stmts.len())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let clamp_to_seg = |start: usize, end: usize| -> usize {
        let i = seg_bounds.partition_point(|&b| b <= start);
        match seg_bounds.get(i) {
            Some(&b) if b < end => b,
            _ => end,
        }
    };
    let chunks: Vec<&[ProtoStatement]> =
        if (chunk_size == 0 || stmts.len() <= chunk_size) && seg_bounds.is_empty() {
            if n_const > 0 && n_const < stmts.len() {
                vec![&stmts[..n_const], &stmts[n_const..]]
            } else {
                vec![stmts]
            }
        } else {
            // Never cut inside a gathered group — splitting one puts the
            // accumulating stores in different functions and gcc can no longer
            // keep the window in a register.
            let mut chunks = Vec::new();
            let (mut start, mut ai) = (0usize, 0usize);
            while start < stmts.len() {
                let mut end = start;
                if chunk_size == 0 {
                    end = stmts.len();
                } else {
                    let mut cost = 0usize;
                    while end < stmts.len() && cost < chunk_size {
                        cost += stmt_cost(&stmts[end]);
                        end += 1;
                    }
                }
                while ai < plan.atoms.len() && plan.atoms[ai].0 + plan.atoms[ai].1 <= end {
                    ai += 1;
                }
                if ai < plan.atoms.len() && plan.atoms[ai].0 < end {
                    end = if plan.atoms[ai].0 > start {
                        plan.atoms[ai].0
                    } else {
                        plan.atoms[ai].0 + plan.atoms[ai].1
                    };
                }
                // Force a boundary at the const prefix so the const chunks can
                // be routed to the run-once entry.  A field-group atom is never
                // split here (the co-writer rule makes its members all-const or
                // all-demoted together); a sink atom straddling the boundary is
                // split, which only costs the pair its locality (the localize
                // sets are recomputed per final chunk).
                if start < n_const && end > n_const {
                    end = n_const;
                }
                let end = clamp_to_seg(start, end);
                chunks.push(&stmts[start..end]);
                start = end;
            }
            chunks
        };
    let const_chunks = {
        let mut acc = 0usize;
        let mut k = 0usize;
        for c in &chunks {
            if acc + c.len() <= n_const {
                acc += c.len();
                k += 1;
            } else {
                break;
            }
        }
        k
    };
    if std::env::var("VERYL_AOT_C_CONST_DIAG").as_deref() == Ok("1") {
        eprintln!(
            "[const_skip] post-gather n_const={n_const} const_chunks={const_chunks} chunks={}",
            chunks.len(),
        );
    }
    if std::env::var("VERYL_AOT_C_SINK_DIAG").as_deref() == Ok("1") {
        sink_census(stmts, &chunks);
    }
    // Chunk-local intermediate localization (VERYL_AOT_C_LOCALIZE): per chunk,
    // the comb offsets that are written by one clean top-level scalar Assign in
    // that chunk and read only there (and not blocklisted) become C locals
    // instead of comb_values round-trips.  Empty sets when the knob is off.
    LAST_LOCALIZED_BYTES.with(|b| b.borrow_mut().clear());
    // The cone-gate state regions live in comb_values but only the AOT side
    // writes them — the validate dual-run must skip them like localized bytes.
    if !cone_segments.is_empty() {
        LAST_LOCALIZED_BYTES.with(|b| {
            let mut v = b.borrow_mut();
            for s in &cone_segments {
                let be: usize = s.backedge.iter().map(|&(a, x)| (x - a) as usize).sum();
                let pb: usize = s.compare_pre.iter().map(|&(a, x)| (x - a) as usize).sum();
                let cb: usize = s.compare.iter().map(|&(_, a, x)| (x - a) as usize).sum();
                let rb: usize = s.replay.iter().map(|&(a, x)| (x - a) as usize).sum();
                let len = (8 + be + pb + cb + rb).next_multiple_of(8);
                v.push((s.state_off as isize, len));
            }
        });
    }
    let localize_sets: Vec<HashSet<isize>> = if localize_armed() {
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
        vec![HashSet::default(); chunks.len()]
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

    // Static SLP policy: scalar-dominated sources lose to gcc's SLP
    // vectorizer — it bundles same-shape statements on SCATTERED words, so
    // the vector is assembled with movd/pinsrd gathers that cost more than
    // the scalar ops they replace (fewer cycles but MORE instructions: the
    // gather's port pressure is the loss, so instruction counts mis-rank
    // this axis).  Wide-data designs vectorize real adjacent words and
    // keep winning.  The density of vw_* calls over emitted statements
    // separates the two cleanly — calibrated on reference designs:
    // scalar CPU cores measure 0.00–0.05 (SLP loses), wide-datapath
    // designs 0.18–0.28 (SLP wins), so 0.1 sits between with ~2x margin
    // each side.  VERYL_AOT_C_COMB_SLP=1 pins SLP on everywhere, =0
    // forces it off.
    // One emitted statement per line (see the chunk loop above), so line
    // count is the statement count; counting ';' would inflate the
    // denominator on multi-';' statement expressions (the overflow guard
    // alone carries five) and shift the ratio scale the threshold was
    // calibrated on.
    let (vw_calls, stmt_count) = chunk_bodies.iter().fold((0usize, 0usize), |(v, s), cb| {
        (v + cb.matches("vw_").count(), s + cb.lines().count())
    });
    let comb_noslp = match std::env::var("VERYL_AOT_C_COMB_SLP").as_deref() {
        Ok("1") => false,
        Ok("0") => true,
        _ => (vw_calls as f64) < 0.1 * (stmt_count as f64),
    };
    if std::env::var("VERYL_AOT_C_SLP_DIAG").as_deref() == Ok("1") {
        eprintln!(
            "[aot-c slp] vw_calls={vw_calls} stmts={stmt_count} ratio={:.4} noslp={comb_noslp}",
            (vw_calls as f64) / (stmt_count.max(1) as f64)
        );
    }

    if chunks.len() == 1 && const_chunks == 0 && cone_segments.is_empty() {
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
        // Const-prefix chunks go to a separate run-once entry: their inputs
        // never change, so the runtime (Ir::const_cone_done) calls this once per
        // simulator instance and the main entry skips them every settle.
        if const_chunks > 0 {
            body.push_str(
                "__attribute__((visibility(\"default\")))\n\
                 void veryl_aot_eval_const(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log, intptr_t ff_delta) {\n",
            );
            for i in 0..const_chunks {
                body.push_str(&format!(
                    "    veryl_aot_chunk_{i}(ff_values, comb_values, write_log);\n",
                ));
            }
            body.push_str("}\n");
        }
        // Cone-gate guards: map each segment to its chunk-call range (chunk
        // boundaries were forced at segment edges above) and emit its compare
        // helper.  A segment whose edges did not land on chunk boundaries is
        // left unguarded — safe, just unskippable.
        let chunk_starts: Vec<usize> = {
            let mut v = Vec::with_capacity(chunks.len() + 1);
            let mut p = 0usize;
            for c in &chunks {
                v.push(p);
                p += c.len();
            }
            v.push(p);
            v
        };
        let cg_dbg = std::env::var("VERYL_CONE_GATE_DIAG").as_deref() == Ok("1");
        let guards: Vec<(usize, usize, &crate::ir::opt::cone_gate::ConeSegment)> = cone_segments
            .iter()
            .filter_map(|s| {
                let k1 = chunk_starts.iter().position(|&q| q == s.stmt_lo)?;
                let k2 = chunk_starts.iter().position(|&q| q == s.stmt_hi)?;
                (k1 < k2 && k1 >= const_chunks).then_some((k1, k2, s))
            })
            .collect();
        for (gi, &(_, _, s)) in guards.iter().enumerate() {
            let mut f = format!(
                "static int cg_cmp_{gi}(const uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values) {{\n"
            );
            let be: usize = s.backedge.iter().map(|&(a, b)| (b - a) as usize).sum();
            let pb: usize = s.compare_pre.iter().map(|&(a, b)| (b - a) as usize).sum();
            let pre_shadow_abs = s.state_off as usize + 8 + be;
            let shadow_abs = s.state_off as usize + 8 + be + pb;
            let mut acc = 0usize;
            for &(a, b) in &s.compare_pre {
                let l = (b - a) as usize;
                f.push_str(&format!(
                    "    if (__builtin_memcmp(comb_values + {a:#x}, comb_values + {sh:#x}, {l})) return 0;\n",
                    sh = pre_shadow_abs + acc,
                ));
                acc += l;
            }
            let mut acc = 0usize;
            for (ri, &(is_ff, a, b)) in s.compare.iter().enumerate() {
                let l = (b - a) as usize;
                let buf = if is_ff { "ff_values" } else { "comb_values" };
                let stash = if cg_dbg {
                    format!(
                        "{{ comb_values[{st}] = {ri_b}; return 0; }}",
                        st = s.state_off as usize + 3,
                        ri_b = (ri % 255) + 1,
                    )
                } else {
                    "return 0;".to_string()
                };
                f.push_str(&format!(
                    "    if (__builtin_memcmp({buf} + {a:#x}, comb_values + {sh:#x}, {l})) {stash}\n",
                    sh = shadow_abs + acc,
                ));
                acc += l;
            }
            f.push_str("    return 1;\n}\n\n");
            body.push_str(&f);
        }
        body.push_str(
            "__attribute__((visibility(\"default\")))\n\
             void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log, intptr_t ff_delta) {\n",
        );
        // Emit-time debug (VERYL_CONE_GATE_DIAG=1 at emit): per-segment
        // skip/run counters printed every ~1M evals.  Statics are fine for a
        // debug build of the artifact.
        if cg_dbg && !guards.is_empty() {
            body.push_str(&format!(
                "    static unsigned long long cg_sk[{n}], cg_rn[{n}]; static unsigned long long cg_calls;\n\
                 \x20   static const unsigned cg_stoff[{n}] = {{{offs}}};\n\
                 \x20   uint8_t *cg_st[{n}]; for (int z = 0; z < {n}; z++) cg_st[z] = comb_values + cg_stoff[z];\n\
                 \x20   if ((++cg_calls & 0x3fff) == 0) {{\n\
                 \x20     __builtin_printf(\"[cg] evals=%llu\", cg_calls);\n\
                 \x20     for (int z = 0; z < {n}; z++) __builtin_printf(\" %llu/%llu:c%u:f%u\", cg_sk[z], cg_rn[z], (unsigned)cg_st[z][1], (unsigned)cg_st[z][3]);\n\
                 \x20     __builtin_printf(\"\\n\");\n\
                 \x20   }}\n",
                n = guards.len(),
                offs = guards
                    .iter()
                    .map(|&(_, _, s)| format!("{:#x}", s.state_off))
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let mut gi = 0usize;
        let mut i = const_chunks;
        while i < chunks.len() {
            if gi < guards.len() && guards[gi].0 == i {
                let (k1, k2, s) = guards[gi];
                let st = s.state_off as usize;
                let be: usize = s.backedge.iter().map(|&(a, b)| (b - a) as usize).sum();
                let pb: usize = s.compare_pre.iter().map(|&(a, b)| (b - a) as usize).sum();
                let cb: usize = s.compare.iter().map(|&(_, a, b)| (b - a) as usize).sum();
                let prerun_abs = st + 8;
                let pre_shadow_abs = st + 8 + be;
                let shadow_abs = st + 8 + be + pb;
                let replay_abs = st + 8 + be + pb + cb;
                // After auto-off the shadows are never consulted again, so
                // the whole maintenance path (pre-run snapshots, convergence
                // check, shadow/replay refresh) is skipped too — an off
                // segment costs exactly the plain chunk calls, mirroring the
                // Rust-side `before_run`/`refresh` early returns.
                body.push_str(&format!(
                    "    {{ uint8_t *cgst = comb_values + {st:#x};\n\
                     \x20     int cg_run = 1;\n\
                     \x20     int cg_off = cgst[2];\n\
                     \x20     if (!cg_off && cgst[0] && cgst[1]) {{\n\
                     \x20       if (cg_cmp_{gi}(ff_values, comb_values)) {{\n\
                     \x20         cg_run = 0;\n\
                     \x20         {{ uint32_t cg_stk; __builtin_memcpy(&cg_stk, cgst + 4, 4);\n\
                     \x20           cg_stk = cg_stk > {decay}u ? cg_stk - {decay}u : 0;\n\
                     \x20           __builtin_memcpy(cgst + 4, &cg_stk, 4); }}\n",
                    decay = s.off_decay,
                ));
                if cg_dbg {
                    body.push_str(&format!("          cg_sk[{gi}]++;\n"));
                }
                let mut acc = 0usize;
                for &(a, b) in &s.replay {
                    let l = (b - a) as usize;
                    body.push_str(&format!(
                        "          __builtin_memcpy(comb_values + {a:#x}, comb_values + {src:#x}, {l});\n",
                        src = replay_abs + acc,
                    ));
                    acc += l;
                }
                body.push_str(
                    "        } else {\n\
                     \x20         uint32_t cg_stk; __builtin_memcpy(&cg_stk, cgst + 4, 4);\n\
                     \x20         if (++cg_stk >= 1024u) cgst[2] = 1;\n\
                     \x20         __builtin_memcpy(cgst + 4, &cg_stk, 4);\n\
                     \x20       }\n\
                     \x20     }\n\
                     \x20     if (cg_run) {\n\
                     \x20       if (!cg_off) {\n",
                );
                let mut acc = 0usize;
                for &(a, b) in &s.backedge {
                    let l = (b - a) as usize;
                    body.push_str(&format!(
                        "        __builtin_memcpy(comb_values + {dst:#x}, comb_values + {a:#x}, {l});\n",
                        dst = prerun_abs + acc,
                    ));
                    acc += l;
                }
                let mut acc = 0usize;
                for &(a, b) in &s.compare_pre {
                    let l = (b - a) as usize;
                    body.push_str(&format!(
                        "        __builtin_memcpy(comb_values + {dst:#x}, comb_values + {a:#x}, {l});\n",
                        dst = pre_shadow_abs + acc,
                    ));
                    acc += l;
                }
                body.push_str("        }\n");
                for k in k1..k2 {
                    body.push_str(&format!(
                        "        veryl_aot_chunk_{k}(ff_values, comb_values, write_log);\n",
                    ));
                }
                body.push_str("        if (!cg_off) {\n        { int cg_conv = 1;\n");
                let mut acc = 0usize;
                for (ri, &(a, b)) in s.backedge.iter().enumerate() {
                    let l = (b - a) as usize;
                    if cg_dbg {
                        body.push_str(&format!(
                            "          if (cg_conv && __builtin_memcmp(comb_values + {pre:#x}, comb_values + {a:#x}, {l})) {{ cg_conv = 0; cgst[3] = {ri_b}; \
                             if ((cg_calls & 0x3fff) == 1) __builtin_printf(\"[cgconv] seg_state={st:#x} range={ri} off={a:#x} pre=%016llx post=%016llx\\n\", *(unsigned long long*)(comb_values + {pre:#x}), *(unsigned long long*)(comb_values + {a:#x})); }}\n",
                            pre = prerun_abs + acc,
                            ri_b = (ri % 255) + 1,
                            st = s.state_off,
                            ri = ri,
                        ));
                    } else {
                        body.push_str(&format!(
                            "          cg_conv = cg_conv && !__builtin_memcmp(comb_values + {pre:#x}, comb_values + {a:#x}, {l});\n",
                            pre = prerun_abs + acc,
                        ));
                    }
                    acc += l;
                }
                body.push_str("          cgst[1] = (uint8_t)cg_conv; }\n");
                let mut acc = 0usize;
                for &(is_ff, a, b) in &s.compare {
                    let l = (b - a) as usize;
                    let buf = if is_ff { "ff_values" } else { "comb_values" };
                    body.push_str(&format!(
                        "        __builtin_memcpy(comb_values + {dst:#x}, {buf} + {a:#x}, {l});\n",
                        dst = shadow_abs + acc,
                    ));
                    acc += l;
                }
                let mut acc = 0usize;
                for &(a, b) in &s.replay {
                    let l = (b - a) as usize;
                    body.push_str(&format!(
                        "        __builtin_memcpy(comb_values + {dst:#x}, comb_values + {a:#x}, {l});\n",
                        dst = replay_abs + acc,
                    ));
                    acc += l;
                }
                if cg_dbg {
                    body.push_str(&format!("        cg_rn[{gi}]++;\n"));
                }
                body.push_str("        cgst[0] = 1;\n        }\n      }\n    }\n");
                gi += 1;
                i = k2;
            } else {
                body.push_str(&format!(
                    "    veryl_aot_chunk_{i}(ff_values, comb_values, write_log);\n",
                ));
                i += 1;
            }
        }
        body.push_str("}\n");
    }
    if comb_noslp {
        // The marker line is the cheapest way to carry the verdict to
        // compile_source (which only sees the source text); it also lands
        // in the cache key, so flipping the policy re-keys the .so.
        body = body.replacen(
            "// AOT-C generated; do not edit.",
            "// AOT-C generated (noslp); do not edit.",
            1,
        );
    }
    Some(body)
}

/// One terminated C statement from a `ProtoStatement`.  `None` if the
/// variant or its substructures aren't emittable, or if the result exceeds
/// [`max_stmt_bytes`].
pub fn emit_stmt(stmt: &ProtoStatement) -> Option<String> {
    let out = emit_stmt_inner(stmt)?;
    let cap = max_stmt_bytes();
    if cap != 0 && out.len() > cap {
        if diag_enabled() {
            eprintln!(
                "[aot_c] one statement emits {} B, over the {} B ceiling; declining AOT-C",
                out.len(),
                cap
            );
        }
        return None;
    }
    Some(out)
}

fn emit_stmt_inner(stmt: &ProtoStatement) -> Option<String> {
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
            // FF stores stay on Cranelift because emit_event_ff_assign does
            // not sign-extend; comb stores are covered at every width.
            if se_from.is_some() && a.dst.is_ff() {
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
            // A runtime-indexed bit-slice store. A ≤64-bit dst is the scalar
            // RMW below; a wide (>64-bit) dst is handled here because that path
            // shifts the mask in u64 and so can't reach a field crossing the
            // 64-bit word boundary (`logic<N, W>[idx] = v` rows).
            if let Some(dyn_sel) = &a.dynamic_select {
                if a.dst_width == 0 {
                    return None;
                }
                if a.dst_width > 64 {
                    // A static bit-select / rhs_select / sign-extend combined
                    // with the dynamic index isn't modelled here — bail those.
                    if a.select.is_some() || eff_rhs_select.is_some() || se_from.is_some() {
                        return None;
                    }
                    let ew = dyn_sel.elem_width;
                    let ne = dyn_sel.num_elements;
                    let win = dyn_sel.window;
                    let VarOffset::Comb(store_off) = a.dst else {
                        return None;
                    };
                    if store_off < 0 || ew == 0 || win == 0 || ne == 0 {
                        return None;
                    }
                    let nb = native_bytes(a.dst_width);
                    let nw = wide_words(nb);
                    if win > nb * 8 {
                        return None;
                    }
                    let dst = format!("(uint8_t*)(comb_values + {store_off:#x})");
                    let max_idx = ne - 1;
                    let idx = emit_expr(&dyn_sel.index_expr)?;
                    let mut pre = String::new();
                    // rhs value (masked to `win` below) as an nb-byte buffer.
                    let r = emit_wide_operand(eff_expr, nb, &mut pre)?;
                    let wmsk = next_wide_tmp(); // mask_range = fill_ones(win) << _sh
                    let keep = next_wide_tmp(); // widthmask & ~mask_range
                    let widm = next_wide_tmp(); // fill_ones(dst_width)
                    let srcsh = next_wide_tmp();
                    let newv = next_wide_tmp();
                    // Mirror AssignStatement::eval_step's dynamic_select +
                    // Value::assign(beg=_sh+win-1, end=_sh) EXACTLY, including
                    // the out-of-width spill (no final width clamp), so AOT and
                    // the interpreter stay byte-identical.
                    return Some(format!(
                        "{{ {pre}uint64_t _di_raw = (uint64_t)({idx}); \
                            uint64_t _di = _di_raw < {max_idx} ? _di_raw : {max_idx}; \
                            uint64_t _sh = _di * {ew}ull; \
                            uint64_t _w{wmsk}[{nw}]; \
                            vw_fill_ones((uint8_t*)_w{wmsk}, (const uint8_t*)0, {pkw}u); \
                            vw_shl((uint8_t*)_w{wmsk}, (const uint8_t*)_w{wmsk}, _sh, {nb}u); \
                            uint64_t _w{widm}[{nw}]; \
                            vw_fill_ones((uint8_t*)_w{widm}, (const uint8_t*)0, {pkd}u); \
                            uint64_t _w{keep}[{nw}]; \
                            vw_band_not((uint8_t*)_w{keep}, (const uint8_t*)_w{widm}, (const uint8_t*)_w{wmsk}, {nb}u); \
                            uint64_t _w{srcsh}[{nw}]; \
                            vw_copy((uint8_t*)_w{srcsh}, {src}, {nb}u); \
                            vw_apply_mask((uint8_t*)_w{srcsh}, (const uint8_t*)0, {pkw}u); \
                            vw_shl((uint8_t*)_w{srcsh}, (const uint8_t*)_w{srcsh}, _sh, {nb}u); \
                            vw_band((uint8_t*)_w{srcsh}, (const uint8_t*)_w{srcsh}, (const uint8_t*)_w{wmsk}, {nb}u); \
                            uint64_t _w{newv}[{nw}]; \
                            vw_band((uint8_t*)_w{newv}, {dst}, (const uint8_t*)_w{keep}, {nb}u); \
                            vw_bor((uint8_t*)_w{newv}, (const uint8_t*)_w{newv}, (const uint8_t*)_w{srcsh}, {nb}u); \
                            vw_copy({dst}, (const uint8_t*)_w{newv}, {nb}u); }}",
                        pkw = wpack(nb, win),
                        pkd = wpack(nb, a.dst_width),
                        src = r.addr,
                        dst = dst,
                    ));
                }
            }
            // Wide comb store via the wide-op helper table.  Two cases route
            // here: (a) dst_width > 128 (never a C scalar); (b) a 65-128-bit
            // dst whose RHS `builds_wide_pointer` — a wide-pointer result (e.g.
            // a wide shift over a >128-bit operand truncated to 128) that the
            // `__uint128_t` scalar path below (emit_expr_root) can't produce.
            // A 65-128-bit dst with a plain C-scalar RHS still takes the scalar
            // path.  A (bit-)select store IS emitted here (scalar fast path for
            // <=64-bit fields, full wide RMW otherwise) and so is a plain
            // rhs_select (field extract + store); the rhs_select + dst-select
            // combination stays on Cranelift.
            if a.dst_width > 128 || (a.dst_width > 64 && eff_expr.builds_wide_pointer()) {
                // A sign-extending RHS combined with a dst bit-select isn't
                // modelled here — only the plain-store arm sign-extends.
                if se_from.is_some() && a.select.is_some() {
                    return None;
                }
                let VarOffset::Comb(store_off) = a.dst else {
                    return None;
                };
                if store_off < 0 {
                    return None;
                }
                let nb = native_bytes(a.dst_width);
                let nw = wide_words(nb);
                let dst = format!("(uint8_t*)(comb_values + {store_off:#x})");
                let dmask = wpack(nb, a.dst_width);
                // Non-foldable rhs_select (rhs isn't a plain variable):
                // extract `value.select(rhs_hi, rhs_lo)` from the wide RHS,
                // then store the field — plain, or RMW into a dst bit-select.
                if let Some((rhs_hi, rhs_lo)) = eff_rhs_select {
                    let mut pre = String::new();
                    let f = emit_wide_rhs_field(eff_expr, rhs_hi, rhs_lo, &mut pre)?;
                    if let Some((hi, lo)) = a.select {
                        // Resize the field to the dst size class, then run
                        // the same wide RMW as the select-only arm below.
                        let nbits2 = hi.checked_sub(lo)?.checked_add(1)?;
                        if nbits2 == 0 || lo + nbits2 > nb * 8 {
                            return None;
                        }
                        let t = next_wide_tmp();
                        let cnb = f.nb.min(nb);
                        pre.push_str(&format!(
                            "uint64_t _w{t}[{nw}] = {{0}}; \
                             vw_copy((uint8_t*)_w{t}, {src}, {cnb}u); ",
                            src = f.addr,
                        ));
                        return Some(emit_wide_select_rmw_store(
                            &format!("((uint8_t*)_w{t})"),
                            pre,
                            &dst,
                            nw,
                            lo,
                            nbits2,
                            a.dst_width,
                        ));
                    }
                    let store = if nb <= f.nb {
                        format!("vw_copy({dst}, {src}, {nb}u); ", src = f.addr)
                    } else {
                        format!(
                            "__builtin_memset({dst}, 0, {nb}); \
                             vw_copy({dst}, {src}, {fnb}u); ",
                            src = f.addr,
                            fnb = f.nb,
                        )
                    };
                    return Some(format!(
                        "{{ {pre}{store}vw_apply_mask({dst}, (const uint8_t*)0, {dmask}u); }}"
                    ));
                }
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
                    // General multi-word field — full wide RMW; see
                    // emit_wide_select_rmw_store.
                    let mut pre = String::new();
                    let r = emit_wide_operand(eff_expr, nb, &mut pre)?;
                    return Some(emit_wide_select_rmw_store(
                        &r.addr,
                        pre,
                        &dst,
                        nw,
                        lo,
                        nbits,
                        a.dst_width,
                    ));
                }
                // Bare signed RHS narrower than the wide destination:
                // sign-extend at the store (value.expand(dst_width, true)).
                if let Some(w) = se_from {
                    let mut pre = String::new();
                    let r = emit_wide_operand(eff_expr, native_bytes(w).max(8), &mut pre)?;
                    return Some(format!(
                        "{{ {pre}vw_sext_copy({dst}, {src}, {w}u, {nb}u); \
                            vw_apply_mask({dst}, (const uint8_t*)0, {dmask}u); }}",
                        src = r.addr,
                    ));
                }
                // Wide ternary RHS: select word-by-word straight into the
                // destination.  Per-word forward select is alias-safe: an
                // arm equal to the destination reads each word before that
                // word is written.  The both-signed-narrower shape keeps the
                // generic path (its arms re-extend through a fresh
                // temporary).
                if let ProtoExpression::Ternary {
                    cond,
                    true_expr,
                    false_expr,
                    width: tw,
                    ..
                } = eff_expr
                    && *tw == a.dst_width
                {
                    let needs_sext = true_expr.expr_context().signed
                        && false_expr.expr_context().signed
                        && true_expr.width() > 0
                        && false_expr.width() > 0
                        && (true_expr.width() < *tw || false_expr.width() < *tw);
                    if !needs_sext && let Some(c) = emit_expr(cond) {
                        let mut pre = String::new();
                        if let Some(t_ref) = emit_wide_operand(true_expr, nb, &mut pre)
                            && let Some(f_ref) = emit_wide_operand(false_expr, nb, &mut pre)
                        {
                            let t = next_wide_tmp();
                            return Some(format!(
                                "{{ {pre}int _c{t} = (({c}) != 0); \
                                 for (int _i{t} = 0; _i{t} < {nw}; _i{t}++) \
                                 VW_WR({dst}, _i{t}, _c{t} ? \
                                 ((const veryl_u64_ua*)({tp}))[_i{t}] : \
                                 ((const veryl_u64_ua*)({fp}))[_i{t}]); \
                                 vw_apply_mask({dst}, (const uint8_t*)0, {dmask}u); }}",
                                nw = nb / 8,
                                tp = t_ref.addr,
                                fp = f_ref.addr,
                            ));
                        }
                    }
                }
                // Wide concat RHS: assemble the elements straight into the
                // destination (one |= per element word), no `_w` temporary.
                // Self-reading concats (the partial-coverage fusion's
                // `x = {x[7:7], b, a}`) keep the temporary form: its
                // read-before-write ordering is what makes them correct.
                if let ProtoExpression::Concatenation {
                    elements,
                    width: cw,
                    ..
                } = eff_expr
                    && *cw == a.dst_width
                {
                    // The into-form zeroes the destination before the elements
                    // evaluate, and the compact gather hides dynamic reads of
                    // an array's middle ones.
                    if !eff_expr.reads_offset(a.dst) {
                        let mut pre = String::new();
                        if emit_wide_concat_into(elements, *cw, nb, &dst, &mut pre).is_some() {
                            return Some(format!("{{ {pre}}}"));
                        }
                    }
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
            // A ≤64-bit destination fed by a wide-pointer RHS — SV's
            // truncating `assign narrow = wide_op;`: materialize the RHS and
            // store its low word masked to the destination width.
            if a.dst_width > 0
                && a.dst_width <= 64
                && eff_expr.builds_wide_pointer()
                && eff_rhs_select.is_none()
                && a.select.is_none()
                && a.dynamic_select.is_none()
                && se_from.is_none()
            {
                let VarOffset::Comb(store_off) = a.dst else {
                    return None;
                };
                if store_off < 0 {
                    return None;
                }
                let nb = native_bytes(a.dst_width);
                let cty = native_c_type(nb)?;
                let src_w = eff_expr.width();
                if src_w == 0 {
                    return None;
                }
                let mut pre = String::new();
                let r = emit_wide_operand(eff_expr, native_bytes(src_w), &mut pre)?;
                let dwmask = width_mask(a.dst_width);
                // A localized dst is read back through its C local, not
                // comb_values — the value must land in the local.
                if is_localized(store_off) {
                    return Some(format!(
                        "{{ {pre}{nm} = (uint64_t)(VW_RD({src}, 0) & 0x{dwmask:x}ULL); }}",
                        nm = local_name(store_off),
                        src = r.addr,
                    ));
                }
                return Some(format!(
                    "{{ {pre}*(({cty}*)(comb_values + {store_off:#x})) = \
                       ({cty})(VW_RD({src}, 0) & 0x{dwmask:x}ULL); }}",
                    src = r.addr,
                ));
            }
            let nb = native_bytes(a.dst_width);
            let cty = native_c_type(nb)?;
            // Compute the rhs after rhs_select extraction (mirrors
            // AssignStatement::eval_step's `value.select(beg, end)`).
            // A wide-pointer RHS value is not a C scalar, but a ≤64-bit
            // rhs_select field of it is.  Scalar emit is tried first:
            // building a wide pointer does not mean scalar emit fails (a
            // narrow dynamic select on a >128-bit var is a C scalar too).
            let (rhs_raw, eff_rhs_select) = if let Some(s) = emit_expr_root(eff_expr) {
                (s, eff_rhs_select)
            } else if eff_expr.builds_wide_pointer() {
                let (rhs_hi, rhs_lo) = eff_rhs_select?;
                let nbits = rhs_hi.checked_sub(rhs_lo)?.checked_add(1)?;
                if nbits > 64 {
                    return None;
                }
                let mut pre = String::new();
                let f = emit_wide_rhs_field(eff_expr, rhs_hi, rhs_lo, &mut pre)?;
                (
                    format!("({{ {pre}(uint64_t)VW_RD({addr}, 0); }})", addr = f.addr),
                    None,
                )
            } else {
                return None;
            };
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
            // Clean-store elision (see expr_emits_clean): the stores below
            // re-mask to dst_width only to canonicalize a dirty RHS.  Only
            // the bare form qualifies — a sign-extending store dirties
            // [w..dst_width) by design, and an rhs_select extraction is
            // handled by its own field mask.
            let rhs_clean = clean_elide()
                && se_from.is_none()
                && eff_rhs_select.is_none()
                && eff_expr.width() <= a.dst_width
                && expr_emits_clean(eff_expr);
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
                    if let Some(narrow) = narrow_field_store(&rhs_str, buf, store_off, lo, nbits) {
                        return Some(narrow);
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
                let val = if a.dst_width < native_bits && a.dst_width > 0 && !rhs_clean {
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
                if a.dst_width > 64 && a.dst_width < native_bits && !rhs_clean {
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
                } else if a.dst_width < native_bits && a.dst_width > 0 && !rhs_clean {
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
            // Wide (>64-bit) dynamic-indexed comb store via the wide-op
            // helper table.  A `var` array written by runtime index inside
            // always_ff whose ff_log_base_current_offset is None maps to the
            // comb buffer, so eval_step writes DIRECTLY to `base + stride*idx`
            // with no write-log push.  Mirror that byte for byte (RMW for
            // select, copy+mask for full).  Covers 65-128-bit elements too
            // (native_bytes/vw_* are width-agnostic); the scalar path below
            // then only ever sees a ≤64-bit dst.
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
                let nw = wide_words(nb);
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
                } else {
                    // Full element write: copy then mask in the destination
                    // (never the source, which may alias a flat-buffer read).
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
fn bitmerge_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_AOT_C_BITMERGE").as_deref() != Ok("0"))
}

/// Bit-test merging: a 1-bit And/Or tree whose leaves
/// test single bits of ONE variable folds to a masked compare on a single
/// full-width load — `!a[11] & a[10] & … & a[0]` becomes
/// `(a & 0xfff) == 0x7ff` (3 ops for what was 2 ops per bit plus the
/// joins).  gcc/clang never rebuild this shape from the per-bit form
/// (the shifts point different ways), so the emitter folds it while the
/// Cranelift arm keeps the expression form — VALIDATE checks the fold
/// byte-for-byte.  Census on a reference dual-thread CPU core: 3,021
/// same-variable groups, ≈26% of static operators (the small scalar core
/// has almost none).
///
/// Returns `None` when no group of ≥2 same-variable bit tests exists —
/// the normal expression path then applies.  Mixed leaves keep their
/// usual emission and join the folded groups with the tree's operator
/// (And/Or are commutative and associative, so regrouping is sound).
/// A negated leaf under Or, and any bit claimed both positive and
/// negated, stay unfolded (the masked-compare form cannot express them).
fn emit_bit_test_merge(expr: &ProtoExpression) -> Option<String> {
    let ProtoExpression::Binary { op, .. } = expr else {
        return None;
    };
    let op = *op;
    // Flatten the same-op 1-bit tree.
    fn flatten<'a>(e: &'a ProtoExpression, op: Op, out: &mut Vec<&'a ProtoExpression>) {
        match e {
            ProtoExpression::Binary {
                x,
                op: o,
                y,
                expr_context,
                ..
            } if *o == op && expr_context.width == 1 && !expr_context.signed => {
                flatten(x, op, out);
                flatten(y, op, out);
            }
            _ => out.push(e),
        }
    }
    // A single-bit test of a ≤64-bit variable: (is_ff, base, bit, full_width).
    fn as_bit_test(e: &ProtoExpression) -> Option<(bool, isize, usize, usize)> {
        let ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            width,
            var_full_width,
            ..
        } = e
        else {
            return None;
        };
        if dynamic_select.is_some() {
            return None;
        }
        let full = (*var_full_width).max(*width);
        if full > 64 {
            return None;
        }
        let bit = match select {
            Some((hi, lo)) if hi == lo => *lo,
            None if full == 1 => 0,
            _ => return None,
        };
        if bit >= full || bit >= 64 {
            return None;
        }
        let (ff, off) = match var_offset {
            VarOffset::Ff(o) => (true, *o),
            VarOffset::Comb(o) => (false, *o),
        };
        Some((ff, off, bit, full))
    }
    let mut leaves: Vec<&ProtoExpression> = Vec::new();
    flatten(expr, op, &mut leaves);
    if leaves.len() < 2 {
        return None;
    }
    // Classify: per-variable (mask, positives-mask, full_width) plus the rest.
    struct Group {
        mask: u64,
        pos: u64,
        /// Parity of the number of negated leaves (Xor folds each `!b`
        /// into one constant flip: `!a ^ b = (a ^ b) ^ 1`).
        neg_parity: u64,
        full: usize,
        count: usize,
        conflict: bool,
    }
    // BTreeMap: deterministic iteration — the emitted text feeds the
    // artifact cache key, so an unstable group order would re-key every run.
    let mut groups: std::collections::BTreeMap<(bool, isize), Group> =
        std::collections::BTreeMap::new();
    let mut others: Vec<&ProtoExpression> = Vec::new();
    for &leaf in &leaves {
        let (test, neg) = match leaf {
            ProtoExpression::Unary {
                op: Op::LogicNot,
                x,
                ..
            } => (as_bit_test(x), true),
            _ => (as_bit_test(leaf), false),
        };
        // A negated leaf under Or has no masked-compare form; keep it
        // plain.  And absorbs it into the expected-pattern; Xor absorbs it
        // as a constant parity flip.
        let usable = match test {
            Some(_) if neg && op == Op::BitOr => None,
            t => t,
        };
        match usable {
            Some((ff, off, bit, full)) => {
                let g = groups.entry((ff, off)).or_insert(Group {
                    mask: 0,
                    pos: 0,
                    neg_parity: 0,
                    full,
                    count: 0,
                    conflict: false,
                });
                g.full = g.full.max(full);
                let b = 1u64 << bit;
                if g.mask & b != 0 {
                    // Same bit seen before: under And/Or identical polarity
                    // is idempotent and opposite polarity cannot be
                    // expressed; under Xor a repeat TOGGLES (`a^a = 0`), so
                    // any duplicate bails the group.
                    let was_pos = g.pos & b != 0;
                    if op == Op::BitXor || was_pos == neg {
                        g.conflict = true;
                    }
                } else {
                    g.mask |= b;
                    if !neg {
                        g.pos |= b;
                    } else if op == Op::BitXor {
                        g.neg_parity ^= 1;
                    }
                }
                g.count += 1;
            }
            None => others.push(leaf),
        }
    }
    if !groups.values().any(|g| !g.conflict && g.count >= 2) {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for ((ff, off), g) in &groups {
        let vo = if *ff {
            VarOffset::Ff(*off)
        } else {
            VarOffset::Comb(*off)
        };
        if g.conflict || g.count < 2 {
            // Re-emit the single test in its original shape.
            let load = emit_var_load(&vo, g.full)?;
            if g.count >= 2 || g.mask.count_ones() != 1 {
                return None; // conflicted multi-bit group: fall back entirely
            }
            let bit = g.mask.trailing_zeros();
            let inner = format!("((({load}) >> {bit}) & 0x1ULL)");
            parts.push(if g.pos == 0 {
                format!("(0x1ULL ^ {inner})")
            } else {
                inner
            });
            continue;
        }
        let load = emit_var_load(&vo, g.full)?;
        parts.push(match op {
            Op::BitAnd => format!(
                "((uint64_t)((({load}) & {:#x}ULL) == {:#x}ULL))",
                g.mask, g.pos
            ),
            // A same-variable Xor bundle is a parity: popcount folds the
            // whole reduction (ECC generators are exactly this shape).
            Op::BitXor => {
                if g.neg_parity != 0 {
                    format!(
                        "(0x1ULL ^ ((uint64_t)__builtin_parityll(({load}) & {:#x}ULL)))",
                        g.mask
                    )
                } else {
                    format!(
                        "((uint64_t)__builtin_parityll(({load}) & {:#x}ULL))",
                        g.mask
                    )
                }
            }
            // Or groups contain only positive tests (negated ones were
            // diverted to `others` above).
            _ => format!("((uint64_t)((({load}) & {:#x}ULL) != 0))", g.mask),
        });
    }
    for o in others {
        parts.push(emit_expr_inner(o, true)?);
    }
    let joiner = match op {
        Op::BitAnd => " & ",
        Op::BitXor => " ^ ",
        _ => " | ",
    };
    Some(format!("({})", parts.join(joiner)))
}

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
                    || dyn_sel.window > 128
                    || dyn_sel.num_elements == 0
                {
                    return None;
                }
                let idx_str = emit_expr(&dyn_sel.index_expr)?;
                let max_idx = dyn_sel.num_elements.saturating_sub(1);
                if *var_full_width <= 128 {
                    let load = emit_var_load(var_offset, *var_full_width)?;
                    if dyn_sel.window < 64 {
                        let mask = (1u64 << dyn_sel.window) - 1;
                        // Result is <= 64 bits; cast down so a __uint128_t
                        // load (65..128-bit var) still yields a scalar.
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
                    // 64..128-bit window (e.g. an 80-bit element of a 160-bit
                    // pair): shift and mask in __uint128_t; the result stays a
                    // u128-typed scalar.
                    let m: u128 = if dyn_sel.window >= 128 {
                        !0u128
                    } else {
                        (1u128 << dyn_sel.window) - 1
                    };
                    let (mhi, mlo) = ((m >> 64) as u64, m as u64);
                    return Some(format!(
                        "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                            uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                            ((((__uint128_t)({load})) >> (_idx * {ew})) \
                             & (((__uint128_t)0x{mhi:x}ULL << 64) | (__uint128_t)0x{mlo:x}ULL)); }})",
                        idx = idx_str,
                        max = max_idx,
                        load = load,
                        ew = dyn_sel.elem_width,
                    ));
                }
                if dyn_sel.window > 64 {
                    // Wide (>128-bit) var with a 65..128-bit window: 3-word
                    // funnel read at the runtime bit offset, assembled into a
                    // __uint128_t.  Reads past the end are guarded to 0.
                    let (buf, off) = match var_offset {
                        VarOffset::Ff(o) => ("ff_values", *o),
                        VarOffset::Comb(o) => ("comb_values", *o),
                    };
                    if off < 0 {
                        return None;
                    }
                    let nw = wide_words(native_bytes(*var_full_width));
                    let m: u128 = if dyn_sel.window >= 128 {
                        !0u128
                    } else {
                        (1u128 << dyn_sel.window) - 1
                    };
                    let (mhi, mlo) = ((m >> 64) as u64, m as u64);
                    return Some(format!(
                        "({{ uint64_t _idx_raw = (uint64_t)({idx}); \
                            uint64_t _idx = _idx_raw < {max} ? _idx_raw : {max}; \
                            uint64_t _bit = _idx * {ew}; uint64_t _w = _bit >> 6; uint32_t _s = (uint32_t)(_bit & 63); \
                            const veryl_u64_ua* _p = (const veryl_u64_ua*)({b} + {off:#x}); \
                            uint64_t _q0 = _w < {nw}ull ? _p[_w] : 0; \
                            uint64_t _q1 = (_w + 1) < {nw}ull ? _p[_w + 1] : 0; \
                            uint64_t _q2 = (_w + 2) < {nw}ull ? _p[_w + 2] : 0; \
                            uint64_t _v0 = _s == 0 ? _q0 : ((_q0 >> _s) | (_q1 << (64 - _s))); \
                            uint64_t _v1 = _s == 0 ? _q1 : ((_q1 >> _s) | (_q2 << (64 - _s))); \
                            ((((__uint128_t)_v1 << 64) | (__uint128_t)_v0) \
                             & (((__uint128_t)0x{mhi:x}ULL << 64) | (__uint128_t)0x{mlo:x}ULL)); }})",
                        idx = idx_str,
                        max = max_idx,
                        ew = dyn_sel.elem_width,
                        b = buf,
                        off = off,
                        nw = nw,
                    ));
                }
                let mask = width_mask(dyn_sel.window);
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
                let nw = wide_words(native_bytes(*var_full_width));
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
                        // The pre-mask canonicalizes a dirty operand before
                        // the reduction compares/parity — elide when clean.
                        let m = if clean_elide() && expr_emits_clean(x) {
                            format!("((uint64_t)({xs}))")
                        } else {
                            format!("(((uint64_t)({xs})) & 0x{mask:x}ULL)")
                        };
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
            // Bit-test merging: see emit_bit_test_merge.
            if matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor)
                && expr_context.width == 1
                && !expr_context.signed
                && bitmerge_enabled()
                && let Some(s) = emit_bit_test_merge(expr)
            {
                return Some(s);
            }
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
            // `x & C` where the constant covers every bit x can carry (the
            // shape a canonicalization wrapper leaves behind): the AND is a
            // bit-exact no-op, so emit x alone.  The result is clean because
            // x is, so any `needs_clean` request is still honored.
            if clean_elide()
                && matches!(op, Op::BitAnd)
                && !expr_context.signed
                && let ProtoExpression::Value {
                    value: Value::U64(v),
                    ..
                } = &**y
                && v.mask_xz == 0
                && x.width() > 0
                && x.width() <= 64
                && (v.payload & width_mask(x.width())) == width_mask(x.width())
                && expr_emits_clean(x)
            {
                return Some(xs);
            }
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
                // 65..128-bit unsigned shift-LEFT with a narrow (≤64-bit) left
                // operand: `(uint64_t)xs << ys` truncates to 64 bits, so promote
                // xs to __uint128_t first. Placed before the `wide_truncates`
                // bail below, which would otherwise send this to the interpreter.
                if expr_context.width > 64
                    && expr_context.width <= 128
                    && matches!(op, Op::LogicShiftL | Op::ArithShiftL)
                    && x.width() <= 64
                    && !expr_context.signed
                {
                    let w = expr_context.width;
                    let xm = if x.width() >= 64 {
                        format!("((__uint128_t)((uint64_t)({xs})))")
                    } else {
                        format!(
                            "((__uint128_t)(((uint64_t)({xs})) & 0x{:x}ULL))",
                            width_mask(x.width())
                        )
                    };
                    let shifted = format!(
                        "((((__uint128_t)({ys})) >= {w}) ? (__uint128_t)0 : (({xm}) << ({ys})))"
                    );
                    return Some(if w < 128 {
                        mask_u128(&shifted, w)
                    } else {
                        shifted
                    });
                }
                // 65..128-bit Add/Sub/Mul with both operands ≤64 bits: C
                // computes the both-narrow case in uint64_t and truncates, so
                // promote both operands to __uint128_t first.  A 64x64
                // product fits 128 bits exactly, and add/sub wrap in 128 then
                // mask to w — the modulo-2^w SystemVerilog result.  Signed
                // variants sign-extend each operand from its own width into
                // __int128 (the low w bits of the infinite-precision result
                // are identical), e.g. a 33x33→66 multiplier.
                if expr_context.width > 64
                    && expr_context.width <= 128
                    && matches!(op, Op::Add | Op::Sub | Op::Mul)
                    && x.width() <= 64
                    && y.width() <= 64
                {
                    let w = expr_context.width;
                    let promote = |s: &str, sw: usize| -> String {
                        if expr_context.signed {
                            if sw >= 64 {
                                format!("((__int128_t)((int64_t)((uint64_t)({s}))))")
                            } else {
                                let sh = 64 - sw;
                                format!(
                                    "((__int128_t)(((int64_t)(((uint64_t)({s})) << {sh})) >> {sh}))"
                                )
                            }
                        } else if sw >= 64 {
                            format!("((__uint128_t)((uint64_t)({s})))")
                        } else {
                            format!(
                                "((__uint128_t)(((uint64_t)({s})) & 0x{:x}ULL))",
                                width_mask(sw)
                            )
                        }
                    };
                    let xm = promote(&xs, x.width());
                    let ym = promote(&ys, y.width());
                    let body = format!("((__uint128_t)(({xm}) {c_op} ({ym})))");
                    return Some(if w < 128 { mask_u128(&body, w) } else { body });
                }
                let wide_truncates = match op {
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
                        let src_nw = wide_words(src_nb);
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
                if *width == 0 || *width > 128 || t_w > 128 || f_w > 128 {
                    return None;
                }
                // The 64-bit arm below cannot sign-extend a wider branch.
                if *width <= 64 && (t_w > 64 || f_w > 64) {
                    return None;
                }
                let t =
                    emit_expr_inner(true_expr, true).or_else(|| emit_scalar_via_wide(true_expr))?;
                let f = emit_expr_inner(false_expr, true)
                    .or_else(|| emit_scalar_via_wide(false_expr))?;
                if *width <= 64 {
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
                // 65..=128.  A ≤64-bit operand is a uint64_t expression, so
                // sign-extend it within 64 bits before widening; a wider one
                // is already __uint128_t.
                let sext128 = |s: &str, w: usize| -> String {
                    if w == 128 {
                        format!("((__int128)(__uint128_t)({s}))")
                    } else if w > 64 {
                        let sh = 128 - w;
                        format!("(((__int128)((__uint128_t)({s}) << {sh})) >> {sh})")
                    } else if w == 64 {
                        format!("((__int128)(int64_t)(uint64_t)({s}))")
                    } else {
                        let sh = 64 - w;
                        format!("((__int128)(((int64_t)((uint64_t)({s}) << {sh})) >> {sh}))")
                    }
                };
                let inner = format!(
                    "(({}) ? ({}) : ({}))",
                    wrap_expect(&c),
                    sext128(&t, t_w),
                    sext128(&f, f_w)
                );
                let r = format!("((__uint128_t)({inner}))");
                if *width < 128 {
                    return Some(mask_u128(&r, *width));
                }
                return Some(r);
            }
            // A branch narrow enough for the scalar emitter can still need
            // the wide pipeline for a >128-bit intermediate.
            let t = emit_expr_inner(true_expr, needs_clean)
                .or_else(|| emit_scalar_via_wide(true_expr))?;
            let f = emit_expr_inner(false_expr, needs_clean)
                .or_else(|| emit_scalar_via_wide(false_expr))?;
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
                    if sub_width == 0 || sub_width > 128 {
                        return None;
                    }
                    let sub_str = emit_expr(sub).or_else(|| emit_scalar_sub_via_wide(sub))?;
                    let ew = *elem_width;
                    if sub_width > 64 {
                        // Wide (65..128-bit) element under a leading sign
                        // repeat, e.g. `{62{sign}, product66}`: total width
                        // > 64 ⇒ `acc` is __uint128_t; mask in u128.
                        let m: u128 = if sub_width >= 128 {
                            !0u128
                        } else {
                            (1u128 << sub_width) - 1
                        };
                        let (mhi, mlo) = ((m >> 64) as u64, m as u64);
                        for _ in 0..*repeat {
                            acc = format!(
                                "((({acc}) << {ew}) | (((__uint128_t)({sub_str})) & (((__uint128_t)0x{mhi:x}ULL << 64) | (__uint128_t)0x{mlo:x}ULL)))"
                            );
                            lower_width += ew;
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
                                w = ew,
                                sub = sub_str,
                                mask = mask,
                            );
                        } else {
                            let elem =
                                format!("(({sub}) & 0x{mask:x}ULL)", sub = sub_str, mask = mask);
                            // `acc << 64` is UB on a uint64_t: x86 shifts use
                            // only the low 6 bits of the count, so it computes
                            // `acc | elem` instead of dropping `acc`.
                            acc = if ew >= 64 {
                                elem
                            } else {
                                format!("((({acc}) << {w}) | {elem})", acc = acc, w = ew)
                            };
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
            for (sub, repeat, elem_width) in elements {
                // An unsized literal ('0/'1) reports width 0; the element
                // tuple's declared width is the authoritative slot size.
                let sub_width = if sub.width() == 0 {
                    *elem_width
                } else {
                    sub.width()
                };
                if sub_width == 0 || sub_width > 128 {
                    return None;
                }
                let (sub_str, via_wide) = match emit_expr(sub) {
                    Some(s) => (s, false),
                    None => (emit_scalar_sub_via_wide(sub)?, true),
                };
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
                // Slot-mask elision: a provably-clean element already carries
                // nothing above its slot width, so the mask is a no-op.  The
                // wide marshaling fallback and unsized literals keep it.
                let sub_clean =
                    clean_elide() && !via_wide && sub.width() == sub_width && expr_emits_clean(sub);
                for _ in 0..*repeat {
                    if wide_acc {
                        acc = if sub_clean {
                            format!(
                                "((({acc}) << {w}) | ((__uint128_t)({sub})))",
                                acc = acc,
                                w = sub_width,
                                sub = sub_str,
                            )
                        } else {
                            format!(
                                "((({acc}) << {w}) | (((__uint128_t)({sub})) & (__uint128_t)0x{mask:x}ULL))",
                                acc = acc,
                                w = sub_width,
                                sub = sub_str,
                                mask = mask,
                            )
                        };
                    } else {
                        let elem = if sub_clean {
                            format!("({sub})", sub = sub_str)
                        } else {
                            format!("(({sub}) & 0x{mask:x}ULL)", sub = sub_str, mask = mask)
                        };
                        // Same UB as the sign-repeat fold above.
                        acc = if sub_width >= 64 {
                            elem
                        } else {
                            format!("((({acc}) << {w}) | {elem})", acc = acc, w = sub_width)
                        };
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
    use std::time::Instant;
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

    fn var_expr_signed(var_offset: VarOffset, width: usize) -> ProtoExpression {
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
        // Two shift+OR steps; the elements are canonical full loads, so the
        // clean-bits elision drops their slot masks.
        assert_eq!(s.matches("<< 8").count(), 2);
        assert_eq!(s.matches("0xffULL").count(), 0);
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
        // repeat=3 yields three nested shift+OR pairs; the canonical load
        // needs no slot mask (clean-bits elision).
        assert_eq!(s.matches("<< 4").count(), 3);
        assert_eq!(s.matches("0xfULL").count(), 0);
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

    fn cc_available() -> bool {
        Command::new(std::env::var("VERYL_AOT_CC").unwrap_or_else(|_| "cc".to_string()))
            .arg("--version")
            .output()
            .is_ok()
    }

    fn read_u128(comb: &[u8], off: usize) -> u128 {
        u128::from_le_bytes(comb[off..off + 16].try_into().unwrap())
    }

    #[test]
    fn emit_sign_extending_store_above_128_stays_covered() {
        // A bare signed RHS narrower than a >128-bit destination sign-extends
        // at the store.  The value is right either way — the fallback path
        // computes it too — so this asserts COVERAGE, which is what the
        // vw_sext_copy store buys.
        let src = var_expr_signed(VarOffset::Comb(0), 100);
        let assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(64),
            dst_width: 200,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: src,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        assert!(
            emit_function(&[assign]).is_some(),
            "a 100-bit signed RHS into a 200-bit dst must stay AOT-covered"
        );
    }

    #[test]
    fn emit_sign_extending_store_with_dst_select_declines() {
        // The plain-store arm is the only one that sign-extends, so the same
        // signed RHS combined with a dst bit-select must decline rather than
        // store an unextended value.
        let src = var_expr_signed(VarOffset::Comb(0), 100);
        let assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(64),
            dst_width: 200,
            select: Some((150, 40)),
            dynamic_select: None,
            rhs_select: None,
            expr: src,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        assert!(emit_function(&[assign]).is_none());
    }

    #[test]
    fn emit_concat_zero_width_literal_uses_declared_slot() {
        // An unsized literal ('0/'1) reports width 0, so the element tuple's
        // declared width is the only source for the slot size — reading
        // `sub.width()` rejected the whole concat.
        let lit = ProtoExpression::Value {
            value: crate::ir::Value::new(0, 0, false),
            width: 0,
            expr_context: ctx(0, false),
        };
        let a = var_expr(VarOffset::Comb(0), 8);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(lit), 1, 8), (Box::new(a), 1, 8)],
            width: 16,
            expr_context: ctx(16, false),
        };
        let s = emit_expr(&e).expect("zero-width literal element must stay covered");
        assert!(
            s.contains("<< 8"),
            "the literal still occupies its 8-bit slot: {s}"
        );
    }

    #[test]
    fn emit_narrow_operand_mul_widens_to_128() {
        // A 33x33->66 multiplier: a 65..128-bit result from <=64-bit
        // operands, which C would otherwise evaluate in uint64_t.
        if !cc_available() {
            eprintln!("emit_narrow_operand_mul_widens_to_128: cc unavailable, skipping");
            return;
        }
        let mul = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0), 33)),
            op: Op::Mul,
            y: Box::new(var_expr(VarOffset::Comb(8), 33)),
            width: 66,
            expr_context: ctx(66, false),
        };
        let assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(32),
            dst_width: 66,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: mul,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let src = emit_function(&[assign]).expect("66-bit product must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_wmul_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "emit_narrow_operand_mul_widens_to_128")
        else {
            return;
        };
        let a: u64 = 0x1_0000_0001;
        let b: u64 = 0x1_0000_0003;
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 64];
        comb[0..8].copy_from_slice(&a.to_le_bytes());
        comb[8..16].copy_from_slice(&b.to_le_bytes());
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        let expect = ((a as u128) * (b as u128)) & ((1u128 << 66) - 1);
        assert_eq!(read_u128(&comb, 32), expect);
        let _ = fs::remove_dir_all(&tmp);

        // Zero-extending the operands instead would make -3 a large positive.
        let mul = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0), 33)),
            op: Op::Mul,
            y: Box::new(var_expr(VarOffset::Comb(8), 33)),
            width: 66,
            expr_context: ctx(66, true),
        };
        let assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(32),
            dst_width: 66,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: mul,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let src = emit_function(&[assign]).expect("signed 66-bit product must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_wmuls_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "emit_narrow_operand_mul_signed") else {
            return;
        };
        let neg3 = ((1u64 << 33) - 3) & ((1u64 << 33) - 1);
        let five: u64 = 5;
        comb[0..8].copy_from_slice(&neg3.to_le_bytes());
        comb[8..16].copy_from_slice(&five.to_le_bytes());
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        let expect = ((-15i128) as u128) & ((1u128 << 66) - 1);
        assert_eq!(read_u128(&comb, 32), expect);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn emit_concat_sign_repeat_with_wide_element() {
        // `{62{sign}, value66}` — a >64-bit element under a leading 1-bit
        // sign repeat (a widened multiply result), previously a
        // whole-module bail.  dst: 128-bit at comb[32..48].
        if !cc_available() {
            eprintln!("emit_concat_sign_repeat_with_wide_element: cc unavailable, skipping");
            return;
        }
        let concat = ProtoExpression::Concatenation {
            elements: vec![
                (Box::new(var_expr(VarOffset::Comb(0), 1)), 62, 1),
                (Box::new(var_expr(VarOffset::Comb(8), 66)), 1, 66),
            ],
            width: 128,
            expr_context: ctx(128, false),
        };
        let assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(32),
            dst_width: 128,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: concat,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let src = emit_function(&[assign])
            .expect("sign-repeat concat with a 66-bit element must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_wcat_{}", std::process::id()));
        let Some(module) =
            compile_for_test(&tmp, &src, "emit_concat_sign_repeat_with_wide_element")
        else {
            return;
        };
        let v66: u128 = 0x2_DEAD_BEEF_1234_5678;
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 64];
        comb[0] = 1; // sign bit set
        comb[8..24].copy_from_slice(&v66.to_le_bytes());
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        let expect = (((1u128 << 62) - 1) << 66) | v66;
        assert_eq!(read_u128(&comb, 32), expect);
        // Sign clear: upper 62 bits zero.
        comb[0] = 0;
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        assert_eq!(read_u128(&comb, 32), v66);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn compile_lock_ticket_states() {
        let tmp = std::env::temp_dir().join(format!("veryl_aot_lock_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let so = tmp.join("veryl_aot_deadbeef.so");
        let lock = tmp.join("veryl_aot_deadbeef.lock");

        // Fresh: we own the lock, and the file exists while we hold it.
        let ticket = acquire_compile_lock(&tmp, "deadbeef", &so);
        assert!(matches!(ticket, CompileTicket::Owned(_)));
        assert!(lock.exists());

        // A second caller must not compile the same hash.  It waits, so give
        // it the exit the owner's publish provides.  (The lock carries no pid
        // yet, so the age rule keeps it alive — exactly the spawn window.)
        fs::write(&so, b"not a real object").unwrap();
        assert!(matches!(
            acquire_compile_lock(&tmp, "deadbeef", &so),
            CompileTicket::Published
        ));

        fs::remove_file(&so).unwrap();

        // Dropping the owner releases the lock even without the script's `rm`
        // (the non-unix path and every early-return error path rely on this).
        drop(ticket);
        assert!(!lock.exists());

        // A lock naming a dead owner is taken over at once rather than after
        // the stale timeout: otherwise a run killed mid-compile would wedge
        // this artifact out of the cache for ten minutes.  Only unix can
        // identify the owner; elsewhere `owner_alive` yields the age rule.
        #[cfg(unix)]
        {
            // `kill -0 0` addresses our own process group, so name a pid that
            // cannot exist rather than 0.
            fs::write(&lock, format!("{}\n", u32::MAX)).unwrap();
            assert_eq!(owner_alive(&lock), Some(false));
            let retaken = acquire_compile_lock(&tmp, "deadbeef", &so);
            assert!(matches!(retaken, CompileTicket::Owned(_)));
            drop(retaken);
            assert!(!lock.exists());
        }

        // An unusable lock directory degrades to compiling unlocked rather
        // than failing the compile.
        assert!(matches!(
            acquire_compile_lock(
                &tmp.join("no").join("such").join("dir"),
                "deadbeef",
                &tmp.join("no").join("such").join("dir").join("x.so"),
            ),
            CompileTicket::Unlocked
        ));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn compile_lock_serializes_identical_sources() {
        // Concurrent callers of one hash: all must get a working module, the
        // artifact must be published exactly once, and no lock may be left
        // behind (a leaked lock would wedge the artifact until the stale
        // takeover).
        if !cc_available() {
            eprintln!("compile_lock_serializes_identical_sources: cc unavailable, skipping");
            return;
        }
        let src = "\
            #include <stdint.h>\n\
            __attribute__((visibility(\"default\")))\n\
            void veryl_aot_eval(uint8_t *ff, uint8_t *comb, uint64_t *log, intptr_t ff_delta) {\n\
                (void)ff; (void)log; (void)ff_delta;\n\
                *(uint32_t*)(comb + 0) = 0x5a5a5a5a;\n\
            }\n";
        let tmp = std::env::temp_dir().join(format!("veryl_aot_lock_cc_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let dir = tmp.clone();
            handles.push(thread::spawn(move || {
                compile_source_in(&dir, src).map(|_| ())
            }));
        }
        let mut skip = false;
        for h in handles {
            match h.join().unwrap() {
                Ok(()) => {}
                Err(e) if e.starts_with("dlopen") || e.starts_with("dlsym") => skip = true,
                Err(e) => panic!("concurrent compile: {e}"),
            }
        }
        if skip {
            eprintln!("compile_lock_serializes_identical_sources: .so not loadable here; skipping");
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
        let mut so = 0usize;
        let mut locks = 0usize;
        for e in fs::read_dir(&tmp).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            if name.ends_with(".so") {
                so += 1;
            } else if name.ends_with(".lock") {
                locks += 1;
            }
        }
        assert_eq!(so, 1, "one artifact per hash");
        assert_eq!(locks, 0, "the compile lock must be released");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn emit_rhs_select_field_into_dst_select_rmw() {
        // A per-bank extract: dst96[23:0] = (x700[174:0])[174:151] —
        // a wide-pointer RHS with BOTH a dst bit-select and an rhs_select.
        if !cc_available() {
            eprintln!("emit_rhs_select_field_into_dst_select_rmw: cc unavailable, skipping");
            return;
        }
        let slice = || ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0),
            select: Some((174, 0)),
            dynamic_select: None,
            width: 175,
            var_full_width: 700,
            expr_context: ctx(175, false),
        };
        let mk = |dst: isize, sel: Option<(usize, usize)>, rsel: Option<(usize, usize)>| {
            ProtoStatement::Assign(ProtoAssignStatement {
                dst: VarOffset::Comb(dst),
                dst_width: 96,
                select: sel,
                dynamic_select: None,
                rhs_select: rsel,
                expr: slice(),
                dst_ff_current_offset: 0,
                token: dummy_token(),
            })
        };
        let src = emit_function(&[
            mk(0x100, Some((23, 0)), Some((174, 151))),
            mk(0x140, None, None),
            mk(0x160, None, Some((174, 151))),
        ])
        .expect("dst-select + rhs_select on a wide RHS must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_wselrmw_{}", std::process::id()));
        let Some(module) =
            compile_for_test(&tmp, &src, "emit_rhs_select_field_into_dst_select_rmw")
        else {
            return;
        };
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 0x200];
        // x bits [151..174] = 0xB7_1FDF (24 bits); bit k lives at byte k/8.
        let field: u64 = 0xB7_1FDF;
        for k in 0..24u64 {
            if (field >> k) & 1 == 1 {
                let bit = 151 + k as usize;
                comb[bit / 8] |= 1 << (bit % 8);
            }
        }
        // Pre-set dst bits above the field to check the RMW keeps them.
        comb[0x100 + 4] = 0xAA;
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        let plain = u64::from_le_bytes(comb[0x140 + 16..0x140 + 24].try_into().unwrap());
        // dst 0x140 = slice[95:0]; bits 151.. are beyond 96 — check bytes 18-21
        // of the SOURCE slice landed... instead check low 96 truncation via a
        // known bit: source bit 151+ not visible here, so just check the
        // rhssel-only variant.
        let _ = plain;
        let fonly = u64::from_le_bytes(comb[0x160..0x168].try_into().unwrap());
        assert_eq!(fonly & 0xFF_FFFF, field, "rhssel-only field");
        let lo = u64::from_le_bytes(comb[0x100..0x108].try_into().unwrap());
        assert_eq!(lo & 0xFF_FFFF, field, "field bits [23:0]");
        assert_eq!((lo >> 32) & 0xFF, 0xAA, "RMW must keep untouched dst bits");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn emit_rhs_select_on_wide_pointer_store() {
        // rhs_select on a non-foldable wide RHS: dst128 = (a192 | b192)
        // [159:32] — the wide-pointer store path extracts the field with
        // vw_lshr instead of bailing to Cranelift.
        if !cc_available() {
            eprintln!("emit_rhs_select_on_wide_pointer_store: cc unavailable, skipping");
            return;
        }
        let or192 = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0), 192)),
            op: Op::BitOr,
            y: Box::new(var_expr(VarOffset::Comb(24), 192)),
            width: 192,
            expr_context: ctx(192, false),
        };
        assert!(
            or192.builds_wide_pointer(),
            "192-bit OR must be a wide-pointer expr"
        );
        let assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(48),
            dst_width: 128,
            select: None,
            dynamic_select: None,
            rhs_select: Some((159, 32)),
            expr: or192,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let src = emit_function(&[assign])
            .expect("rhs_select on a wide-pointer RHS must stay AOT-covered");
        assert!(
            src.contains("vw_lshr"),
            "field extract goes through vw_lshr"
        );
        let tmp = std::env::temp_dir().join(format!("veryl_aot_wsel_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "emit_rhs_select_on_wide_pointer_store")
        else {
            return;
        };
        let a: [u64; 3] = [
            0x1111_2222_3333_4444,
            0x5555_6666_7777_8888,
            0x9999_AAAA_BBBB_CCCC,
        ];
        let b: [u64; 3] = [
            0x0F0F_0F0F_0F0F_0F0F,
            0xF0F0_F0F0_F0F0_F0F0,
            0x00FF_00FF_00FF_00FF,
        ];
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 80];
        for (i, w) in a.iter().enumerate() {
            comb[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        for (i, w) in b.iter().enumerate() {
            comb[24 + i * 8..24 + i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        // Expected: bits [159:32] of a|b, as a 128-bit value.
        let or0 = a[0] | b[0];
        let or1 = a[1] | b[1];
        let or2 = a[2] | b[2];
        // value >> 32 over the 192-bit [or2:or1:or0].
        let expected_sel = ((or0 as u128) >> 32) | ((or1 as u128) << 32) | ((or2 as u128) << 96);
        assert_eq!(read_u128(&comb, 48), expected_sel);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn wide_concat_store_to_a_dynamically_read_middle_element() {
        // arr[1] = {arr[idx][63:0], src}: the dynamic read can alias the
        // destination (idx == 1), so the direct into-form (which zeroes the
        // destination first) must not be used — the expanded gather sees the
        // middle element where the compact one does not.
        if !cc_available() {
            eprintln!("wide_concat_store_to_a_dynamically_read_middle_element: skipping");
            return;
        }
        let base = 0x40isize; // 4 elements x 192 bit (24 B)
        let dyn_low = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(base),
            stride: 24,
            element_native_bytes: 24,
            index_expr: Box::new(ProtoExpression::Variable {
                var_offset: VarOffset::Comb(0x200),
                select: None,
                dynamic_select: None,
                width: 32,
                var_full_width: 32,
                expr_context: ctx(32, false),
            }),
            num_elements: 4,
            select: Some((63, 0)),
            dynamic_select: None,
            width: 64,
            expr_context: ctx(64, false),
        };
        let src = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x210),
            select: None,
            dynamic_select: None,
            width: 128,
            var_full_width: 128,
            expr_context: ctx(128, false),
        };
        let stmt = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(base + 24), // element 1: the middle
            dst_width: 192,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Concatenation {
                elements: vec![(Box::new(dyn_low), 1, 64), (Box::new(src), 1, 128)],
                width: 192,
                expr_context: ctx(192, false),
            },
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let src_txt = emit_function(&[stmt]).expect("must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_dynmid_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src_txt, "wide_concat_dyn_middle") else {
            return;
        };
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 0x300];
        for (i, b) in comb[0x58..0x70].iter_mut().enumerate() {
            *b = 0xa0 + i as u8; // old arr[1]
        }
        comb[0x200..0x204].copy_from_slice(&1u32.to_le_bytes()); // idx = 1 (the dst)
        for (i, b) in comb[0x210..0x220].iter_mut().enumerate() {
            *b = 0x10 + i as u8; // src
        }
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        // Low 128 bits = src; bits [191:128] = OLD arr[1][63:0].
        assert_eq!(
            &comb[0x58..0x68],
            &(0x10..0x20).collect::<Vec<u8>>()[..],
            "src half"
        );
        assert_eq!(
            &comb[0x68..0x70],
            &(0xa0..0xa8).collect::<Vec<u8>>()[..],
            "old low word must survive into the high bits"
        );
    }

    #[test]
    fn emit_narrow_dst_rhs_select_on_wide_pointer_rhs() {
        // `assign narrow = (a192 | b192)[150:125];` — the scalar sibling
        // of `emit_rhs_select_on_wide_pointer_store`.
        if !cc_available() {
            eprintln!("emit_narrow_dst_rhs_select_on_wide_pointer_rhs: cc unavailable, skipping");
            return;
        }
        let or192 = || ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0), 192)),
            op: Op::BitOr,
            y: Box::new(var_expr(VarOffset::Comb(24), 192)),
            width: 192,
            expr_context: ctx(192, false),
        };
        let mk = |dst: isize, dw: usize, sel: Option<(usize, usize)>| {
            ProtoStatement::Assign(ProtoAssignStatement {
                dst: VarOffset::Comb(dst),
                dst_width: dw,
                select: sel,
                dynamic_select: None,
                rhs_select: Some((150, 125)),
                expr: or192(),
                dst_ff_current_offset: 0,
                token: dummy_token(),
            })
        };
        let src = emit_function(&[
            mk(48, 26, None),          // plain narrow store
            mk(56, 64, Some((30, 5))), // field into a dst bit-select RMW
        ])
        .expect("narrow-dst rhs_select on a wide-pointer RHS must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_nwsel_{}", std::process::id()));
        let Some(module) =
            compile_for_test(&tmp, &src, "emit_narrow_dst_rhs_select_on_wide_pointer_rhs")
        else {
            return;
        };
        let a: [u64; 3] = [
            0x1111_2222_3333_4444,
            0x5555_6666_7777_8888,
            0x9999_AAAA_BBBB_CCCC,
        ];
        let b: [u64; 3] = [
            0x0F0F_0F0F_0F0F_0F0F,
            0xF0F0_F0F0_F0F0_F0F0,
            0x00FF_00FF_00FF_00FF,
        ];
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 80];
        for (i, w) in a.iter().enumerate() {
            comb[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        for (i, w) in b.iter().enumerate() {
            comb[24 + i * 8..24 + i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        // Pre-set dst2 bits outside the [30:5] window to check the RMW.
        comb[56..64].copy_from_slice(&0xDEAD_0000_0000_0003u64.to_le_bytes());
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        let or: Vec<u64> = a.iter().zip(&b).map(|(x, y)| x | y).collect();
        // Bits [150:125] span or1/or2 (125 = 64 + 61).
        let shifted = (or[1] >> 61) | (or[2] << 3);
        let field = shifted & ((1u64 << 26) - 1);
        let plain = u64::from_le_bytes(comb[48..56].try_into().unwrap());
        assert_eq!(plain & ((1 << 26) - 1), field, "plain narrow store");
        let rmw = u64::from_le_bytes(comb[56..64].try_into().unwrap());
        assert_eq!((rmw >> 5) & ((1 << 26) - 1), field, "field into [30:5]");
        assert_eq!(rmw & 0x3, 0x3, "RMW keeps bits below the window");
        assert_eq!(
            rmw & 0xFFFF_0000_0000_0000,
            0xDEAD_0000_0000_0000,
            "RMW keeps bits above the window"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn emit_narrow_dst_from_wide_pointer_rhs() {
        // `assign bit1 = a192 & b192;` — SV truncation of a wide-op RHS
        // into a ≤64-bit destination.
        // dst: 1-bit at comb[48], operands at comb[0]/comb[24].
        if !cc_available() {
            eprintln!("emit_narrow_dst_from_wide_pointer_rhs: cc unavailable, skipping");
            return;
        }
        let and192 = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0), 192)),
            op: Op::BitAnd,
            y: Box::new(var_expr(VarOffset::Comb(24), 192)),
            width: 192,
            expr_context: ctx(192, false),
        };
        let assign = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(48),
            dst_width: 1,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: and192,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let src = emit_function(&[assign])
            .expect("narrow dst fed by a wide-pointer RHS must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_ndw_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "emit_narrow_dst_from_wide_pointer_rhs")
        else {
            return;
        };
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 64];
        comb[0] = 0x3; // a bit0/bit1 set
        comb[24] = 0x1; // b bit0 set → (a&b) bit0 = 1
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        assert_eq!(comb[48] & 1, 1);
        comb[24] = 0x2; // b bit1 only → (a&b) bit0 = 0
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        assert_eq!(comb[48] & 1, 0);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn emit_dynamic_select_wide_windows() {
        // Dynamic element reads with 64..128-bit windows: an 80-bit element
        // of a 160-bit pair (u128 shift path) and a 96-bit element of a
        // 288-bit triple (3-word funnel path).  Both were interpreter-only.
        if !cc_available() {
            eprintln!("emit_dynamic_select_wide_windows: cc unavailable, skipping");
            return;
        }
        use crate::ir::ProtoDynamicBitSelect;
        let mk_read = |base: isize, vfw: usize, ew: usize, ne: usize, idx_off: isize| {
            ProtoExpression::Variable {
                var_offset: VarOffset::Comb(base),
                select: None,
                dynamic_select: Some(ProtoDynamicBitSelect {
                    index_expr: Box::new(var_expr(VarOffset::Comb(idx_off), 32)),
                    elem_width: ew,
                    window: ew,
                    num_elements: ne,
                }),
                width: ew,
                var_full_width: vfw,
                expr_context: ctx(ew, false),
            }
        };
        let mk_assign = |dst: isize, dw: usize, e: ProtoExpression| {
            ProtoStatement::Assign(ProtoAssignStatement {
                dst: VarOffset::Comb(dst),
                dst_width: dw,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: e,
                dst_ff_current_offset: 0,
                token: dummy_token(),
            })
        };
        // pair160 at comb[0..24] (idx at 96), triple288 at comb[24..64]
        // (idx at 100); dst80 at comb[112..128], dst96 at comb[128..144].
        let src = emit_function(&[
            mk_assign(112, 80, mk_read(0, 160, 80, 2, 96)),
            mk_assign(128, 96, mk_read(24, 288, 96, 3, 100)),
            // vfw ≤ 128 with a full-64-bit window (u128 shift path).
            mk_assign(144, 64, mk_read(64, 128, 64, 2, 104)),
        ])
        .expect("64..128-bit dynamic-select windows must stay AOT-covered");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_dsw_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "emit_dynamic_select_wide_windows") else {
            return;
        };
        // pair160 = element1(80b) : element0(80b)
        let e0: u128 = 0x1234_5678_9ABC_DEF0_1122u128 & ((1u128 << 80) - 1);
        let e1: u128 = 0xFEDC_BA98_7654_3210_3344u128 & ((1u128 << 80) - 1);
        let pair: [u8; 20] = {
            // 80 bits = 10 bytes per element, little-endian, byte-aligned.
            let mut v = [0u8; 20];
            for byte in 0..10 {
                v[byte] = ((e0 >> (byte * 8)) & 0xff) as u8;
                v[10 + byte] = ((e1 >> (byte * 8)) & 0xff) as u8;
            }
            v
        };
        // triple288: 96-bit elements t0,t1,t2
        let t = [
            0xAAAA_BBBB_CCCC_DDDD_EEEE_FFFFu128 & ((1u128 << 96) - 1),
            0x0102_0304_0506_0708_090A_0B0Cu128 & ((1u128 << 96) - 1),
            0xF00D_FACE_CAFE_BEEF_1234_5678u128 & ((1u128 << 96) - 1),
        ];
        let mut triple = [0u8; 36];
        for (i, tv) in t.iter().enumerate() {
            let bit = i * 96;
            // Scatter tv into the little-endian byte array at bit offset.
            for byte in 0..12 {
                let val = ((tv >> (byte * 8)) & 0xff) as u8;
                let pos_bit = bit + byte * 8;
                let (pb, ps) = (pos_bit / 8, pos_bit % 8);
                assert_eq!(ps, 0); // 96 is byte-aligned
                triple[pb] |= val;
            }
        }
        let g: [u64; 2] = [0xDEAD_BEEF_0BAD_F00D, 0x0123_4567_89AB_CDEF];
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 176];
        comb[0..20].copy_from_slice(&pair);
        comb[24..60].copy_from_slice(&triple);
        comb[64..72].copy_from_slice(&g[0].to_le_bytes());
        comb[72..80].copy_from_slice(&g[1].to_le_bytes());
        let mut log = vec![0u64; 16];
        for (idx, want80, want96, want64) in [
            (0u32, e0, t[0], g[0]),
            (1, e1, t[1], g[1]),
            (2, e1, t[2], g[1]), // out-of-range indexes clamp to the last element
        ] {
            comb[96..100].copy_from_slice(&idx.min(7).to_le_bytes());
            comb[100..104].copy_from_slice(&idx.to_le_bytes());
            comb[104..108].copy_from_slice(&idx.to_le_bytes());
            unsafe {
                (module.func)(
                    ff.as_mut_ptr(),
                    comb.as_mut_ptr(),
                    log.as_mut_ptr() as *mut u8,
                    0,
                );
            }
            assert_eq!(read_u128(&comb, 112) & ((1u128 << 80) - 1), want80);
            assert_eq!(read_u128(&comb, 128) & ((1u128 << 96) - 1), want96);
            assert_eq!(
                u64::from_le_bytes(comb[144..152].try_into().unwrap()),
                want64
            );
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn emit_wide_select_reads_above_128_bits() {
        // Reads whose RESULT exceeds 128 bits, from a 576-bit source: a
        // static select spanning words 2..5, one whose top window has no
        // successor word to funnel in, and a dynamic 192-bit element.
        if !cc_available() {
            eprintln!("emit_wide_select_reads_above_128_bits: cc unavailable, skipping");
            return;
        }
        use crate::ir::ProtoDynamicBitSelect;
        // Reference extraction, bit by bit: independent of the shift the
        // emitted helper performs.
        fn slice_bits(src: &[u64; 9], lo: usize, nbits: usize) -> Vec<u8> {
            let mut out = vec![0u8; nbits.div_ceil(64) * 8];
            for i in 0..nbits {
                let bit = lo + i;
                if bit < 576 && (src[bit / 64] >> (bit % 64)) & 1 == 1 {
                    out[i / 8] |= 1 << (i % 8);
                }
            }
            out
        }
        let sel_read = |hi: usize, lo: usize| ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0),
            select: Some((hi, lo)),
            dynamic_select: None,
            width: hi - lo + 1,
            var_full_width: 576,
            expr_context: ctx(hi - lo + 1, false),
        };
        let dyn_read = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0),
            select: None,
            dynamic_select: Some(ProtoDynamicBitSelect {
                index_expr: Box::new(var_expr(VarOffset::Comb(72), 32)),
                elem_width: 192,
                window: 192,
                num_elements: 3,
            }),
            width: 192,
            var_full_width: 576,
            expr_context: ctx(192, false),
        };
        let mk_assign = |dst: isize, dw: usize, e: ProtoExpression| {
            ProtoStatement::Assign(ProtoAssignStatement {
                dst: VarOffset::Comb(dst),
                dst_width: dw,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: e,
                dst_ff_current_offset: 0,
                token: dummy_token(),
            })
        };
        // Two windowed reads as operands of a wider op: each is promoted from
        // its own 192-bit window, not from the 576-bit source.
        let xor256 = ProtoExpression::Binary {
            x: Box::new(sel_read(330, 139)),
            op: Op::BitXor,
            y: Box::new(sel_read(521, 330)),
            width: 256,
            expr_context: ctx(256, false),
        };
        // src576 at comb[0..72], index at comb[72..76], the results at
        // comb[80..104], [104..128], [128..152], [152..176] and [176..208].
        let src = emit_function(&[
            mk_assign(80, 192, sel_read(330, 139)),
            mk_assign(104, 160, sel_read(559, 400)),
            mk_assign(128, 192, dyn_read),
            // Reaching past the source reads zeros, not its neighbours.
            mk_assign(152, 192, sel_read(640, 449)),
            mk_assign(176, 256, xor256),
        ])
        .expect(">128-bit select reads must stay AOT-covered");
        assert!(
            src.contains("vw_lshr_win((uint8_t*)_w"),
            "the read is windowed to its result: {src}"
        );
        let tmp = std::env::temp_dir().join(format!("veryl_aot_wsr_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "emit_wide_select_reads_above_128_bits")
        else {
            return;
        };
        let words: [u64; 9] = [
            0x0123_4567_89AB_CDEF,
            0xFEDC_BA98_7654_3210,
            0xAAAA_5555_CCCC_3333,
            0xDEAD_BEEF_0BAD_F00D,
            0x1122_3344_5566_7788,
            0x99AA_BBCC_DDEE_FF00,
            0xF00D_FACE_CAFE_BEEF,
            0x0102_0304_0506_0708,
            0x8090_A0B0_C0D0_E0F0,
        ];
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 208];
        for (i, w) in words.iter().enumerate() {
            comb[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        let mut log = vec![0u64; 16];
        // Index 3 is out of range and clamps to the last element.
        for (idx, elem) in [(0u32, 0usize), (1, 1), (2, 2), (3, 2)] {
            comb[72..76].copy_from_slice(&idx.to_le_bytes());
            unsafe {
                (module.func)(
                    ff.as_mut_ptr(),
                    comb.as_mut_ptr(),
                    log.as_mut_ptr() as *mut u8,
                    0,
                );
            }
            assert_eq!(
                &comb[80..104],
                &slice_bits(&words, 139, 192)[..],
                "[330:139]"
            );
            assert_eq!(
                &comb[104..128],
                &slice_bits(&words, 400, 160)[..],
                "[559:400]"
            );
            assert_eq!(
                &comb[128..152],
                &slice_bits(&words, elem * 192, 192)[..],
                "element {idx}"
            );
            assert_eq!(
                &comb[152..176],
                &slice_bits(&words, 449, 192)[..],
                "[640:449]"
            );
            let (x, y) = (slice_bits(&words, 139, 192), slice_bits(&words, 330, 192));
            let mut want = [0u8; 32];
            for (i, w) in want.iter_mut().enumerate().take(24) {
                *w = x[i] ^ y[i];
            }
            assert_eq!(&comb[176..208], &want[..], "[330:139] ^ [521:330]");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn emit_expr_concatenation_64_bit_slot_drops_the_accumulator() {
        // Regression: emitted `((0ULL) << 64) | elem`, which gcc warns on
        // (-Wshift-count-overflow) and x86 evaluates as `acc | elem`.
        let a = var_expr(VarOffset::Comb(0), 64);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(a), 1, 64)],
            width: 64,
            expr_context: ctx(64, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(
            !s.contains("<< 64"),
            "u64 accumulator must not shift by 64: {s}"
        );

        // A __uint128_t accumulator must keep its 64-bit shift.
        let a = var_expr(VarOffset::Comb(0), 64);
        let b = var_expr(VarOffset::Comb(8), 4);
        let e = ProtoExpression::Concatenation {
            elements: vec![(Box::new(a), 1, 64), (Box::new(b), 1, 4)],
            width: 68,
            expr_context: ctx(68, false),
        };
        let s = emit_expr(&e).unwrap();
        assert!(s.contains("__uint128_t"));
        assert!(
            s.contains("(__uint128_t)0)) << 64"),
            "u128 accumulator keeps its 64-bit shift: {s}"
        );
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

    fn wide_bit_select_store(off: isize, hi: usize, lo: usize) -> String {
        let a = ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: 128,
            select: Some((hi, lo)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0xab, 8),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        };
        emit_stmt(&ProtoStatement::Assign(a)).unwrap()
    }

    #[test]
    fn emit_stmt_wide_bit_select_store_65_to_128() {
        // Byte 8 of the container holds [71:64] on its own, so the store is a
        // byte read-modify-write there rather than one over all 16 bytes.
        let s = wide_bit_select_store(0x40, 71, 64);
        assert!(!s.contains("__uint128_t"), "{s}");
        assert!(s.contains("uint8_t _o"), "{s}");
        assert!(s.contains("comb_values + 0x48"), "{s}");
        assert!(s.contains("_v << 0"), "{s}");
    }

    #[test]
    fn emit_stmt_wide_bit_select_store_straddling_field_keeps_the_128_bit_form() {
        // [71:56] spans bytes 7 and 8, so every aligned window up to 8 bytes
        // cuts it and the wide form has to stay reachable.
        let s = wide_bit_select_store(0x40, 71, 56);
        assert!(s.contains("veryl_u128_ua"), "{s}");
        assert!(s.contains("__uint128_t _o"), "{s}");
        assert!(s.contains("_v << 56"), "{s}");
    }

    /// A top-level single-bit store of `comb[off][bit]`.
    fn bit_store(off: isize, bit: usize) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: 128,
            select: Some((bit, bit)),
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(1, 1),
            dst_ff_current_offset: 0,
            token: dummy_token(),
        })
    }

    /// A statement that reads `[hi:lo]` of the variable at `off`.
    fn read_of(off: isize, hi: usize, lo: usize) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x900),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Variable {
                var_offset: VarOffset::Comb(off),
                select: Some((hi, lo)),
                dynamic_select: None,
                width: hi - lo + 1,
                var_full_width: 128,
                expr_context: ctx(hi - lo + 1, false),
            },
            dst_ff_current_offset: 0,
            token: dummy_token(),
        })
    }

    #[test]
    fn field_group_plans_a_fully_covered_byte() {
        // Eight disjoint single-bit stores define byte 1 of the slot outright.
        let stmts: Vec<_> = (8..16).map(|b| bit_store(0x40, b)).collect();
        let roles = plan_field_groups(&stmts).roles;
        assert_eq!(roles.len(), 8, "{roles:?}");
        assert_eq!(roles.get(&(0x41, 1 << 0)), Some(&FieldRole::Init));
        for b in 1..8u64 {
            assert_eq!(
                roles.get(&(0x41, 1 << b)),
                Some(&FieldRole::OrIn),
                "bit {b}"
            );
        }
    }

    #[test]
    fn field_group_gathers_its_members_before_the_last_one() {
        // Two unrelated statements sit between the bit stores; the plan sinks
        // the stores past them so the window is built in one run, and keeps
        // everything else in its original order.
        let mut stmts: Vec<ProtoStatement> = Vec::new();
        for b in 8..12 {
            stmts.push(bit_store(0x40, b));
        }
        stmts.push(read_of(0x200, 7, 0)); // index 4
        stmts.push(read_of(0x300, 7, 0)); // index 5
        for b in 12..16 {
            stmts.push(bit_store(0x40, b));
        }
        let plan = plan_field_groups(&stmts);
        assert_eq!(plan.roles.len(), 8);
        assert_eq!(plan.order, vec![4, 5, 0, 1, 2, 3, 6, 7, 8, 9]);
        assert_eq!(plan.atoms, vec![(2, 8)], "the run must be pinned");
    }

    /// A full-width scalar comb store — localization's candidate shape, and
    /// so the sink's.
    fn scalar_def(off: isize, reads: isize) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Variable {
                var_offset: VarOffset::Comb(reads),
                select: None,
                dynamic_select: None,
                width: 32,
                var_full_width: 32,
                expr_context: ctx(32, false),
            },
            dst_ff_current_offset: 0,
            token: dummy_token(),
        })
    }

    /// Arms the blocklist gate the sink shares with localization.
    fn armed<T>(f: impl FnOnce() -> T) -> T {
        set_localize_blocklist(HashSet::default(), Vec::new());
        let out = f();
        clear_localize_blocklist();
        out
    }

    #[test]
    fn sink_moves_a_single_reader_def_next_to_its_reader() {
        // 0x100 is written once and read once; the def joins its reader so
        // chunk-local localization can turn it into a C local.
        let stmts = vec![
            scalar_def(0x100, 0x900), // 0: the def
            scalar_def(0x200, 0x910), // 1
            scalar_def(0x300, 0x920), // 2
            scalar_def(0x400, 0x100), // 3: the only reader
        ];
        let plan = armed(|| plan_field_groups(&stmts));
        assert_eq!(plan.order, vec![1, 2, 0, 3]);
        assert_eq!(plan.atoms, vec![(2, 2)], "def and reader pinned together");
    }

    #[test]
    fn sink_declines_a_def_with_two_readers() {
        // Substituting a two-reader def would duplicate its work, and
        // localization would not take it either.
        let stmts = vec![
            scalar_def(0x100, 0x900),
            scalar_def(0x400, 0x100),
            scalar_def(0x500, 0x100),
        ];
        let plan = armed(|| plan_field_groups(&stmts));
        assert!(plan.atoms.is_empty(), "{:?}", plan.order);
    }

    #[test]
    fn sink_declines_when_an_input_is_rewritten_on_the_way() {
        // Moving the def past the statement that overwrites 0x900 would make
        // it read the new value.
        let stmts = vec![
            scalar_def(0x100, 0x900), // 0: reads 0x900
            scalar_def(0x900, 0x910), // 1: rewrites 0x900
            scalar_def(0x400, 0x100), // 2: the reader
        ];
        let plan = armed(|| plan_field_groups(&stmts));
        assert!(plan.atoms.is_empty(), "{:?}", plan.order);
    }

    #[test]
    fn sink_checks_the_input_against_the_final_position_of_a_moving_reader() {
        // 0x100's reader is itself sunk to 0x400's, so 0x100 travels past the
        // rewrite of 0x900 that sits between them.
        let stmts = vec![
            scalar_def(0x100, 0x900), // 0: reads 0x900
            scalar_def(0x200, 0x100), // 1: reads 0x100, sinks to 3
            scalar_def(0x900, 0x910), // 2: rewrites 0x900
            scalar_def(0x400, 0x200), // 3: reads 0x200
        ];
        let plan = armed(|| plan_field_groups(&stmts));
        assert_eq!(plan.order, vec![0, 2, 1, 3], "0's sink dropped, 1's kept");
        assert_eq!(plan.atoms, vec![(2, 2)]);
    }

    #[test]
    fn sink_chain_deeper_than_any_recursion_cap_keeps_every_statement() {
        // 70 links: every def's only reader is the next def, so the whole
        // chain arrives as one run — and none of it may go missing.
        let n = 70usize;
        let stmts: Vec<ProtoStatement> = (0..n)
            .map(|j| scalar_def(0x100 + j as isize * 4, 0x900 + j as isize * 4))
            .collect();
        let stmts: Vec<ProtoStatement> = (0..n)
            .map(|j| {
                if j == 0 {
                    stmts[0].clone()
                } else {
                    scalar_def(0x100 + j as isize * 4, 0x100 + (j - 1) as isize * 4)
                }
            })
            .collect();
        let plan = armed(|| plan_field_groups(&stmts));
        assert_eq!(plan.order, (0..n).collect::<Vec<_>>());
        assert_eq!(plan.atoms, vec![(0, n)]);
    }

    #[test]
    fn sink_declines_a_reader_above_the_def() {
        // The reader sees the previous settle's value; hoisting the def over
        // it would change that.
        let stmts = vec![
            scalar_def(0x400, 0x100), // 0: reads 0x100 (previous settle)
            scalar_def(0x100, 0x900), // 1: the def
        ];
        let plan = armed(|| plan_field_groups(&stmts));
        assert_eq!(plan.order, vec![0, 1]);
        assert!(plan.atoms.is_empty());
    }

    #[test]
    fn sink_localizes_and_executes_correctly() {
        // The def sinks to its reader, localization takes it, and the reader
        // still computes from the def's value.
        if !cc_available() {
            eprintln!("sink_localizes_and_executes_correctly: cc unavailable, skipping");
            return;
        }
        let stmts = vec![
            scalar_def(0x100, 0x900), // the def
            scalar_def(0x200, 0x910), // unrelated
            scalar_def(0x400, 0x100), // the only reader
        ];
        // 0x200/0x400 are blocklisted so they stay materialized and the
        // effect is observable in comb memory; 0x100 localizes away.
        set_localize_blocklist(HashSet::from_iter([0x200isize, 0x400isize]), Vec::new());
        let src = emit_function(&stmts);
        clear_localize_blocklist();
        let src = src.expect("sinkable comb must stay AOT-covered");
        assert!(
            src.contains(&local_name(0x100)),
            "the sunk def must localize:\n{src}"
        );
        assert!(
            !src.contains(&local_name(0x400)),
            "0x400 must stay in memory"
        );
        let tmp = std::env::temp_dir().join(format!("veryl_aot_sink_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "sink_localizes_and_executes") else {
            return;
        };
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 0x1000];
        comb[0x900..0x904].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        comb[0x910..0x914].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        let rd = |o: usize| u32::from_le_bytes(comb[o..o + 4].try_into().unwrap());
        assert_eq!(rd(0x400), 0xDEAD_BEEF, "reader sees the def's value");
        assert_eq!(rd(0x200), 0x1234_5678, "unrelated def unaffected");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sink_chains_land_as_one_run() {
        // a -> b -> c: all three end up adjacent so every link localizes.
        let stmts = vec![
            scalar_def(0x100, 0x900), // 0
            scalar_def(0x200, 0x100), // 1: reads 0
            scalar_def(0x800, 0x910), // 2: unrelated
            scalar_def(0x300, 0x200), // 3: reads 1
        ];
        let plan = armed(|| plan_field_groups(&stmts));
        assert_eq!(plan.order, vec![2, 0, 1, 3]);
        assert_eq!(plan.atoms, vec![(1, 3)]);
    }

    #[test]
    fn field_group_past_the_span_limit_keeps_its_roles_but_is_not_gathered() {
        // Dropping the merge is sound however far apart the stores sit; only
        // moving them is capped, so a group over the limit still gets roles
        // and stays where it is.
        let filler = 4000; // beyond any plausible VERYL_AOT_C_GATHER_SPAN
        let mut stmts: Vec<ProtoStatement> = vec![bit_store(0x40, 8)];
        stmts.extend((0..filler).map(|i| read_of(0x200 + i as isize * 8, 7, 0)));
        stmts.extend((9..16).map(|b| bit_store(0x40, b)));
        let plan = plan_field_groups(&stmts);
        assert_eq!(plan.roles.len(), 8, "roles survive the cap");
        assert!(plan.atoms.is_empty(), "nothing pinned");
        assert!(
            plan.order.iter().copied().eq(0..stmts.len()),
            "order unchanged"
        );
    }

    #[test]
    fn field_group_drops_a_window_a_member_reads() {
        // v[10] = v[9]: the read of bit 9 lands in the group's own window,
        // and its producer stores later — Init would zero what the
        // interpreter reads as the previous settle's value.
        let self_read = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x40),
            dst_width: 128,
            select: Some((10, 10)),
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Variable {
                var_offset: VarOffset::Comb(0x40),
                select: Some((9, 9)),
                dynamic_select: None,
                width: 1,
                var_full_width: 128,
                expr_context: ctx(1, false),
            },
            dst_ff_current_offset: 0,
            token: dummy_token(),
        });
        let mut stmts = vec![bit_store(0x40, 8), self_read];
        stmts.extend([9usize, 11, 12, 13, 14, 15].map(|b| bit_store(0x40, b)));
        assert!(plan_field_groups(&stmts).roles.is_empty());
    }

    #[test]
    fn field_group_init_orin_stores_execute_correctly() {
        // A full byte built by eight grouped single-bit stores: the emitted
        // Init/OrIn run must overwrite the previous settle's byte with the
        // assembled bits and leave the neighbours alone.
        if !cc_available() {
            eprintln!("field_group_init_orin_stores_execute_correctly: cc unavailable, skipping");
            return;
        }
        let stmts: Vec<ProtoStatement> = (8..16usize)
            .map(|b| {
                ProtoStatement::Assign(ProtoAssignStatement {
                    dst: VarOffset::Comb(0x40),
                    dst_width: 128,
                    select: Some((b, b)),
                    dynamic_select: None,
                    rhs_select: None,
                    expr: const_expr(((b % 2) == 0) as u64, 1),
                    dst_ff_current_offset: 0,
                    token: dummy_token(),
                })
            })
            .collect();
        let src = emit_function(&stmts).expect("grouped bit stores must stay AOT-covered");
        assert!(src.contains("|="), "roles were not armed:\n{src}");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_initorin_{}", std::process::id()));
        let Some(module) = compile_for_test(&tmp, &src, "field_group_init_orin_stores_execute")
        else {
            return;
        };
        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; 96];
        comb[0x40] = 0x5A; // neighbour bytes must survive
        comb[0x41] = 0xAA; // previous settle's value, fully redefined
        comb[0x42] = 0xC3;
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        // bit b of the byte gets (b % 2 == 0): bits 8,10,12,14 → 0b0101_0101.
        assert_eq!(comb[0x41], 0x55, "assembled byte");
        assert_eq!(comb[0x40], 0x5A, "byte below the window");
        assert_eq!(comb[0x42], 0xC3, "byte above the window");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn field_group_needs_every_bit_of_the_window() {
        // Seven of the byte's eight bits: the eighth keeps its old value, so
        // the merge cannot be dropped.
        let stmts: Vec<_> = (8..15).map(|b| bit_store(0x40, b)).collect();
        assert!(plan_field_groups(&stmts).roles.is_empty());
    }

    #[test]
    fn field_group_stops_at_a_read_of_the_window() {
        // A reader between the first and last store would see the window
        // half-built where today it sees the previous settle's value.
        let mut stmts: Vec<_> = (8..12).map(|b| bit_store(0x40, b)).collect();
        stmts.push(read_of(0x40, 15, 8));
        stmts.extend((12..16).map(|b| bit_store(0x40, b)));
        assert!(plan_field_groups(&stmts).roles.is_empty());

        // The same read after the last store is fine — the window is fully
        // defined by then either way.
        let mut after: Vec<_> = (8..16).map(|b| bit_store(0x40, b)).collect();
        after.push(read_of(0x40, 15, 8));
        assert_eq!(plan_field_groups(&after).roles.len(), 8);
    }

    #[test]
    fn field_group_ignores_a_read_of_a_neighbouring_byte() {
        // [7:0] lives in byte 0; the group owns byte 1.
        let mut stmts: Vec<_> = (8..12).map(|b| bit_store(0x40, b)).collect();
        stmts.push(read_of(0x40, 7, 0));
        stmts.extend((12..16).map(|b| bit_store(0x40, b)));
        assert_eq!(plan_field_groups(&stmts).roles.len(), 8);
    }

    #[test]
    fn field_group_drops_a_window_whose_field_is_stored_twice() {
        // Roles are keyed by field, so a second store of the same bit — here
        // inside a conditional — would pick up the first one's role.
        let mut stmts: Vec<_> = (8..16).map(|b| bit_store(0x40, b)).collect();
        stmts.push(ProtoStatement::If(ProtoIfStatement {
            cond: Some(const_expr(1, 1)),
            true_side: vec![bit_store(0x40, 9)],
            false_side: vec![],
        }));
        let roles = plan_field_groups(&stmts).roles;
        assert!(!roles.contains_key(&(0x41, 1 << 1)), "{roles:?}");
    }

    #[test]
    fn narrow_field_windows_contain_the_field_and_stay_in_the_container() {
        for lo in 0..128usize {
            for nbits in 1..=64usize {
                if lo + nbits > 128 {
                    continue;
                }
                let Some((start, w)) = narrow_field_window(lo, nbits) else {
                    continue;
                };
                assert!(matches!(w, 1 | 2 | 4 | 8), "lo={lo} nbits={nbits} w={w}");
                assert_eq!(start % w, 0, "lo={lo} nbits={nbits}: window unaligned");
                assert!(start + w <= 16, "lo={lo} nbits={nbits}: past the container");
                assert!(
                    start * 8 <= lo,
                    "lo={lo} nbits={nbits}: field starts before"
                );
                assert!(
                    lo + nbits <= (start + w) * 8,
                    "lo={lo} nbits={nbits}: field ends after"
                );
                // The shift has to keep the field inside the window's own width.
                assert!(lo - start * 8 + nbits <= w * 8, "lo={lo} nbits={nbits}");
            }
        }
        // A single bit always fits a byte — the case that dominates the count.
        for lo in 0..128usize {
            assert_eq!(narrow_field_window(lo, 1), Some((lo / 8, 1)), "lo={lo}");
        }
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

    #[test]
    fn ternary_both_signed_sext_128() {
        // Regression: a both-signed ternary whose branches are narrower than
        // the result sign-extends each branch to the result width
        // (LRM 11.4.11); results wider than 64 bits used to be declined.
        let t64_signed = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x1800),
            select: None,
            dynamic_select: None,
            width: 64,
            var_full_width: 64,
            expr_context: ctx(64, true),
        };
        let f8_signed = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x1810),
            select: None,
            dynamic_select: None,
            width: 8,
            var_full_width: 8,
            expr_context: ctx(8, true),
        };
        let tern = ProtoExpression::Ternary {
            cond: Box::new(var_expr(VarOffset::Comb(0x1820), 1)),
            true_expr: Box::new(t64_signed),
            false_expr: Box::new(f8_signed),
            width: 128,
            expr_context: ctx(128, true),
        };
        let stmts = vec![comb_assign(0x2000, 128, None, tern)];
        let src = emit_function(&stmts).expect("128-bit both-signed ternary must emit");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_tern128_{}", std::process::id()));
        if let Some(module) = compile_for_test(&tmp, &src, "tern128") {
            let mut ff = vec![0u8; 16];
            let mut comb = vec![0u8; 0x2020];
            let mut log = vec![0u64; 16];
            // cond=0: f = -2 (signed 8-bit) → all-ones down to ...fe.
            comb[0x1810] = 0xfe;
            unsafe {
                (module.func)(
                    ff.as_mut_ptr(),
                    comb.as_mut_ptr(),
                    log.as_mut_ptr() as *mut u8,
                    0,
                );
            }
            let lo = u64::from_le_bytes(comb[0x2000..0x2008].try_into().unwrap());
            let hi = u64::from_le_bytes(comb[0x2008..0x2010].try_into().unwrap());
            assert_eq!(
                (lo, hi),
                (0xffff_ffff_ffff_fffe, 0xffff_ffff_ffff_ffff),
                "8-bit -2 must sign-extend across all 128 bits"
            );
            // cond=1: t = 64-bit negative → high word all-ones.
            comb[0x1820] = 1;
            comb[0x1800..0x1808].copy_from_slice(&0x8000_0000_0000_0123u64.to_le_bytes());
            unsafe {
                (module.func)(
                    ff.as_mut_ptr(),
                    comb.as_mut_ptr(),
                    log.as_mut_ptr() as *mut u8,
                    0,
                );
            }
            let lo = u64::from_le_bytes(comb[0x2000..0x2008].try_into().unwrap());
            let hi = u64::from_le_bytes(comb[0x2008..0x2010].try_into().unwrap());
            assert_eq!(
                (lo, hi),
                (0x8000_0000_0000_0123, 0xffff_ffff_ffff_ffff),
                "negative 64-bit branch must sign-extend into the high word"
            );
            let _ = fs::remove_dir_all(&tmp);
        }
    }

    #[test]
    fn ternary_branch_via_wide_intermediate() {
        // A ternary branch narrow enough for the scalar emitter but built on
        // a 184-bit intermediate, which only the wide pipeline can compute.
        // Without the fallback the branch declines and takes the whole
        // statement off the AOT-C path.
        let concat = ProtoExpression::Concatenation {
            elements: vec![
                (Box::new(var_expr(VarOffset::Comb(0x1800), 92)), 1, 92),
                (Box::new(var_expr(VarOffset::Comb(0x1820), 92)), 1, 92),
            ],
            width: 184,
            expr_context: ctx(184, false),
        };
        let branch = ProtoExpression::Binary {
            x: Box::new(concat),
            op: Op::BitAnd,
            y: Box::new(var_expr(VarOffset::Comb(0x1840), 184)),
            width: 128,
            expr_context: ctx(128, false),
        };
        let tern = ProtoExpression::Ternary {
            cond: Box::new(var_expr(VarOffset::Comb(0x1860), 1)),
            true_expr: Box::new(branch),
            false_expr: Box::new(var_expr(VarOffset::Comb(0x1870), 128)),
            width: 128,
            expr_context: ctx(128, false),
        };
        let stmts = vec![comb_assign(0x2000, 128, None, tern)];
        let src = emit_function(&stmts)
            .expect("a wide-only ternary branch must not decline the statement");
        let tmp = std::env::temp_dir().join(format!("veryl_aot_tvw_{}", std::process::id()));
        if let Some(module) = compile_for_test(&tmp, &src, "tern_via_wide") {
            let hi: u128 = 5;
            let lo: u128 = 0x123_4567_89AB_CDEF_0123_4567;
            let mut ff = vec![0u8; 16];
            let mut comb = vec![0u8; 0x2010];
            comb[0x1800..0x1810].copy_from_slice(&hi.to_le_bytes());
            comb[0x1820..0x1830].copy_from_slice(&lo.to_le_bytes());
            comb[0x1840..0x1857].fill(0xff); // 184-bit all-ones mask
            comb[0x1860] = 1;
            let mut log = vec![0u64; 16];
            unsafe {
                (module.func)(
                    ff.as_mut_ptr(),
                    comb.as_mut_ptr(),
                    log.as_mut_ptr() as *mut u8,
                    0,
                );
            }
            assert_eq!(
                u128::from_le_bytes(comb[0x2000..0x2010].try_into().unwrap()),
                (hi << 92) | lo,
                "the 184-bit concat must narrow to its low 128 bits"
            );
            let _ = fs::remove_dir_all(&tmp);
        }
    }

    #[test]
    fn ternary_both_signed_declines_branch_wider_than_result() {
        // A both-signed ternary result narrower than one of its branches has
        // no 64-bit sign-extension; it must decline instead of underflowing
        // the shift count.
        let t8_signed = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x1800),
            select: None,
            dynamic_select: None,
            width: 8,
            var_full_width: 8,
            expr_context: ctx(8, true),
        };
        let f100_signed = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x1810),
            select: None,
            dynamic_select: None,
            width: 100,
            var_full_width: 100,
            expr_context: ctx(100, true),
        };
        let tern = ProtoExpression::Ternary {
            cond: Box::new(var_expr(VarOffset::Comb(0x1830), 1)),
            true_expr: Box::new(t8_signed),
            false_expr: Box::new(f100_signed),
            width: 64,
            expr_context: ctx(64, true),
        };
        let stmts = vec![comb_assign(0x2000, 64, None, tern)];
        assert!(emit_function(&stmts).is_none());
    }

    // --- Clean-bits mask elision (expr_emits_clean) ---
    // A wrongly-elided mask stores dirty high bits (silent divergence from
    // Cranelift, caught by VERYL_AOT_C_VALIDATE byte compares) — each rule
    // direction gets a direct emit-string test.

    #[test]
    fn clean_store_elides_mask_for_predicate() {
        // 1-bit dst, RHS = (a == b): the compare produces 0/1, so the store
        // width mask is a no-op and must be gone.
        let cmp = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0x00), 32)),
            op: Op::Eq,
            y: Box::new(var_expr(VarOffset::Comb(0x08), 32)),
            width: 1,
            expr_context: ctx(1, false),
        };
        let s = emit_stmt(&comb_assign(0x10, 1, None, cmp)).unwrap();
        assert!(!s.contains("& 0x1ULL"), "store mask must be elided: {s}");
    }

    #[test]
    fn clean_store_keeps_mask_for_dirty_add() {
        // 7-bit dst, RHS = a + b (carry can reach bit 7): mask must stay.
        let add = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0x00), 7)),
            op: Op::Add,
            y: Box::new(var_expr(VarOffset::Comb(0x08), 7)),
            width: 7,
            expr_context: ctx(7, false),
        };
        let s = emit_stmt(&comb_assign(0x10, 7, None, add)).unwrap();
        assert!(s.contains("& 0x7fULL"), "dirty add keeps the mask: {s}");
    }

    #[test]
    fn clean_wrapper_and_collapses_to_operand() {
        // The wrapper shape `load & width_mask` over a canonical full load is
        // an identity — the emitted C must be the bare load.
        let wrapped = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0x20), 32)),
            op: Op::BitAnd,
            y: Box::new(const_expr(0xffff_ffff, 32)),
            width: 32,
            expr_context: ctx(32, false),
        };
        let s = emit_expr(&wrapped).unwrap();
        assert!(!s.contains('&'), "identity AND must vanish: {s}");
    }

    #[test]
    fn clean_wrapper_and_kept_for_dirty_operand() {
        // Same wrapper over `~x` (dirty under needs_clean=false): keep it.
        let wrapped = ProtoExpression::Binary {
            x: Box::new(ProtoExpression::Unary {
                op: Op::BitNot,
                x: Box::new(var_expr(VarOffset::Comb(0x20), 32)),
                width: 32,
                expr_context: ctx(32, false),
            }),
            op: Op::BitAnd,
            y: Box::new(const_expr(0xffff_ffff, 32)),
            width: 32,
            expr_context: ctx(32, false),
        };
        let s = emit_expr_root(&wrapped).unwrap();
        assert!(s.contains('&'), "dirty operand keeps the AND: {s}");
    }

    #[test]
    fn clean_analysis_rejects_signed_bitwise() {
        // Signed context sign-extends narrow operands and the bitwise
        // result is not re-masked — never clean.
        let e = ProtoExpression::Binary {
            x: Box::new(var_expr(VarOffset::Comb(0x00), 8)),
            op: Op::BitOr,
            y: Box::new(var_expr(VarOffset::Comb(0x08), 8)),
            width: 16,
            expr_context: ctx(16, true),
        };
        assert!(!expr_emits_clean(&e));
    }

    #[test]
    fn clean_analysis_variable_rules() {
        // Canonical full load: clean.  A width-mismatched load shape: not.
        assert!(expr_emits_clean(&var_expr(VarOffset::Comb(0x00), 32)));
        let partial = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x00),
            select: None,
            dynamic_select: None,
            width: 8,
            var_full_width: 32,
            expr_context: ctx(8, false),
        };
        assert!(!expr_emits_clean(&partial));
        // Select extracts mask themselves: clean.
        let sel = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x00),
            select: Some((11, 4)),
            dynamic_select: None,
            width: 8,
            var_full_width: 32,
            expr_context: ctx(8, false),
        };
        assert!(expr_emits_clean(&sel));
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
    #[test]
    #[cfg(unix)]
    fn compile_script_hands_every_flag_to_the_compiler() {
        // Regression: the script's `shift` count and the argument list must
        // agree.  With one argument missing, `$lk` swallowed the first flag
        // and `-O3` never reached the compile.
        let tmp = std::env::temp_dir().join(format!("veryl_aot_args_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let seen = tmp.join("seen.txt");
        let fake_cc = tmp.join("record_cc.sh");
        fs::write(
            &fake_cc,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n\
                 while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n: > \"$out\"\n",
                seen.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_cc, fs::Permissions::from_mode(0o755)).unwrap();

        let tmp_c = tmp.join("t.c");
        let tmp_so = tmp.join("t.so");
        let c_path = tmp.join("p.c");
        let so_path = tmp.join("p.so");
        let lock = tmp.join("p.lock");
        let log = tmp.join("p.log");
        fs::write(&tmp_c, b"").unwrap();
        fs::write(&lock, b"").unwrap();
        let flags = ["-O3".to_string(), "-fPIC".to_string()];

        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(COMPILE_SCRIPT)
            .args(compile_script_args(
                &CompileScriptPaths {
                    cc: &fake_cc.to_string_lossy(),
                    tmp_so: &tmp_so,
                    tmp_c: &tmp_c,
                    published_c: &c_path,
                    published_so: &so_path,
                    lock: Some(&lock),
                    log: &log,
                },
                &flags,
            ))
            .status()
            .unwrap();
        assert!(status.success(), "the script must succeed");
        let recorded = fs::read_to_string(&seen).unwrap();
        for f in &flags {
            assert!(
                recorded.lines().any(|l| l == f),
                "{f} must reach the compiler, got: {recorded}"
            );
        }
        assert!(so_path.exists(), "the artifact must publish");
        assert!(c_path.exists(), "the source must publish beside it");
        assert!(!log.exists(), "a successful compile leaves no log behind");
        assert!(!lock.exists(), "the script must release the lock");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    #[cfg(unix)]
    fn aot_cache_publish_survives_process_exit() {
        // A short run exits while cc is still going, so the publish must not
        // depend on this process surviving to rename the temp files.  The
        // test re-executes itself as a child that starts one compile and
        // exits; the parent then waits for the artifact to appear.
        const CHILD_DIR: &str = "VERYL_TEST_AOT_PUBLISH_DIR";
        if let Ok(dir) = std::env::var(CHILD_DIR) {
            let dir = PathBuf::from(dir);
            thread::spawn(move || {
                let _ = compile_source_in(&dir, "// AOT-C publish probe\n");
            });
            thread::sleep(Duration::from_millis(200));
            std::process::exit(0);
        }

        let tmp = std::env::temp_dir().join(format!("veryl_aot_pub_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        // A cc that outlives the child, and that emits a diagnostic once it
        // does — on our pipes that is a SIGPIPE, and the compile would die
        // before writing anything.
        let slow_cc = tmp.join("slow_cc.sh");
        fs::write(
            &slow_cc,
            "#!/bin/sh\nsleep 2\necho 'warning: probe diagnostic' >&2\n\
             while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n: > \"$out\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&slow_cc, fs::Permissions::from_mode(0o755)).unwrap();

        let status = Command::new(std::env::current_exe().unwrap())
            .arg("aot_cache_publish_survives_process_exit")
            .env(CHILD_DIR, &tmp)
            .env("VERYL_AOT_CC", &slow_cc)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "child run failed");

        // Published artifacts are `veryl_aot_<hash>.so`; the temps carry an
        // extra `.<pid>.<n>` and must not count.
        let published = || {
            fs::read_dir(&tmp).into_iter().flatten().flatten().any(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("veryl_aot_") && n.ends_with(".so") && n.matches('.').count() == 1
            })
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while !published() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        assert!(published(), "the .so must publish after the run exits");
        let _ = fs::remove_dir_all(&tmp);
    }

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

        let _ = fs::remove_dir_all(&tmp);
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
    fn gc_sweeps_only_stale_temp_artifacts() {
        let dir = std::env::temp_dir().join(format!("veryl_gc_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let published = dir.join("veryl_aot_dead.c");
        let published_so = dir.join("veryl_aot_dead.so");
        let log = dir.join("veryl_aot_dead.123.0.log");
        let stale = dir.join("veryl_aot_dead.123.0.c");
        let stale_so = dir.join("veryl_aot_dead.123.0.so");
        let fresh = dir.join("veryl_aot_beef.456.1.c");
        for f in [&published, &published_so, &log, &stale, &stale_so, &fresh] {
            fs::write(f, "x").unwrap();
        }
        let now = std::time::SystemTime::now();
        for f in [&published, &published_so, &log, &stale, &stale_so] {
            fs::File::options()
                .write(true)
                .open(f)
                .unwrap()
                .set_modified(now - Duration::from_secs(7200))
                .unwrap();
        }

        sweep_temp_artifacts(&dir, now - Duration::from_secs(3600));

        assert!(published.exists(), "a published .c is not a temp");
        assert!(published_so.exists(), "a published .so is not a temp");
        assert!(
            log.exists(),
            "a failed compile's log is the only record of why"
        );
        assert!(fresh.exists(), "a compile that may still be running");
        assert!(!stale.exists() && !stale_so.exists());
        let _ = fs::remove_dir_all(&dir);
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
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A source shaped like the chunked emit, with `count` chunks each storing
    /// its own index, and an entry that calls them all.
    #[cfg(unix)]
    fn chunked_stub_source(count: usize) -> String {
        let mut s = String::from(
            "// AOT-C generated (noslp); do not edit.\n\
             #include <stdint.h>\n\
             typedef struct { int unused; } veryl_wideops_t;\n\
             __attribute__((visibility(\"default\"))) veryl_wideops_t veryl_wideops;\n\
             __attribute__((visibility(\"default\"))) void veryl_set_wideops(const void* t) { veryl_wideops = *(const veryl_wideops_t*)t; }\n\
             static inline uint32_t vw_tag(uint32_t i) { return i + 1; }\n",
        );
        for i in 0..count {
            s.push_str(&format!(
                "{CHUNK_FN_MARKER}{i}(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log) {{\n\
                 (void)ff_values; (void)write_log;\n\
                 *(uint32_t*)(comb_values + {}) = vw_tag({i});\n\
                 }}\n",
                i * 4
            ));
        }
        s.push_str(
            "__attribute__((visibility(\"default\")))\n\
             void veryl_aot_eval(uint8_t *__restrict__ ff_values, uint8_t *__restrict__ comb_values, uint64_t *__restrict__ write_log, intptr_t ff_delta) {\n\
             (void)ff_delta;\n",
        );
        for i in 0..count {
            s.push_str(&format!(
                "veryl_aot_chunk_{i}(ff_values, comb_values, write_log);\n"
            ));
        }
        s.push_str("}\n");
        s
    }

    #[test]
    #[cfg(unix)]
    fn split_units_compile_to_the_same_behaviour() {
        // The split rewrites the source that reaches `cc`, so the guarantee
        // worth testing is behavioural: every chunk still runs, and reaches
        // the header's `static inline` helper from whichever unit it landed in.
        if Command::new(std::env::var("VERYL_AOT_CC").unwrap_or_else(|_| "cc".to_string()))
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("split_units_compile_to_the_same_behaviour: cc unavailable, skipping");
            return;
        }
        const CHUNKS: usize = 12;
        let src = chunked_stub_source(CHUNKS);
        assert!(
            split_translation_units(&src, 4).is_some(),
            "the stub must have the shape the split recognises"
        );

        let tmp = std::env::temp_dir().join(format!("veryl_aot_split_{}", std::process::id()));
        TEST_TU_SPLIT.with(|c| c.set(Some(4)));
        let module = compile_for_test(&tmp, &src, "split_units_compile_to_the_same_behaviour");
        TEST_TU_SPLIT.with(|c| c.set(None));
        let Some(module) = module else { return };

        let mut ff = vec![0u8; 16];
        let mut comb = vec![0u8; CHUNKS * 4];
        let mut log = vec![0u64; 16];
        unsafe {
            (module.func)(
                ff.as_mut_ptr(),
                comb.as_mut_ptr(),
                log.as_mut_ptr() as *mut u8,
                0,
            );
        }
        for i in 0..CHUNKS {
            let got = u32::from_le_bytes(comb[i * 4..i * 4 + 4].try_into().unwrap());
            assert_eq!(got, i as u32 + 1, "chunk {i} did not run");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn split_declines_sources_it_does_not_recognise() {
        // Anything without the chunked shape must compile whole rather than
        // be cut at a guess.
        assert!(split_translation_units("int main(void) { return 0; }", 4).is_none());
        // Too few chunks to fill the units.
        assert!(
            split_translation_units(
                &{
                    let mut s = String::new();
                    for i in 0..3 {
                        s.push_str(&format!("{CHUNK_FN_MARKER}{i}(void) {{}}\n"));
                    }
                    s
                },
                4
            )
            .is_none()
        );
        // An exported symbol the split cannot give every unit.
        let mut extra = String::from("#include <stdint.h>\n");
        extra.push_str("__attribute__((visibility(\"default\"))) int veryl_extra_global;\n");
        for i in 0..12 {
            extra.push_str(&format!("{CHUNK_FN_MARKER}{i}(void) {{}}\n"));
        }
        extra.push_str("__attribute__((visibility(\"default\")))\nvoid veryl_aot_eval(void) {}\n");
        assert!(split_translation_units(&extra, 4).is_none());
    }

    #[test]
    fn split_count_is_a_function_of_the_source_alone() {
        // Load-dependent splitting would give one cache key differing binaries
        // and make a split-only bug reproduce only sometimes.
        let small = "x".repeat(TU_SPLIT_BYTES - 1);
        let large = "x".repeat(TU_SPLIT_BYTES * 3);
        assert_eq!(tu_split_count(&small), 1);
        assert_eq!(tu_split_count(&large), 3);
        assert_eq!(
            tu_split_count(&"x".repeat(TU_SPLIT_BYTES * (TU_SPLIT_MAX + 4))),
            TU_SPLIT_MAX
        );
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
    ) -> Vec<HashSet<isize>> {
        let bl: HashSet<isize> = blocklist.iter().copied().collect();
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
    fn localize_skips_read_before_write() {
        // `y = v` placed BEFORE `v = const` in the same chunk — see
        // `LocalAnalysis::read_before_write`.
        let c0 = vec![
            comb_assign(0x20, 32, None, var_expr(VarOffset::Comb(0x10), 32)),
            comb_assign(0x10, 32, None, const_expr(0, 32)),
        ];
        let sets = localize_sets(&[&c0], &[], &[]);
        assert!(
            !sets[0].contains(&0x10),
            "read-before-write (backward edge) must not localize"
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

    // ── bit-test merging ────────────────────────────────────────────────

    fn bit(off: isize, b: usize, full: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: Some((b, b)),
            dynamic_select: None,
            width: 1,
            var_full_width: full,
            expr_context: ctx(1, false),
        }
    }

    fn bnot(e: ProtoExpression) -> ProtoExpression {
        ProtoExpression::Unary {
            op: Op::LogicNot,
            x: Box::new(e),
            width: 1,
            expr_context: ctx(1, false),
        }
    }

    fn bjoin(op: Op, es: Vec<ProtoExpression>) -> ProtoExpression {
        es.into_iter()
            .reduce(|a, b| ProtoExpression::Binary {
                x: Box::new(a),
                op,
                y: Box::new(b),
                width: 1,
                expr_context: ctx(1, false),
            })
            .unwrap()
    }

    #[test]
    fn bitmerge_folds_an_and_bundle() {
        // a[0] & a[1] & !a[2] -> (a & 0x7) == 0x3.
        let e = bjoin(
            Op::BitAnd,
            vec![bit(0x0, 0, 16), bit(0x0, 1, 16), bnot(bit(0x0, 2, 16))],
        );
        let src = emit_bit_test_merge(&e).unwrap();
        assert!(src.contains("& 0x7ULL) == 0x3ULL"), "{src}");
    }

    #[test]
    fn bitmerge_folds_an_or_bundle_and_keeps_a_negated_leaf_plain() {
        // !a[0] | a[1] | a[2]: the negated leaf has no masked-compare form
        // under Or; the positive pair still folds.
        let e = bjoin(
            Op::BitOr,
            vec![bnot(bit(0x0, 0, 16)), bit(0x0, 1, 16), bit(0x0, 2, 16)],
        );
        let src = emit_bit_test_merge(&e).unwrap();
        assert!(src.contains("& 0x6ULL) != 0"), "{src}");
    }

    #[test]
    fn bitmerge_folds_an_xor_bundle_to_a_parity() {
        let e = bjoin(
            Op::BitXor,
            vec![bit(0x0, 0, 16), bit(0x0, 1, 16), bit(0x0, 2, 16)],
        );
        let src = emit_bit_test_merge(&e).unwrap();
        assert!(src.contains("__builtin_parityll"), "{src}");
        assert!(src.contains("0x7ULL"), "{src}");
        // A negated leaf becomes one constant parity flip.
        let e = bjoin(Op::BitXor, vec![bnot(bit(0x0, 0, 16)), bit(0x0, 1, 16)]);
        let src = emit_bit_test_merge(&e).unwrap();
        assert!(src.contains("0x1ULL ^"), "{src}");
    }

    #[test]
    fn bitmerge_bails_on_opposite_polarities_of_one_bit() {
        // a[3] & !a[3] cannot be a masked compare.
        let e = bjoin(Op::BitAnd, vec![bit(0x0, 3, 16), bnot(bit(0x0, 3, 16))]);
        assert!(emit_bit_test_merge(&e).is_none());
    }

    #[test]
    fn bitmerge_bails_on_a_duplicated_xor_leaf() {
        // a[3] ^ a[3] == 0; a single mask bit would emit a[3].
        let e = bjoin(Op::BitXor, vec![bit(0x0, 3, 16), bit(0x0, 3, 16)]);
        assert!(emit_bit_test_merge(&e).is_none());
        // Chained: a[1] ^ a[3] ^ a[3] == a[1].
        let e = bjoin(
            Op::BitXor,
            vec![bit(0x0, 1, 16), bit(0x0, 3, 16), bit(0x0, 3, 16)],
        );
        assert!(emit_bit_test_merge(&e).is_none());
        // Negated duplicates: !a[3] ^ !a[3] == 0.
        let e = bjoin(
            Op::BitXor,
            vec![bnot(bit(0x0, 3, 16)), bnot(bit(0x0, 3, 16))],
        );
        assert!(emit_bit_test_merge(&e).is_none());
        // Under And the same duplicate IS idempotent and still folds with
        // a second bit.
        let e = bjoin(
            Op::BitAnd,
            vec![bit(0x0, 3, 16), bit(0x0, 3, 16), bit(0x0, 4, 16)],
        );
        assert!(emit_bit_test_merge(&e).is_some());
    }

    #[test]
    fn bitmerge_needs_two_tests_of_one_variable() {
        // Different variables (offsets) never group.
        let e = bjoin(Op::BitAnd, vec![bit(0x0, 0, 16), bit(0x40, 1, 16)]);
        assert!(emit_bit_test_merge(&e).is_none());
    }

    #[test]
    fn bitmerge_excludes_a_wide_variable() {
        let e = bjoin(Op::BitAnd, vec![bit(0x0, 0, 65), bit(0x0, 1, 65)]);
        assert!(emit_bit_test_merge(&e).is_none());
    }

    // ── const-cone partition ────────────────────────────────────────────

    fn cassign(dst: isize, w: usize, expr: ProtoExpression) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(dst),
            dst_width: w,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr,
            dst_ff_current_offset: 0,
            token: dummy_token(),
        })
    }

    fn cdyn_write(base: isize, stride: isize, num: usize) -> ProtoStatement {
        ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(base),
            dst_stride: stride,
            dst_num_elements: num,
            dst_index_expr: var_expr(VarOffset::Ff(0x40), 8),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: const_expr(0, 32),
            dst_ff_current_base_offset: 0,
        })
    }

    #[test]
    fn const_partition_moves_literal_cones_to_the_front() {
        // stmt0 non-const (FF read), stmt1 const literal, stmt2 const
        // reader of stmt1: the const pair moves to the front, in order.
        let stmts = vec![
            cassign(0x10, 32, var_expr(VarOffset::Ff(0), 32)),
            cassign(0x0, 32, const_expr(7, 32)),
            cassign(0x8, 32, var_expr(VarOffset::Comb(0x0), 32)),
        ];
        let (out, n, _) = const_cone_partition(&stmts, &HashSet::default()).unwrap();
        assert_eq!(n, 2);
        assert!(matches!(&out[0], ProtoStatement::Assign(a) if a.dst == VarOffset::Comb(0x0)));
        assert!(matches!(&out[1], ProtoStatement::Assign(a) if a.dst == VarOffset::Comb(0x8)));
        assert!(matches!(&out[2], ProtoStatement::Assign(a) if a.dst == VarOffset::Comb(0x10)));
    }

    #[test]
    fn const_partition_respects_event_written_offsets() {
        // The same cone with the def's offset in `unsafe_comb`: the WRITER
        // must be demoted (rerunning it every settle is what clobbers the
        // event's value) and the reader with it.
        let stmts = vec![
            cassign(0x0, 32, const_expr(7, 32)),
            cassign(0x8, 32, var_expr(VarOffset::Comb(0x0), 32)),
        ];
        assert_eq!(
            const_cone_partition(&stmts, &HashSet::default()).map(|(_, n, _)| n),
            Some(2)
        );
        let unsafe_comb = HashSet::from_iter([0x0isize]);
        assert!(const_cone_partition(&stmts, &unsafe_comb).is_none());
    }

    #[test]
    fn const_partition_demotes_a_reader_ahead_of_its_writer() {
        // Back-edge: the reader precedes its writer in (topo-sorted) order,
        // so it reads last settle's value — the READER is never const.  The
        // literal writer itself stays const: the SCC converges over
        // `required_comb_passes` to the same fixpoint either way.
        let stmts = vec![
            cassign(0x8, 32, var_expr(VarOffset::Comb(0x0), 32)),
            cassign(0x0, 32, const_expr(7, 32)),
        ];
        let (out, n, _) = const_cone_partition(&stmts, &HashSet::default()).unwrap();
        assert_eq!(n, 1);
        assert!(matches!(&out[0], ProtoStatement::Assign(a) if a.dst == VarOffset::Comb(0x0)));
    }

    #[test]
    fn const_partition_demotes_a_self_rmw() {
        let stmts = vec![cassign(0x0, 32, var_expr(VarOffset::Comb(0x0), 32))];
        assert!(const_cone_partition(&stmts, &HashSet::default()).is_none());
    }

    #[test]
    fn const_partition_taints_a_dynamic_write_range() {
        // A dynamic write covers [0x0, 0x20): the const write of 0x8 (an
        // interior element) is demoted; a write of 0x20 (one past the
        // range) survives.
        let in_range = vec![cdyn_write(0x0, 0x8, 4), cassign(0x8, 32, const_expr(7, 32))];
        assert!(const_cone_partition(&in_range, &HashSet::default()).is_none());
        let past_range = vec![
            cdyn_write(0x0, 0x8, 4),
            cassign(0x20, 32, const_expr(7, 32)),
        ];
        assert_eq!(
            const_cone_partition(&past_range, &HashSet::default()).map(|(_, n, _)| n),
            Some(1)
        );
    }

    #[test]
    fn const_partition_demotes_a_const_co_writer_of_a_nonconst_statement() {
        // The If reads an FF (never const) and conditionally writes 0x0;
        // the unconditional const write of 0x0 must be demoted, or it
        // freezes a value the If keeps overwriting.
        let cond_write = ProtoStatement::If(ProtoIfStatement {
            cond: Some(var_expr(VarOffset::Ff(0), 1)),
            true_side: vec![cassign(0x0, 32, const_expr(1, 32))],
            false_side: vec![],
        });
        let stmts = vec![cond_write, cassign(0x0, 32, const_expr(7, 32))];
        assert!(const_cone_partition(&stmts, &HashSet::default()).is_none());
    }

    #[test]
    fn const_partition_walks_compiled_block_originals() {
        // The CB's compressed output list names base + last element only;
        // its ORIGINAL statements dynamically write the whole range, so
        // the const write of the MIDDLE element (0x8) must be demoted.
        let cb = crate::ir::CompiledBlockStatement {
            artifact: bogus_artifact(),
            ff_delta_bytes: 0,
            comb_delta_bytes: 0,
            input_offsets: vec![],
            output_offsets: vec![VarOffset::Comb(0x0), VarOffset::Comb(0x10)],
            ff_canonical_offsets: vec![],
            stmt_deps: vec![],
            original_stmts: vec![cdyn_write(0x0, 0x8, 3)],
        };
        let stmts = vec![
            ProtoStatement::CompiledBlock(cb),
            cassign(0x8, 32, const_expr(7, 32)),
        ];
        assert!(const_cone_partition(&stmts, &HashSet::default()).is_none());
    }

    #[test]
    fn const_partition_disarms_on_an_unboundable_statement() {
        let stmts = vec![
            cassign(0x0, 32, const_expr(7, 32)),
            ProtoStatement::SystemFunctionCall(ProtoSystemFunctionCall::Readmemh {
                filename: "x.hex".into(),
                elements: vec![],
                width: 32,
            }),
        ];
        assert!(const_cone_partition(&stmts, &HashSet::default()).is_none());
    }

    #[test]
    fn const_split_emits_a_run_once_entry() {
        // Armed: the const literal goes to `veryl_aot_eval_const`
        // (chunk 0) and the main entry runs only the non-const chunk.
        // Unarmed: the export must be absent — pins that the feature
        // cannot silently arm without `set_const_unsafe`.
        let stmts = vec![
            cassign(0x0, 32, const_expr(7, 32)),
            cassign(0x10, 32, var_expr(VarOffset::Ff(0), 32)),
        ];
        let unarmed = emit_function(&stmts).unwrap();
        assert!(!unarmed.contains("veryl_aot_eval_const"));
        set_const_unsafe(HashSet::default());
        let armed = emit_function(&stmts);
        clear_const_unsafe();
        let armed = armed.unwrap();
        let const_entry = armed
            .split("void veryl_aot_eval_const(")
            .nth(1)
            .expect("armed emit must export the const entry");
        let (const_body, rest) = const_entry.split_once("void veryl_aot_eval(").unwrap();
        assert!(const_body.contains("veryl_aot_chunk_0("));
        assert!(!const_body.contains("veryl_aot_chunk_1("));
        assert!(rest.contains("veryl_aot_chunk_1("));
        assert!(!rest.contains("veryl_aot_chunk_0("));
    }

    #[test]
    fn const_split_survives_a_field_group_gather() {
        // A const literal ahead of a two-store field group: the gather
        // reorders the group into one atom, the re-count keeps the const
        // prefix, and the run-once entry still emits.
        let stmts = vec![
            cassign(0x0, 32, const_expr(7, 32)),
            ProtoStatement::Assign(ProtoAssignStatement {
                dst: VarOffset::Comb(0x10),
                dst_width: 32,
                select: Some((15, 0)),
                dynamic_select: None,
                rhs_select: None,
                expr: var_expr(VarOffset::Ff(0), 16),
                dst_ff_current_offset: 0,
                token: dummy_token(),
            }),
            cassign(0x20, 32, var_expr(VarOffset::Ff(8), 32)),
            ProtoStatement::Assign(ProtoAssignStatement {
                dst: VarOffset::Comb(0x10),
                dst_width: 32,
                select: Some((31, 16)),
                dynamic_select: None,
                rhs_select: None,
                expr: var_expr(VarOffset::Ff(2), 16),
                dst_ff_current_offset: 0,
                token: dummy_token(),
            }),
        ];
        set_const_unsafe(HashSet::default());
        let src = emit_function(&stmts);
        clear_const_unsafe();
        let src = src.unwrap();
        assert!(src.contains("veryl_aot_eval_const"));
    }
}
