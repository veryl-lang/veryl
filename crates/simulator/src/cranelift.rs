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
}

pub struct Context {
    pub use_4state: bool,
    pub ff_values: Value,
    pub comb_values: Value,
    pub zero: Value,
    pub zero_128: Value,
    /// Load CSE cache: (VarOffset, nb) → (payload, mask_xz, is_exact_load)
    /// Keyed on (offset, native_bytes) to avoid mixing I32+uextend and I64 loads.
    /// `is_exact_load` is true when the value was loaded directly from memory
    /// (and thus exactly matches memory content). False when the value was
    /// forwarded from a store (may have masked upper bits when dst_width < nb*8).
    pub load_cache: HashMap<(VarOffset, usize), (Value, Option<Value>, bool)>,
    /// Comb offsets where stores can be skipped (value forwarded via Variable).
    pub store_elim_offsets: HashSet<VarOffset>,
    /// Whether store elimination is active (disabled inside If blocks).
    pub store_elim_enabled: bool,
    /// Cranelift Variables for store_elim offsets, surviving across If blocks.
    /// Key: (VarOffset, nb). Value: (payload_var, mask_xz_var).
    pub store_elim_vars: HashMap<(VarOffset, usize), (Variable, Option<Variable>)>,
    /// Next Variable index for store_elim_vars allocation.
    pub next_var_index: usize,
    /// Helper function signatures (cached per arity/return type).
    pub helper_sigs: HashMap<HelperSig, SigRef>,
    /// Calling convention for helper functions.
    pub call_conv: CallConv,
    /// Disable load_cache: every access emits a fresh load instruction.
    /// Used for unified comb where helper functions may modify values
    /// between cached loads.
    pub disable_load_cache: bool,
    /// If set, emit a dirty-flag store when an event writes to a cold comb array.
    /// Value is the byte offset of the flag within comb_values.
    pub cold_dirty_flag_offset: Option<i64>,
    /// Size of the hot comb region in bytes (for cold write detection).
    pub comb_hot_size: usize,
    /// If set, emit a dirty-flag store when an event writes to any comb variable.
    /// Value is the byte offset of the flag within comb_values.
    pub event_comb_dirty_flag_offset: Option<i64>,
    /// True when compiling an event-phase function. Gates strict-NBA
    /// redirection of comb stores to the write-log.
    pub in_event: bool,
    /// During If arms, tracks which VarOffsets were stored to.
    /// Used by If::build_binary to preserve pre-If cache entries
    /// that weren't modified in either arm.
    /// (stored_offsets, dynamic_assign_occurred)
    pub if_store_tracker: Option<(HashSet<VarOffset>, bool)>,
    /// Enable pre-If cache preservation across If block boundaries.
    pub enable_if_cache_preserve: bool,
    /// Write-log buffer pointer (heap address embedded as immediate).
    /// When Some, event JIT emits log append after each FF store.
    pub write_log_entries_ptr: Option<i64>,
    /// Write-log count pointer (heap address of u64 counter).
    pub write_log_count_ptr: Option<i64>,
    /// Cranelift Variable holding the current log entry count (SSA).
    /// Loaded at function entry, stored at function exit.
    pub log_count_var: Option<Variable>,
    /// Heap address of WriteLogBuffer.ff_values_base (u64).
    /// JIT loads this at function entry to compute ff_delta.
    pub ff_values_base_ptr: Option<i64>,
    /// Cranelift Variable holding ff_delta = ff_values_param - ff_values_base.
    /// Used to adjust write-log offsets for JIT-cached instances.
    pub ff_delta_var: Option<Variable>,
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

/// Allocate a stack slot of `nb` bytes and return its address as an I64 value.
pub fn alloc_wide_slot(builder: &mut FunctionBuilder, nb: usize) -> Value {
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift::codegen::ir::StackSlotKind::ExplicitSlot,
        u32::try_from(nb).expect("alloc_wide_slot: nb exceeds u32::MAX"),
        8,
    ));
    builder.ins().stack_addr(I64, slot, 0)
}

/// Build a JIT function with store elimination for internal comb variables.
/// Offsets in `store_elim` will skip memory stores (values forwarded via load_cache).
pub fn build_binary_with_store_elim(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
    store_elim: HashSet<VarOffset>,
) -> Option<FuncPtr> {
    build_binary_inner(context, proto, store_elim, false)
}

pub fn build_binary(context: &mut ConvContext, proto: Vec<ProtoStatement>) -> Option<FuncPtr> {
    build_binary_inner(context, proto, HashSet::default(), false)
}

/// Build a JIT function for an event-phase statement group.
/// Sets `in_event = true` so that comb stores are redirected to the
/// write-log (strict NBA semantics).
pub fn build_binary_event(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
) -> Option<FuncPtr> {
    let saved = context.in_event;
    context.in_event = true;
    let result = build_binary_inner(context, proto, HashSet::default(), false);
    context.in_event = saved;
    result
}

pub fn build_binary_with_store_elim_and_no_cache(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
    store_elim: HashSet<VarOffset>,
) -> Option<FuncPtr> {
    build_binary_inner(context, proto, store_elim, true)
}

/// Build a JIT function with load_cache disabled.
/// Used for unified comb where helper functions (CompiledBlocks) may
/// modify comb values between cached loads within the same JIT function.
pub fn build_binary_no_cache(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
) -> Option<FuncPtr> {
    let saved = context.event_comb_dirty_flag_offset.take();
    // NOTE: write_log_buffer is NOT suppressed here because this function
    // is also used for large event chunks (try_jit_group). Only comb-specific
    // build functions (build_binary_comb_cached*) suppress write-log.
    let result = build_binary_inner(context, proto, HashSet::default(), true);
    context.event_comb_dirty_flag_offset = saved;
    result
}

/// Build a JIT function with load_cache enabled and alias_analysis enabled.
/// Used for unified comb chunks. Our load_cache provides explicit CSE,
/// and Cranelift's alias analysis adds redundant-load elimination and
/// store-to-load forwarding on top.
pub fn build_binary_comb_cached(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
) -> Option<FuncPtr> {
    // Comb JIT does not set event_comb_dirty flag (only event JIT does).
    // Write-log is kept active: comb chunks may contain FF stores
    // (e.g., child module output port → parent FF connections).
    let saved_ecdf = context.event_comb_dirty_flag_offset.take();
    let result = build_binary_inner2(context, proto, HashSet::default(), false, false);
    context.event_comb_dirty_flag_offset = saved_ecdf;
    result
}

/// Build a JIT function with store_elim and load_cache enabled.
/// Used for unified comb chunks where internal variables can skip stores.
pub fn build_binary_comb_cached_with_store_elim(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
    store_elim: HashSet<VarOffset>,
) -> Option<FuncPtr> {
    let saved_ecdf = context.event_comb_dirty_flag_offset.take();
    let result = build_binary_inner2(context, proto, store_elim, false, false);
    context.event_comb_dirty_flag_offset = saved_ecdf;
    result
}

fn build_binary_inner(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
    store_elim: HashSet<VarOffset>,
    disable_load_cache: bool,
) -> Option<FuncPtr> {
    build_binary_inner2(
        context,
        proto,
        store_elim,
        disable_load_cache,
        disable_load_cache,
    )
}

fn build_binary_inner2(
    context: &mut ConvContext,
    proto: Vec<ProtoStatement>,
    store_elim: HashSet<VarOffset>,
    disable_load_cache: bool,
    disable_alias_analysis: bool,
) -> Option<FuncPtr> {
    let config = &context.config;

    let mut settings_builder = settings::builder();
    settings_builder.set("opt_level", "speed").unwrap();
    if !config.dump_cranelift {
        settings_builder.set("enable_verifier", "false").unwrap();
    }
    if disable_alias_analysis {
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

    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut func_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut func_ctx);

    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);

    let ff_values = builder.block_params(block)[0];
    let comb_values = builder.block_params(block)[1];
    let zero = builder.ins().iconst(I64, 0);
    let zero_lo = builder.ins().iconst(I64, 0);
    let zero_hi = builder.ins().iconst(I64, 0);
    let zero_128 = builder.ins().iconcat(zero_lo, zero_hi);

    let mut cranelift_context = Context {
        use_4state: config.use_4state,
        ff_values,
        comb_values,
        zero,
        zero_128,
        load_cache: HashMap::default(),
        store_elim_offsets: store_elim,
        store_elim_enabled: true,
        store_elim_vars: HashMap::default(),
        next_var_index: 0,
        helper_sigs: HashMap::default(),
        call_conv,
        disable_load_cache,
        cold_dirty_flag_offset: context.cold_dirty_flag_offset,
        comb_hot_size: context.comb_hot_size,
        event_comb_dirty_flag_offset: context.event_comb_dirty_flag_offset,
        in_event: context.in_event && context.write_log_buffer.is_some(),
        if_store_tracker: None,
        // Test: only enable for event path (disable_load_cache=false,
        // disable_alias_analysis=false → regular build_binary)
        // Comb_cached: disable_alias_analysis=false, disable_load_cache=false
        // Regular: disable_alias_analysis=true, disable_load_cache=false
        // So we use disable_alias_analysis to distinguish
        // Enable pre-If cache preservation for paths with load_cache active.
        // Currently limited to FF reads only (comb value preservation has
        // correctness issues that need further investigation).
        enable_if_cache_preserve: !disable_load_cache,
        write_log_entries_ptr: context
            .write_log_buffer
            .as_ref()
            .map(|buf| buf.entries.as_ptr() as i64),
        write_log_count_ptr: context
            .write_log_buffer
            .as_ref()
            .map(|buf| &buf.count as *const u64 as i64),
        log_count_var: None,
        ff_values_base_ptr: context
            .write_log_buffer
            .as_ref()
            .map(|buf| &buf.ff_values_base as *const u64 as i64),
        ff_delta_var: None,
    };

    // For event JIT: set up write-log count Variable (SSA).
    // Load count at function entry from heap address, store back at function exit.
    if let Some(count_ptr) = cranelift_context.write_log_count_ptr {
        let var = builder.declare_var(I64);
        let addr = builder.ins().iconst(I64, count_ptr);
        let count_val = builder.ins().load(I64, MemFlags::trusted(), addr, 0);
        builder.def_var(var, count_val);
        cranelift_context.log_count_var = Some(var);
    }

    // For event JIT: compute ff_delta = ff_values_param - ff_values_base.
    // This allows write-log entries to store absolute offsets even when the
    // JIT function is reused for a cached module instance with ff_delta != 0.
    if let Some(base_ptr) = cranelift_context.ff_values_base_ptr {
        let var = builder.declare_var(I64);
        let base_addr = builder.ins().iconst(I64, base_ptr);
        let base_val = builder.ins().load(I64, MemFlags::trusted(), base_addr, 0);
        let delta = builder.ins().isub(ff_values, base_val);
        builder.def_var(var, delta);
        cranelift_context.ff_delta_var = Some(var);
    }

    let len = proto.len();
    for (i, x) in proto.iter().enumerate() {
        let is_last = (i + 1) == len;
        x.build_binary(&mut cranelift_context, &mut builder, is_last)?;
    }

    // Store write-log count back to heap address at function exit.
    if let (Some(var), Some(count_ptr)) = (
        cranelift_context.log_count_var,
        cranelift_context.write_log_count_ptr,
    ) {
        let addr = builder.ins().iconst(I64, count_ptr);
        let count_val = builder.use_var(var);
        builder.ins().store(MemFlags::trusted(), count_val, addr, 0);
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
