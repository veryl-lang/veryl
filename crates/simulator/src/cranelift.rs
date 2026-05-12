use crate::ir::Context as ConvContext;
use crate::ir::ProtoStatement;
use crate::ir::VarOffset;
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

/// Signature kinds for helper function calls via call_indirect.
#[derive(Hash, Eq, PartialEq, Clone, Copy)]
pub enum HelperSig {
    /// (I64, I64, I64, I32) -> void  [binary ops, shifts: dst, a, b/amount, nb]
    BinaryOp,
    /// (I64, I64, I32) -> void  [unary ops, copy: dst, a, nb]
    UnaryOp,
    /// (I64, I64, I32) -> I64  [comparisons: a, b, nb -> result]
    Compare,
    /// (I64, I32) -> I64  [reductions: a, nb -> result]
    Reduce,
    /// (I32, I64, I32) -> void  [write-log push: offset, payload, width_class]
    /// Phase 2d-prep of ff_commit redesign — signature reserved for the
    /// upcoming JIT-emitted FF write log push call.  Not yet emitted.
    WriteLogPushStatic,
}

pub struct Context {
    pub use_4state: bool,
    pub ff_values: Value,
    pub comb_values: Value,
    /// Pointer to the per-Ir `WriteLogBuffer` (3rd JIT arg).  Used by
    /// `emit_inline_write_log_push` to perform inline log entry stores.
    pub log_buf: Value,
    pub zero: Value,
    pub zero_128: Value,
    /// Load CSE cache: VarOffset → (payload, mask_xz)
    pub load_cache: HashMap<VarOffset, (Value, Option<Value>)>,
    /// Comb offsets where stores can be skipped (value forwarded via load_cache only).
    pub store_elim_offsets: HashSet<VarOffset>,
    /// Whether store elimination is active (disabled inside If blocks).
    pub store_elim_enabled: bool,
    /// Helper function signatures (cached per arity/return type).
    pub helper_sigs: HashMap<HelperSig, SigRef>,
    /// Calling convention for helper functions.
    pub call_conv: CallConv,
    /// Disable load_cache: every access emits a fresh load instruction.
    /// Used for unified comb where helper functions may modify values
    /// between cached loads.
    pub disable_load_cache: bool,
    /// Stage 7 Phase B look-ahead load_cache eviction (env-gated).
    /// Pre-computed per-VarOffset future read positions across the
    /// chunk's top-level statement sequence.  After each stmt, the
    /// main loop evicts cached entries by Belady-optimal policy
    /// (farthest next-read first) when size exceeds capacity.
    /// `None` when VERYL_STAGE7_LOOKAHEAD is unset.
    pub future_reads: Option<crate::ir::load_cache_lookahead::FutureReads>,
    /// Belady eviction trigger.  Cache size <= capacity → no evict.
    /// Default ~physical GPR budget after ABI-reserved regs.
    pub lookahead_capacity: usize,
}

/// Get or create a SigRef for the given helper signature kind.
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
    }

    let sig_ref = builder.import_signature(sig);
    context.helper_sigs.insert(kind, sig_ref);
    sig_ref
}

/// Call a helper function that returns void.
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

/// Call a helper function that returns an I64 value.
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

/// Phase 1.6: emit an inline write-log push.  Replaces
/// `call_helper_void(WriteLogPushStatic, ...)` with direct loads/stores to
/// the `WriteLogBuffer` whose pointer is passed as the 3rd JIT arg
/// (`context.log_buf`).
///
/// Layout (mirrors `write_log::WriteLogBuffer`):
///   offset 0 : entries_ptr (*mut WriteLogEntry)  — `i64` load
///   offset 8 : count       (u32)                 — `i32` load + store
/// Entry layout (16 B):
///   offset 0 : offset      (u32)
///   offset 4 : mask_xz     (u16) — store 0
///   offset 6 : width_class (u16) — caller passes I32, truncate to I16
///   offset 8 : payload     (u64)
///
/// Asm sequence: load count, increment, store count (back), load entries_ptr,
/// compute entry slot = ptr + count*16, store offset/mask_xz/width_class/
/// payload.  ~8 instructions vs ~30+ via helper call.
pub fn emit_inline_write_log_push(
    context: &Context,
    builder: &mut FunctionBuilder,
    offset: Value,
    payload: Value,
    width_class: Value,
) {
    use crate::ir::write_log::{
        WRITE_LOG_ENTRY_OFFSET_MASK_XZ, WRITE_LOG_ENTRY_OFFSET_OFFSET,
        WRITE_LOG_ENTRY_OFFSET_PAYLOAD, WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS, WRITE_LOG_ENTRY_SIZE,
        WRITE_LOG_OFFSET_COUNT, WRITE_LOG_OFFSET_ENTRIES_PTR,
    };
    let log_buf = context.log_buf;
    let flags = MemFlags::trusted();

    // count = *(log_buf + WRITE_LOG_OFFSET_COUNT) as u32
    let count = builder
        .ins()
        .load(I32, flags, log_buf, WRITE_LOG_OFFSET_COUNT);
    // new_count = count + 1
    let one = builder.ins().iconst(I32, 1);
    let new_count = builder.ins().iadd(count, one);
    // *(log_buf + WRITE_LOG_OFFSET_COUNT) = new_count
    builder
        .ins()
        .store(flags, new_count, log_buf, WRITE_LOG_OFFSET_COUNT);
    // entries_ptr = *(log_buf + WRITE_LOG_OFFSET_ENTRIES_PTR) as *mut Entry
    let entries_ptr = builder
        .ins()
        .load(I64, flags, log_buf, WRITE_LOG_OFFSET_ENTRIES_PTR);
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
}

/// Allocate a stack slot of `nb` bytes and return its address as an I64 value.
pub fn alloc_wide_slot(builder: &mut FunctionBuilder, nb: usize) -> Value {
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift::codegen::ir::StackSlotKind::ExplicitSlot,
        u32::try_from(nb).expect("alloc_wide_slot: nb exceeds u32::MAX"),
        8,
    ));
    builder.ins().stack_addr(I64, slot, 0)
}

pub fn build_binary(context: &mut ConvContext, proto: Vec<ProtoStatement>) -> Option<FuncPtr> {
    build_binary_inner(context, proto, HashSet::default(), false)
}

/// Build a JIT function with load_cache disabled.
/// Used for unified comb where helper functions (CompiledBlocks) may
/// modify comb values between cached loads within the same JIT function.
pub fn build_binary_no_cache(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
) -> Option<FuncPtr> {
    build_binary_inner(context, proto, HashSet::default(), true)
}

/// Recursively check whether the chunk contains any `CompiledBlock`
/// (interpreted helper) statement.
fn proto_contains_compiled_block(stmts: &[ProtoStatement]) -> bool {
    stmts.iter().any(|s| match s {
        ProtoStatement::CompiledBlock(_) => true,
        ProtoStatement::SequentialBlock(body) => proto_contains_compiled_block(body),
        ProtoStatement::If(if_stmt) => {
            proto_contains_compiled_block(&if_stmt.true_side)
                || proto_contains_compiled_block(&if_stmt.false_side)
        }
        ProtoStatement::For(for_stmt) => proto_contains_compiled_block(&for_stmt.body),
        _ => false,
    })
}

fn build_binary_inner(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
    store_elim: HashSet<VarOffset>,
    disable_load_cache: bool,
) -> Option<FuncPtr> {
    let config = &context.config;

    let mut settings_builder = settings::builder();
    settings_builder.set("opt_level", "speed").unwrap();
    if !config.dump_cranelift {
        settings_builder.set("enable_verifier", "false").unwrap();
    }
    // `disable_load_cache` is only required when the chunk contains an
    // interpreted CompiledBlock — those helpers can mutate comb storage
    // between cached loads, breaking the IR-level load CSE in
    // `expression.rs::build_binary`.  Fully-JIT chunks are safe, so the
    // cache (and Cranelift's alias analysis) can stay on.
    let chunk_has_compiled_block = proto_contains_compiled_block(&proto);
    let force_disable_load_cache = std::env::var("VERYL_FORCE_DISABLE_LOAD_CACHE")
        .ok()
        .as_deref()
        == Some("1");
    let effective_disable_load_cache =
        disable_load_cache && (chunk_has_compiled_block || force_disable_load_cache);
    if effective_disable_load_cache {
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

    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut func_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut func_ctx);

    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);

    let ff_values = builder.block_params(block)[0];
    let comb_values = builder.block_params(block)[1];
    let log_buf = builder.block_params(block)[2];
    let zero = builder.ins().iconst(I64, 0);
    let zero_lo = builder.ins().iconst(I64, 0);
    let zero_hi = builder.ins().iconst(I64, 0);
    let zero_128 = builder.ins().iconcat(zero_lo, zero_hi);

    let mut cranelift_context = Context {
        use_4state: config.use_4state,
        ff_values,
        comb_values,
        log_buf,
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

    // Stage 7 Phase B: pre-compute future-read positions for Belady-
    // optimal load_cache eviction (env-gated `VERYL_STAGE7_LOOKAHEAD`,
    // default-on; opt-out via VERYL_STAGE7_LOOKAHEAD=0).  Cap each
    // chunk's resident cache entry count at `lookahead_capacity` to
    // bound SSA Value live range to ~physical GPR budget and prevent
    // regalloc spill cascade.  See ir/load_cache_lookahead.rs.
    if !effective_disable_load_cache
        && std::env::var("VERYL_STAGE7_LOOKAHEAD").as_deref() != Ok("0")
    {
        cranelift_context.future_reads = Some(
            crate::ir::load_cache_lookahead::compute_read_positions(&proto),
        );
        cranelift_context.lookahead_capacity = std::env::var("VERYL_STAGE7_LOOKAHEAD_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(12);
    }

    let len = proto.len();
    for (i, x) in proto.iter().enumerate() {
        let is_last = (i + 1) == len;
        x.build_binary(&mut cranelift_context, &mut builder, is_last)?;

        // Stage 7 Phase B: Belady-optimal eviction after each top-level
        // stmt.  While cache size > capacity, evict the entry whose
        // next read is farthest in the future (None = never used again,
        // evicted first).
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
    builder.finalize();

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

    let mut buffer = memmap2::MmapOptions::new()
        .len(code.code_buffer().len())
        .map_anon()
        .unwrap();

    buffer.copy_from_slice(code.code_buffer());
    let buffer = buffer.make_exec().unwrap();

    let func_ptr: FuncPtr = unsafe { std::mem::transmute(buffer.as_ptr()) };

    context.binary.push(buffer);

    Some(func_ptr)
}
