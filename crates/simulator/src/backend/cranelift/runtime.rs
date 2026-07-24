use crate::ir::opt::load_cache_lookahead::{FutureReads, compute_read_positions};
use crate::ir::{Config, ProtoStatement, VarOffset};
use crate::{HashMap, HashSet};
use cranelift::codegen::control::ControlPlane;
use cranelift::codegen::ir::{AbiParam, Function, SigRef, Signature, StackSlotData, UserFuncName};
use cranelift::codegen::isa::{self, CallConv};
use cranelift::codegen::{self, settings};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift::prelude::types::{I32, I64};
use cranelift::prelude::*;
use indent::indent_all_by;
use target_lexicon::Triple;

pub use crate::FuncPtr;

/// Signature kinds for `call_indirect` helper calls.
#[derive(Hash, Eq, PartialEq, Clone, Copy)]
pub enum HelperSig {
    /// `(dst, a, b/amount, nb) -> ()` — binary ops, shifts.
    BinaryOp,
    /// `(dst, a, nb) -> ()` — unary ops, copy.
    UnaryOp,
    /// `(a, b, nb) -> i64` — comparisons.
    Compare,
    /// `(a, b, a_packed, b_packed) -> i64` — asymmetric-width signed compare.
    Compare2,
    /// `(a, nb) -> i64` — reductions.
    Reduce,
    /// `(offset, payload, width_class) -> ()` — write-log push.
    WriteLogPushStatic,
    /// `(buf, offset, payload, width_class) -> ()` — write-log grow+push (narrow).
    WriteLogGrowPushNarrow,
    /// `(buf, offset, src_ptr, nb) -> ()` — write-log grow+push (wide).
    WriteLogGrowPushWide,
}

pub struct Context {
    pub use_4state: bool,
    pub ff_values: Value,
    pub comb_values: Value,
    /// Per-Ir `WriteLogBuffer` pointer (3rd JIT arg).
    pub log_buf: Value,
    /// FF byte delta (4th JIT arg, I32) added to baked write-log offsets so a
    /// relocated (cache-reused) chunk records absolute `ff_values` offsets.
    pub ff_delta: Value,
    pub zero: Value,
    pub zero_128: Value,
    /// Load CSE cache: VarOffset → (payload, mask_xz).
    pub load_cache: HashMap<VarOffset, (Value, Option<Value>)>,
    /// Offsets where stores can be skipped (forwarded via load_cache only).
    pub store_elim_offsets: HashSet<VarOffset>,
    /// Disabled inside If blocks.
    pub store_elim_enabled: bool,
    pub helper_sigs: HashMap<HelperSig, SigRef>,
    pub call_conv: CallConv,
    /// Force a fresh load on every access.  Set when the chunk contains
    /// CompiledBlock helpers that may mutate values between loads.
    pub disable_load_cache: bool,
    /// Pre-computed future read positions for Belady-optimal cache
    /// eviction.  `None` when eviction is disabled.
    pub future_reads: Option<FutureReads>,
    /// Eviction trigger; cache size ≤ capacity → no evict.  Defaults
    /// to ~physical GPR budget after ABI-reserved regs.
    pub lookahead_capacity: usize,
}

pub fn get_or_create_sig(
    context: &mut Context,
    builder: &mut FunctionBuilder,
    kind: HelperSig,
) -> SigRef {
    if let Some(&sig) = context.helper_sigs.get(&kind) {
        return sig;
    }

    let mut sig = Signature::new(context.call_conv);
    match kind {
        HelperSig::BinaryOp => {
            sig.params.push(AbiParam::new(I64)); // dst
            sig.params.push(AbiParam::new(I64)); // a
            sig.params.push(AbiParam::new(I64)); // b / amount
            sig.params.push(AbiParam::new(I32)); // nb
        }
        HelperSig::UnaryOp => {
            sig.params.push(AbiParam::new(I64)); // dst
            sig.params.push(AbiParam::new(I64)); // a
            sig.params.push(AbiParam::new(I32)); // nb
        }
        HelperSig::Compare => {
            sig.params.push(AbiParam::new(I64)); // a
            sig.params.push(AbiParam::new(I64)); // b
            sig.params.push(AbiParam::new(I32)); // nb
            sig.returns.push(AbiParam::new(I64));
        }
        HelperSig::Compare2 => {
            sig.params.push(AbiParam::new(I64)); // a
            sig.params.push(AbiParam::new(I64)); // b
            sig.params.push(AbiParam::new(I32)); // a_packed
            sig.params.push(AbiParam::new(I32)); // b_packed
            sig.returns.push(AbiParam::new(I64));
        }
        HelperSig::Reduce => {
            sig.params.push(AbiParam::new(I64)); // a
            sig.params.push(AbiParam::new(I32)); // nb
            sig.returns.push(AbiParam::new(I64));
        }
        HelperSig::WriteLogPushStatic => {
            sig.params.push(AbiParam::new(I32)); // offset
            sig.params.push(AbiParam::new(I64)); // payload
            sig.params.push(AbiParam::new(I32)); // width_class (caller-zext from u16)
        }
        HelperSig::WriteLogGrowPushNarrow => {
            sig.params.push(AbiParam::new(I64)); // buf
            sig.params.push(AbiParam::new(I32)); // offset
            sig.params.push(AbiParam::new(I64)); // payload
            sig.params.push(AbiParam::new(I32)); // width_class
        }
        HelperSig::WriteLogGrowPushWide => {
            sig.params.push(AbiParam::new(I64)); // buf
            sig.params.push(AbiParam::new(I32)); // offset
            sig.params.push(AbiParam::new(I64)); // src_ptr
            sig.params.push(AbiParam::new(I32)); // nb
        }
    }

    let sig_ref = builder.import_signature(sig);
    context.helper_sigs.insert(kind, sig_ref);
    sig_ref
}

pub fn call_helper_void(
    context: &mut Context,
    builder: &mut FunctionBuilder,
    kind: HelperSig,
    func_addr: usize,
    args: &[Value],
) {
    let sig_ref = get_or_create_sig(context, builder, kind);
    let ptr = builder.ins().iconst(I64, func_addr as i64);
    builder.ins().call_indirect(sig_ref, ptr, args);
}

pub fn call_helper_ret(
    context: &mut Context,
    builder: &mut FunctionBuilder,
    kind: HelperSig,
    func_addr: usize,
    args: &[Value],
) -> Value {
    let sig_ref = get_or_create_sig(context, builder, kind);
    let ptr = builder.ins().iconst(I64, func_addr as i64);
    let call = builder.ins().call_indirect(sig_ref, ptr, args);
    builder.inst_results(call)[0]
}

/// Inline write-log push: direct loads/stores into the `WriteLogBuffer`
/// at `context.log_buf` (mirrors `write_log::WriteLogBuffer` layout).
/// ~8 instructions vs ~30+ via helper call.  A full pool branches to the
/// out-of-line grow+push helper (fn pointer in the buffer header).
pub fn emit_inline_write_log_push(
    context: &mut Context,
    builder: &mut FunctionBuilder,
    offset: Value,
    payload: Value,
    width_class: Value,
) {
    use crate::ir::write_log::{
        WRITE_LOG_ENTRY_OFFSET_MASK_XZ, WRITE_LOG_ENTRY_OFFSET_OFFSET,
        WRITE_LOG_ENTRY_OFFSET_PAYLOAD, WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS, WRITE_LOG_ENTRY_SIZE,
        WRITE_LOG_NARROW_OFFSET_CAPACITY, WRITE_LOG_NARROW_OFFSET_COUNT,
        WRITE_LOG_NARROW_OFFSET_ENTRIES_PTR, WRITE_LOG_OFFSET_GROW_PUSH_NARROW,
    };
    let log_buf = context.log_buf;
    let flags = MemFlagsData::trusted();

    // Relocate the baked offset to an absolute ff_values offset: a cache-reused
    // chunk runs at a different base, so add the runtime ff_delta (0 otherwise).
    let offset = builder.ins().iadd(offset, context.ff_delta);

    let count = builder
        .ins()
        .load(I32, flags, log_buf, WRITE_LOG_NARROW_OFFSET_COUNT);
    let capacity = builder
        .ins()
        .load(I32, flags, log_buf, WRITE_LOG_NARROW_OFFSET_CAPACITY);
    let full = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, count, capacity);

    let slow_block = builder.create_block();
    let fast_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.set_cold_block(slow_block);
    builder.ins().brif(full, slow_block, &[], fast_block, &[]);

    builder.switch_to_block(slow_block);
    let grow_push = builder
        .ins()
        .load(I64, flags, log_buf, WRITE_LOG_OFFSET_GROW_PUSH_NARROW);
    let sig_ref = get_or_create_sig(context, builder, HelperSig::WriteLogGrowPushNarrow);
    builder
        .ins()
        .call_indirect(sig_ref, grow_push, &[log_buf, offset, payload, width_class]);
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(fast_block);
    let one = builder.ins().iconst(I32, 1);
    let new_count = builder.ins().iadd(count, one);
    builder
        .ins()
        .store(flags, new_count, log_buf, WRITE_LOG_NARROW_OFFSET_COUNT);
    let entries_ptr = builder
        .ins()
        .load(I64, flags, log_buf, WRITE_LOG_NARROW_OFFSET_ENTRIES_PTR);
    // entry_slot = entries_ptr + count * ENTRY_SIZE
    let count_i64 = builder.ins().uextend(I64, count);
    let entry_size = builder.ins().iconst(I64, WRITE_LOG_ENTRY_SIZE as i64);
    let byte_offset = builder.ins().imul(count_i64, entry_size);
    let entry_slot = builder.ins().iadd(entries_ptr, byte_offset);
    // entry.offset (i32 @ +0) = offset
    builder
        .ins()
        .store(flags, offset, entry_slot, WRITE_LOG_ENTRY_OFFSET_OFFSET);
    // entry.mask_xz (i16 @ +4) = 0  — istore16 of zero
    let zero_i16 = builder.ins().iconst(I32, 0);
    builder
        .ins()
        .istore16(flags, zero_i16, entry_slot, WRITE_LOG_ENTRY_OFFSET_MASK_XZ);
    // entry.width_class (i16 @ +6) = width_class (i32, low 16 bits)
    builder.ins().istore16(
        flags,
        width_class,
        entry_slot,
        WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS,
    );
    // entry.payload (i64 @ +8) = payload
    builder
        .ins()
        .store(flags, payload, entry_slot, WRITE_LOG_ENTRY_OFFSET_PAYLOAD);
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(merge_block);
}

/// Inline write-log push for a wide FF: writes a single `WriteLogWideEntry`
/// with offset, byte count, and `nb` bytes copied from `payload_ptr`.
/// Requires `nb <= WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES` (= 56).  A full pool
/// branches to the out-of-line grow+push helper, like the narrow push.
pub fn emit_inline_write_log_push_wide(
    context: &mut Context,
    builder: &mut FunctionBuilder,
    offset: Value,
    payload_ptr: Value,
    nb: usize,
) {
    use crate::ir::write_log::{
        WRITE_LOG_OFFSET_GROW_PUSH_WIDE, WRITE_LOG_WIDE_ENTRY_OFFSET_NB,
        WRITE_LOG_WIDE_ENTRY_OFFSET_OFFSET, WRITE_LOG_WIDE_ENTRY_OFFSET_PAYLOAD,
        WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES, WRITE_LOG_WIDE_ENTRY_SIZE,
        WRITE_LOG_WIDE_OFFSET_CAPACITY, WRITE_LOG_WIDE_OFFSET_COUNT,
        WRITE_LOG_WIDE_OFFSET_ENTRIES_PTR,
    };
    debug_assert!(nb > 0 && nb <= WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES);
    let log_buf = context.log_buf;
    let flags = MemFlagsData::trusted();

    // Relocate the baked offset to an absolute ff_values offset (see narrow push).
    let offset = builder.ins().iadd(offset, context.ff_delta);

    let count = builder
        .ins()
        .load(I32, flags, log_buf, WRITE_LOG_WIDE_OFFSET_COUNT);
    let capacity = builder
        .ins()
        .load(I32, flags, log_buf, WRITE_LOG_WIDE_OFFSET_CAPACITY);
    let full = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, count, capacity);

    let slow_block = builder.create_block();
    let fast_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.set_cold_block(slow_block);
    builder.ins().brif(full, slow_block, &[], fast_block, &[]);

    builder.switch_to_block(slow_block);
    let grow_push = builder
        .ins()
        .load(I64, flags, log_buf, WRITE_LOG_OFFSET_GROW_PUSH_WIDE);
    let nb_arg = builder.ins().iconst(I32, nb as i64);
    let sig_ref = get_or_create_sig(context, builder, HelperSig::WriteLogGrowPushWide);
    builder
        .ins()
        .call_indirect(sig_ref, grow_push, &[log_buf, offset, payload_ptr, nb_arg]);
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(fast_block);
    let one = builder.ins().iconst(I32, 1);
    let new_count = builder.ins().iadd(count, one);
    builder
        .ins()
        .store(flags, new_count, log_buf, WRITE_LOG_WIDE_OFFSET_COUNT);
    let entries_ptr = builder
        .ins()
        .load(I64, flags, log_buf, WRITE_LOG_WIDE_OFFSET_ENTRIES_PTR);
    let count_i64 = builder.ins().uextend(I64, count);
    let entry_size = builder.ins().iconst(I64, WRITE_LOG_WIDE_ENTRY_SIZE as i64);
    let byte_offset = builder.ins().imul(count_i64, entry_size);
    let entry_slot = builder.ins().iadd(entries_ptr, byte_offset);

    builder.ins().store(
        flags,
        offset,
        entry_slot,
        WRITE_LOG_WIDE_ENTRY_OFFSET_OFFSET,
    );
    let nb_val = builder.ins().iconst(I32, nb as i64);
    builder
        .ins()
        .istore8(flags, nb_val, entry_slot, WRITE_LOG_WIDE_ENTRY_OFFSET_NB);

    let payload_dst_base = WRITE_LOG_WIDE_ENTRY_OFFSET_PAYLOAD;
    let mut i: usize = 0;
    while i + 8 <= nb {
        let v = builder.ins().load(I64, flags, payload_ptr, i as i32);
        builder
            .ins()
            .store(flags, v, entry_slot, payload_dst_base + i as i32);
        i += 8;
    }
    while i < nb {
        let rem = nb - i;
        let chunk = if rem >= 4 {
            4
        } else if rem >= 2 {
            2
        } else {
            1
        };
        let v = builder.ins().load(I32, flags, payload_ptr, i as i32);
        match chunk {
            4 => builder
                .ins()
                .store(flags, v, entry_slot, payload_dst_base + i as i32),
            2 => builder
                .ins()
                .istore16(flags, v, entry_slot, payload_dst_base + i as i32),
            _ => builder
                .ins()
                .istore8(flags, v, entry_slot, payload_dst_base + i as i32),
        };
        i += chunk;
    }
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(merge_block);
}

/// Stack slot of `nb` bytes; returns its address as I64.
pub fn alloc_wide_slot(builder: &mut FunctionBuilder, nb: usize) -> Value {
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift::codegen::ir::StackSlotKind::ExplicitSlot,
        u32::try_from(nb).expect("alloc_wide_slot: nb exceeds u32::MAX"),
        8,
    ));
    builder.ins().stack_addr(I64, slot, 0)
}

/// Compile a chunk into a native function.  Returns `(func, mmap)`;
/// the caller must keep `mmap` alive for as long as `func` is callable
/// (typically by wrapping both in `ChunkArtifact`).
pub fn build_binary(
    config: &Config,
    proto: Vec<ProtoStatement>,
) -> Option<(FuncPtr, Option<memmap2::Mmap>)> {
    build_binary_inner(config, proto, HashSet::default(), false)
}

/// [`build_binary`] with load_cache disabled, for chunks containing
/// CompiledBlock helpers that may mutate comb values between loads.
pub fn build_binary_no_cache(
    config: &Config,
    proto: Vec<ProtoStatement>,
) -> Option<(FuncPtr, Option<memmap2::Mmap>)> {
    build_binary_inner(config, proto, HashSet::default(), true)
}

fn proto_contains_compiled_block(stmts: &[ProtoStatement]) -> bool {
    stmts.iter().any(|s| match s {
        ProtoStatement::CompiledBlock(_) => true,
        ProtoStatement::SequentialBlock(body) => proto_contains_compiled_block(body),
        ProtoStatement::If(if_stmt) => {
            proto_contains_compiled_block(&if_stmt.true_side)
                || proto_contains_compiled_block(&if_stmt.false_side)
        }
        ProtoStatement::Case(case_stmt) => {
            case_stmt
                .arms
                .iter()
                .any(|arm| proto_contains_compiled_block(&arm.body))
                || proto_contains_compiled_block(&case_stmt.default)
        }
        ProtoStatement::For(for_stmt) => proto_contains_compiled_block(&for_stmt.body),
        _ => false,
    })
}

/// (statement count incl. nested, max destination bit-width) for a chunk, used to
/// label its `perf` map entry so hot wide chunks stand out by shape.
fn chunk_shape(stmts: &[ProtoStatement]) -> (usize, usize) {
    let mut count = 0;
    let mut max_w = 0;
    for s in stmts {
        count += 1;
        match s {
            ProtoStatement::Assign(a) => max_w = max_w.max(a.dst_width),
            ProtoStatement::AssignDynamic(a) => max_w = max_w.max(a.dst_width),
            ProtoStatement::If(x) => {
                let (tc, tw) = chunk_shape(&x.true_side);
                let (fc, fw) = chunk_shape(&x.false_side);
                count += tc + fc;
                max_w = max_w.max(tw).max(fw);
            }
            ProtoStatement::For(x) => {
                let (bc, bw) = chunk_shape(&x.body);
                count += bc;
                max_w = max_w.max(bw);
            }
            ProtoStatement::SequentialBlock(b) => {
                let (bc, bw) = chunk_shape(b);
                count += bc;
                max_w = max_w.max(bw);
            }
            _ => {}
        }
    }
    (count, max_w)
}

/// Append a `/tmp/perf-<pid>.map` entry so `perf` can symbolize this JIT'd chunk
/// (which otherwise samples as an anonymous-mmap address). Gated by
/// `VERYL_JIT_PERFMAP=1`; the name encodes the chunk's shape so the report groups
/// hot chunks by statement count and max width. Best-effort — a failed diagnostic
/// write never aborts a compile.
fn emit_jit_perfmap(addr: *const u8, size: usize, proto: &[ProtoStatement]) {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var("VERYL_JIT_PERFMAP").as_deref() == Ok("1")) {
        return;
    }
    let (count, max_w) = chunk_shape(proto);
    let line = format!(
        "{:x} {:x} veryl_comb_{count}s_w{max_w}\n",
        addr as usize, size
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("/tmp/perf-{}.map", std::process::id()))
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

fn build_binary_inner(
    config: &Config,
    proto: Vec<ProtoStatement>,
    store_elim: HashSet<VarOffset>,
    disable_load_cache: bool,
) -> Option<(FuncPtr, Option<memmap2::Mmap>)> {
    let mut settings_builder = settings::builder();
    settings_builder.set("opt_level", "speed").unwrap();
    if !config.dump_cranelift {
        settings_builder.set("enable_verifier", "false").unwrap();
    }
    let chunk_has_compiled_block = proto_contains_compiled_block(&proto);
    let force_disable_load_cache = std::env::var("VERYL_FORCE_DISABLE_LOAD_CACHE")
        .ok()
        .as_deref()
        == Some("1");
    // IR load-CSE: honor the caller's no-cache request unconditionally. A
    // sub-group split from a settle that embeds a CompiledBlock is flagged
    // disable_load_cache, and CSE across the (separately-compiled) CompiledBlock
    // boundary can forward a pre-CompiledBlock (uninitialized) value.
    let effective_disable_load_cache = disable_load_cache || force_disable_load_cache;
    // Cranelift alias analysis is only unsafe when this chunk embeds a
    // CompiledBlock (an opaque callee that may mutate comb storage between our
    // memory ops).  Fully-JIT chunks keep it on for speed.
    if (effective_disable_load_cache && chunk_has_compiled_block) || force_disable_load_cache {
        settings_builder
            .set("enable_alias_analysis", "false")
            .unwrap();
    }
    let flags = settings::Flags::new(settings_builder);

    let isa = match isa::lookup(Triple::host()) {
        Err(err) => panic!("Error looking up target: {}", err),
        Ok(isa_builder) => isa_builder.finish(flags).unwrap(),
    };

    let ptr_type = isa.pointer_type();
    let call_conv = CallConv::triple_default(&Triple::host());

    let mut sig = Signature::new(call_conv);
    sig.params.push(AbiParam::new(ptr_type)); // *const u8 (ff base)
    sig.params.push(AbiParam::new(ptr_type)); // *const u8 (comb base)
    sig.params.push(AbiParam::new(ptr_type)); // *mut u8 (write_log buffer)
    sig.params.push(AbiParam::new(ptr_type)); // isize (ff_delta for write-log)

    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut func_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut func_ctx);

    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);

    let ff_values = builder.block_params(block)[0];
    let comb_values = builder.block_params(block)[1];
    let log_buf = builder.block_params(block)[2];
    // 4th arg is the FF byte delta; reduce to I32 once (write-log offsets are u32).
    let ff_delta_raw = builder.block_params(block)[3];
    let ff_delta = builder.ins().ireduce(I32, ff_delta_raw);
    let zero = builder.ins().iconst(I64, 0);
    let zero_lo = builder.ins().iconst(I64, 0);
    let zero_hi = builder.ins().iconst(I64, 0);
    let zero_128 = builder.ins().iconcat(zero_lo, zero_hi);

    let mut cranelift_context = Context {
        use_4state: config.use_4state,
        ff_values,
        comb_values,
        log_buf,
        ff_delta,
        zero,
        zero_128,
        load_cache: HashMap::default(),
        store_elim_offsets: store_elim,
        store_elim_enabled: true,
        helper_sigs: HashMap::default(),
        call_conv,
        disable_load_cache: effective_disable_load_cache,
        future_reads: None,
        lookahead_capacity: 0,
    };

    // Belady-optimal load_cache eviction (opt-out via VERYL_STAGE7_LOOKAHEAD=0).
    // Caps resident entries to bound SSA live ranges and prevent regalloc
    // spill cascade.  See ir/load_cache_lookahead.rs.
    if !effective_disable_load_cache
        && std::env::var("VERYL_STAGE7_LOOKAHEAD").as_deref() != Ok("0")
    {
        cranelift_context.future_reads = Some(compute_read_positions(&proto));
        cranelift_context.lookahead_capacity = std::env::var("VERYL_STAGE7_LOOKAHEAD_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(12);
    }

    let len = proto.len();
    for (i, x) in proto.iter().enumerate() {
        let is_last = (i + 1) == len;
        x.build_binary(&mut cranelift_context, &mut builder, is_last)?;

        // Belady: while over capacity, evict the entry whose next read
        // is farthest in the future (None = never read again).
        if let Some(future) = cranelift_context.future_reads.take() {
            let cap = cranelift_context.lookahead_capacity;
            while cranelift_context.load_cache.len() > cap {
                let evict = cranelift_context
                    .load_cache
                    .keys()
                    .max_by_key(|off| match future.get(off) {
                        None => usize::MAX,
                        Some(positions) => match positions.iter().copied().find(|&p| p > i) {
                            None => usize::MAX,
                            Some(next) => next - i,
                        },
                    })
                    .copied();
                if let Some(off) = evict {
                    cranelift_context.load_cache.remove(&off);
                } else {
                    break;
                }
            }
            cranelift_context.future_reads = Some(future);
        }
    }

    builder.ins().return_(&[]);
    builder.seal_all_blocks();
    builder.finalize(isa.frontend_config());

    if config.dump_cranelift {
        println!("Cranelift IR");
        println!("{}", indent_all_by(2, func.display().to_string()));
    }

    let mut ctx = codegen::Context::for_function(func);
    if config.dump_asm {
        ctx.set_disasm(true);
    }

    let mut control_plane = ControlPlane::default();
    let code = match ctx.compile(&*isa, &mut control_plane) {
        Ok(code) => code,
        Err(err) => {
            log::warn!("JIT compilation failed, falling back to interpreter: {err:?}");
            return None;
        }
    };

    if config.dump_asm
        && let Some(disasm) = &code.vcode
    {
        println!("Assembly of {}", isa.name());
        println!("{}", indent_all_by(2, disasm.to_string()));
    }

    // Chunk code is packed tightly, in compile (≈ schedule) order, into a
    // shared executable arena: one page-per-chunk mmap scatters thousands
    // of small chunks across the address space, and the settle loop —
    // whose per-run cost is dominated by L1-icache misses — then gets no
    // spatial locality between consecutively dispatched chunks.  Falls
    // back to a private mapping if the arena is unavailable (e.g. W^X
    // policy).
    let (code_ptr, keepalive) = match code_arena::alloc(code.code_buffer()) {
        Some(p) => (p as *const u8, None),
        None => {
            let mut buffer = memmap2::MmapOptions::new()
                .len(code.code_buffer().len())
                .map_anon()
                .unwrap();
            buffer.copy_from_slice(code.code_buffer());
            let buffer = buffer.make_exec().unwrap();
            let p = buffer.as_ptr();
            (p, Some(buffer))
        }
    };

    let func_ptr: FuncPtr = unsafe { std::mem::transmute(code_ptr) };

    emit_jit_perfmap(code_ptr, code.code_buffer().len(), &proto);

    Some((func_ptr, keepalive))
}

/// Shared executable code arena (see the comment at the allocation site).
/// Blocks are never unmapped: artifacts cached across tests may outlive
/// any single simulator, and the whole arena stays far smaller than the
/// page-per-chunk scheme it replaces.
mod code_arena {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    mod imp {
        use std::sync::Mutex;

        struct Arena {
            cur: *mut u8,
            remaining: usize,
        }
        // SAFETY: the raw pointer is only touched under the mutex.
        unsafe impl Send for Arena {}

        static ARENA: Mutex<Arena> = Mutex::new(Arena {
            cur: std::ptr::null_mut(),
            remaining: 0,
        });

        const BLOCK: usize = 8 << 20;

        pub fn alloc(code: &[u8]) -> Option<*mut u8> {
            let need = code.len().checked_add(15)? & !15;
            let mut a = ARENA.lock().unwrap();
            if a.remaining < need {
                let size = BLOCK.max(need.next_multiple_of(4096));
                // SAFETY: fresh anonymous mapping, error-checked below.
                let p = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        size,
                        libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
                        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                        -1,
                        0,
                    )
                };
                if p == libc::MAP_FAILED {
                    return None;
                }
                // Huge pages keep the call-heavy settle loop off iTLB
                // walks; best-effort.
                unsafe {
                    libc::madvise(p, size, libc::MADV_HUGEPAGE);
                }
                a.cur = p as *mut u8;
                a.remaining = size;
            }
            let dst = a.cur;
            // SAFETY: dst..dst+need is inside the current block (checked
            // above) and nothing has been published there yet; x86 keeps
            // icache coherent with these stores once the pointer is
            // published through the artifact.
            unsafe {
                std::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len());
                a.cur = dst.add(need);
            }
            a.remaining -= need;
            Some(dst)
        }
    }

    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    mod imp {
        /// The arena is Linux/x86_64-only: it relies on `MADV_HUGEPAGE`
        /// (Linux-specific) and a persistent RWX mapping (macOS enforces
        /// W^X, non-x86 needs an icache flush). Everything else keeps the
        /// per-chunk mapping path.
        pub fn alloc(_code: &[u8]) -> Option<*mut u8> {
            None
        }
    }

    pub use imp::alloc;
}
