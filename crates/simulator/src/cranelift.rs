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

pub type FuncPtr = unsafe extern "system" fn(*const u8, *const u8);

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
    build_binary_inner(context, proto, HashSet::default(), true)
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
        helper_sigs: HashMap::default(),
        call_conv,
        disable_load_cache,
    };

    let len = proto.len();
    for (i, x) in proto.iter().enumerate() {
        let is_last = (i + 1) == len;
        x.build_binary(&mut cranelift_context, &mut builder, is_last)?;
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
    let code = ctx.compile(&*isa, &mut control_plane).unwrap();

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
