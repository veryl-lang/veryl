use crate::ir::Context as ConvContext;
use crate::ir::{CombValue, FfValue, ProtoStatement};
use cranelift::codegen::control::ControlPlane;
use cranelift::codegen::ir::{AbiParam, Function, Signature, UserFuncName};
use cranelift::codegen::isa::{self, CallConv};
use cranelift::codegen::{self, settings};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift::prelude::types::I64;
use cranelift::prelude::*;
use indent::indent_all_by;
use target_lexicon::Triple;

pub type FuncPtr = unsafe extern "system" fn(*const FfValue, *const CombValue);

pub struct Context {
    pub use_4state: bool,
    pub ff_values: Value,
    pub comb_values: Value,
    pub zero: Value,
}

pub fn build_binary(context: &mut ConvContext, proto: Vec<ProtoStatement>) -> Option<FuncPtr> {
    let config = &context.config;

    let mut settings_builder = settings::builder();
    settings_builder.set("opt_level", "speed").unwrap();
    let flags = settings::Flags::new(settings_builder);

    let isa = match isa::lookup(Triple::host()) {
        Err(err) => panic!("Error looking up target: {}", err),
        Ok(isa_builder) => isa_builder.finish(flags).unwrap(),
    };

    let ptr_type = isa.pointer_type();
    let call_conv = CallConv::triple_default(&Triple::host());

    let mut sig = Signature::new(call_conv);
    sig.params.push(AbiParam::new(ptr_type)); // *const FfValue
    sig.params.push(AbiParam::new(ptr_type)); // *const CombValue

    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut func_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut func_ctx);

    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);

    let ff_values = builder.block_params(block)[0];
    let comb_values = builder.block_params(block)[1];
    let zero = builder.ins().iconst(I64, 0);

    let mut cranelift_context = Context {
        use_4state: config.use_4state,
        ff_values,
        comb_values,
        zero,
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
