use crate::HashMap;
use crate::conv::Context as ConvContext;
use crate::ir::{self, StatementProto};
use cranelift::codegen::control::ControlPlane;
use cranelift::codegen::ir::types::I8;
use cranelift::codegen::ir::{AbiParam, Function, Signature, UserFuncName};
use cranelift::codegen::isa::{self, CallConv};
use cranelift::codegen::{self, settings};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift::prelude::*;
use target_lexicon::Triple;

pub type FuncPtr = unsafe extern "system" fn(bool, *const *mut usize);

pub struct Context {
    pub reset: Value,
    pub args: Value,
    pub value_map: HashMap<usize, Value>,
}

pub fn build_binary(
    context: &mut ConvContext,
    proto: Vec<StatementProto>,
) -> Option<(FuncPtr, Vec<*mut ir::Value>)> {
    let func_ptr = if let Some(x) = context.func_ptr_map.get(&proto) {
        Some(*x)
    } else if let Some(x) = get_func_ptr(context, &proto) {
        context.func_ptr_map.insert(proto, x);
        Some(x)
    } else {
        None
    };
    if let Some(x) = func_ptr {
        // args should be sorted by index
        let mut args: Vec<_> = context.declaration_values.drain().collect();
        args.sort_by_key(|x| x.1);
        let args = args.into_iter().map(|x| x.0).collect();
        Some((x, args))
    } else {
        None
    }
}

fn get_func_ptr(context: &mut ConvContext, proto: &[StatementProto]) -> Option<FuncPtr> {
    let mut builder = settings::builder();
    builder.set("opt_level", "speed").unwrap();
    let flags = settings::Flags::new(builder);

    let isa = match isa::lookup(Triple::host()) {
        Err(err) => panic!("Error looking up target: {}", err),
        Ok(isa_builder) => isa_builder.finish(flags).unwrap(),
    };

    let ptr_type = isa.pointer_type();
    let call_conv = CallConv::triple_default(&Triple::host());

    let mut sig = Signature::new(call_conv);
    // bool
    sig.params.push(AbiParam::new(I8));
    // &[*mut Value]
    sig.params.push(AbiParam::new(ptr_type));

    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut func_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut func_ctx);

    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);

    let reset = builder.block_params(block)[0];
    let args = builder.block_params(block)[1];

    let mut cratelift_context = Context {
        reset,
        args,
        value_map: HashMap::default(),
    };

    let len = proto.len();
    for (i, x) in proto.iter().enumerate() {
        let is_last = (i + 1) == len;
        x.build_binary(&mut cratelift_context, &mut builder, is_last);
    }

    builder.ins().return_(&[]);
    builder.seal_all_blocks();

    builder.finalize();

    //println!("{}", func.display());

    let mut ctx = codegen::Context::for_function(func);
    //ctx.set_disasm(true);

    let mut control_plane = ControlPlane::default();
    let code = ctx.compile(&*isa, &mut control_plane).unwrap();

    //if let Some(disasm) = &code.vcode {
    //    println!("{disasm}");
    //}

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
