use crate::HashSet;
use crate::cranelift::Context as CraneliftContext;
use crate::cranelift::FuncPtr;
use crate::ir::context::{Context as ConvContext, Conv};
use crate::ir::expression::build_linear_index_expr;
use crate::ir::expression::{
    ExpressionContext, band_const, gen_mask_for_width, gen_mask_range_128, iconst_128,
};
use crate::ir::variable::{
    native_bytes as calc_native_bytes, read_native_value, write_native_value,
};
use crate::ir::{Expression, ProtoExpression, Value};
use cranelift::prelude::types::{I32, I64, I128};
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::conv::utils::eval_array_literal;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::FunctionCall;
use veryl_analyzer::ir::{SystemFunctionInput, SystemFunctionKind, TypeKind, ValueVariant};
use veryl_analyzer::value::{MaskCache, ValueU64};
use veryl_parser::resource_table::StrId;

/// A single block within a ProtoStatements list: either interpreted or JIT-compiled.
pub enum ProtoStatementBlock {
    Interpreted(Vec<ProtoStatement>),
    Compiled(FuncPtr),
}

/// A sequence of statement blocks that may mix interpreted and JIT-compiled groups.
pub struct ProtoStatements(pub Vec<ProtoStatementBlock>);

impl ProtoStatements {
    pub(crate) fn to_statements(
        &self,
        ff_ptr: *mut u8,
        comb_ptr: *mut u8,
        use_4state: bool,
    ) -> Vec<Statement> {
        let mut result = Vec::new();
        for block in &self.0 {
            match block {
                ProtoStatementBlock::Interpreted(proto) => {
                    for s in proto {
                        result.push(unsafe { s.apply_values_ptr(ff_ptr, comb_ptr, use_4state) });
                    }
                }
                ProtoStatementBlock::Compiled(func) => {
                    result.push(Statement::Binary(
                        *func,
                        ff_ptr as *const u8,
                        comb_ptr as *const u8,
                    ));
                }
            }
        }
        result
    }
}

#[derive(Clone, Debug)]
pub struct ReadmemhElement {
    pub current_offset: isize,
    pub next_offset: Option<isize>,
    pub is_ff: bool,
}

#[derive(Clone)]
pub enum SystemFunctionCall {
    Display {
        format_str: String,
        args: Vec<Expression>,
    },
    Readmemh {
        filename: String,
        /// (current_ptr, next_ptr, native_bytes, use_4state)
        elements: Vec<(*mut u8, Option<*mut u8>, usize, bool)>,
        width: usize,
    },
    Assert {
        condition: Expression,
        message: Option<String>,
    },
    Finish,
}

#[derive(Clone)]
pub struct AssignDynamicStatement {
    pub dst_base_ptr: *mut u8,
    pub dst_stride: isize,
    pub dst_num_elements: usize,
    pub dst_index_expr: Expression,
    pub dst_width: usize,
    pub dst_native_bytes: usize,
    pub dst_use_4state: bool,
    pub select: Option<(usize, usize)>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: Expression,
}

#[derive(Clone, Debug)]
pub enum ProtoTbMethodKind {
    ClockNext { count: Option<ProtoExpression> },
    ResetAssert,
}

#[derive(Clone)]
pub enum TbMethodKind {
    ClockNext { count: Option<Expression> },
    ResetAssert,
}

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    AssignDynamic(AssignDynamicStatement),
    If(IfStatement),
    Binary(FuncPtr, *const u8, *const u8),
    BinaryBatch(FuncPtr, Vec<(*const u8, *const u8)>),
    SystemFunctionCall(SystemFunctionCall),
    TbMethodCall { inst: StrId, method: TbMethodKind },
}

impl Statement {
    pub fn is_binary(&self) -> bool {
        matches!(
            self,
            Statement::Binary(_, _, _) | Statement::BinaryBatch(_, _)
        )
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Statement::Assign(_) => "Assign",
            Statement::AssignDynamic(_) => "AssignDynamic",
            Statement::If(_) => "If",
            Statement::Binary(_, _, _) => "Binary",
            Statement::BinaryBatch(_, _) => "BinaryBatch",
            Statement::SystemFunctionCall(_) => "SystemFunctionCall",
            Statement::TbMethodCall { .. } => "TbMethodCall",
        }
    }

    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        match self {
            Statement::Assign(x) => x.eval_step(mask_cache),
            Statement::AssignDynamic(x) => x.eval_step(mask_cache),
            Statement::If(x) => x.eval_step(mask_cache),
            Statement::Binary(func, ff_values, comb_values) => unsafe {
                func(*ff_values, *comb_values);
            },
            Statement::BinaryBatch(func, args) => unsafe {
                for &(ff_values, comb_values) in args {
                    func(ff_values, comb_values);
                }
            },
            Statement::SystemFunctionCall(x) => x.eval_step(mask_cache),
            Statement::TbMethodCall { .. } => (),
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>, outputs: &mut Vec<*const u8>) {
        match self {
            Statement::Assign(x) => x.gather_variable(inputs, outputs),
            Statement::AssignDynamic(x) => x.gather_variable(inputs, outputs),
            Statement::If(x) => x.gather_variable(inputs, outputs),
            Statement::Binary(_, _, _) | Statement::BinaryBatch(_, _) => (),
            Statement::SystemFunctionCall(x) => x.gather_variable(inputs),
            Statement::TbMethodCall { .. } => (),
        }
    }
}

fn format_display_string(format_str: &str, values: &[veryl_analyzer::value::Value]) -> String {
    let mut result = String::new();
    let mut chars = format_str.chars().peekable();
    let mut arg_idx = 0;

    while let Some(ch) = chars.next() {
        if ch == '%' {
            if let Some(&spec) = chars.peek() {
                chars.next();
                match spec {
                    '%' => result.push('%'),
                    'h' | 'H' | 'x' | 'X' => {
                        if let Some(v) = values.get(arg_idx) {
                            result.push_str(&v.format_hex());
                        }
                        arg_idx += 1;
                    }
                    'd' | 'D' => {
                        if let Some(v) = values.get(arg_idx) {
                            result.push_str(&v.format_dec());
                        }
                        arg_idx += 1;
                    }
                    'o' | 'O' => {
                        if let Some(v) = values.get(arg_idx) {
                            result.push_str(&v.format_oct());
                        }
                        arg_idx += 1;
                    }
                    'b' | 'B' => {
                        if let Some(v) = values.get(arg_idx) {
                            result.push_str(&v.format_bin());
                        }
                        arg_idx += 1;
                    }
                    'c' | 'C' => {
                        if let Some(v) = values.get(arg_idx) {
                            let ch = (v.payload_u64() & 0xFF) as u8 as char;
                            result.push(ch);
                        }
                        arg_idx += 1;
                    }
                    's' | 'S' => {
                        if let Some(v) = values.get(arg_idx) {
                            result.push_str(&v.format_dec());
                        }
                        arg_idx += 1;
                    }
                    'm' | 'M' => {
                        result.push_str("<hierarchy>");
                    }
                    't' | 'T' => {
                        result.push('0');
                    }
                    _ => {
                        result.push('%');
                        result.push(spec);
                    }
                }
            } else {
                result.push('%');
            }
        } else {
            result.push(ch);
        }
    }
    result
}

impl SystemFunctionCall {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        match self {
            SystemFunctionCall::Display { format_str, args } => {
                let values: Vec<_> = args.iter().map(|e| e.eval(mask_cache)).collect();
                if format_str.is_empty() {
                    let parts: Vec<String> = values.iter().map(|v| v.format_hex()).collect();
                    println!("{}", parts.join(" "));
                } else if values.is_empty() {
                    println!("{format_str}");
                } else {
                    let output = format_display_string(format_str, &values);
                    println!("{output}");
                }
            }
            SystemFunctionCall::Readmemh {
                filename,
                elements,
                width,
            } => {
                let values = parse_hex_file(filename, *width);
                let count = values.len().min(elements.len());
                for i in 0..count {
                    let (current, next, nb, use_4state) = elements[i];
                    unsafe { write_native_value(current, nb, use_4state, &values[i]) };
                    if let Some(next) = next {
                        unsafe { write_native_value(next, nb, use_4state, &values[i]) };
                    }
                }
            }
            SystemFunctionCall::Assert { condition, message } => {
                let val = condition.eval(mask_cache);
                if val.payload_u64() == 0 {
                    let msg = message.as_deref().unwrap_or("assertion failed");
                    panic!("$assert failed: {msg}");
                }
            }
            SystemFunctionCall::Finish => {
                // Handled by testbench driver
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>) {
        match self {
            SystemFunctionCall::Display { args, .. } => {
                for e in args {
                    let mut dummy_outputs = vec![];
                    e.gather_variable(inputs, &mut dummy_outputs);
                }
            }
            SystemFunctionCall::Readmemh { .. } => {}
            SystemFunctionCall::Assert { condition, .. } => {
                let mut dummy_outputs = vec![];
                condition.gather_variable(inputs, &mut dummy_outputs);
            }
            SystemFunctionCall::Finish => {}
        }
    }
}

#[derive(Clone, Debug)]
pub enum ProtoSystemFunctionCall {
    Display {
        format_str: String,
        args: Vec<ProtoExpression>,
    },
    Readmemh {
        filename: String,
        elements: Vec<ReadmemhElement>,
        width: usize,
    },
    Assert {
        condition: ProtoExpression,
        message: Option<String>,
    },
    Finish,
}

#[derive(Clone, Debug)]
pub struct ProtoAssignDynamicStatement {
    pub dst_base_offset: isize,
    pub dst_stride: isize,
    pub dst_is_ff: bool,
    pub dst_num_elements: usize,
    pub dst_index_expr: ProtoExpression,
    pub dst_width: usize,
    pub select: Option<(usize, usize)>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: ProtoExpression,
    /// Canonical (current) base byte offset for FF variables.
    pub dst_ff_current_base_offset: isize,
}

impl ProtoAssignDynamicStatement {
    pub fn can_build_binary(&self) -> bool {
        self.dst_width <= 64
            && self.expr.can_build_binary()
            && self.dst_index_expr.can_build_binary()
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        let (mut payload, mut mask_xz) = self.expr.build_binary(context, builder)?;
        let nb = calc_native_bytes(self.dst_width);
        let nb_i32 = nb as i32;

        if let Some((beg, end)) = self.rhs_select {
            let mask = ValueU64::gen_mask(beg - end + 1);

            payload = builder.ins().ushr_imm(payload, end as i64);
            payload = builder.ins().band_imm(payload, mask as i64);

            if let Some(mxz) = mask_xz {
                let mxz = builder.ins().ushr_imm(mxz, end as i64);
                let mxz = builder.ins().band_imm(mxz, mask as i64);
                mask_xz = Some(mxz);
            }
        }

        // Compute dynamic address
        let (idx_payload, _idx_mask_xz) = self.dst_index_expr.build_binary(context, builder)?;

        let max_idx = builder
            .ins()
            .iconst(I64, (self.dst_num_elements as i64).saturating_sub(1));
        let in_bounds = builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, idx_payload, max_idx);
        let clamped = builder.ins().select(in_bounds, idx_payload, max_idx);

        let stride_val = builder.ins().iconst(I64, self.dst_stride as i64);
        let byte_offset = builder.ins().imul(clamped, stride_val);

        let base_addr = if self.dst_is_ff {
            context.ff_values
        } else {
            context.comb_values
        };
        let static_offset = builder.ins().iconst(I64, self.dst_base_offset as i64);
        let addr = builder.ins().iadd(base_addr, static_offset);
        let addr = builder.ins().iadd(addr, byte_offset);

        let load_mem_flag = MemFlags::trusted().with_readonly();
        let store_mem_flag = MemFlags::trusted();

        if let Some((beg, end)) = self.select {
            let mask = ValueU64::gen_mask_range(beg, end);
            let load_type = if nb == 4 { I32 } else { I64 };

            let payload = builder.ins().ishl_imm(payload, end as i64);
            let org = builder.ins().load(load_type, load_mem_flag, addr, 0);
            let org = if nb == 4 {
                builder.ins().uextend(I64, org)
            } else {
                org
            };
            let org = builder.ins().band_imm(org, !mask as i64);
            let result = builder.ins().bor(payload, org);
            if nb == 4 {
                // istore32 expects I64 and truncates internally
                builder.ins().istore32(store_mem_flag, result, addr, 0);
            } else {
                builder.ins().store(store_mem_flag, result, addr, 0);
            }
            if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm(mask_xz, end as i64);
                let org = builder.ins().load(load_type, load_mem_flag, addr, nb_i32);
                let org = if nb == 4 {
                    builder.ins().uextend(I64, org)
                } else {
                    org
                };
                let org = builder.ins().band_imm(org, !mask as i64);
                let result = builder.ins().bor(mask_xz, org);
                if nb == 4 {
                    builder.ins().istore32(store_mem_flag, result, addr, nb_i32);
                } else {
                    builder.ins().store(store_mem_flag, result, addr, nb_i32);
                }
            }
        } else {
            match self.dst_width {
                8 => {
                    builder.ins().istore8(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder.ins().istore8(store_mem_flag, mask_xz, addr, nb_i32);
                    }
                }
                16 => {
                    builder.ins().istore16(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder
                            .ins()
                            .istore16(store_mem_flag, mask_xz, addr, nb_i32);
                    }
                }
                32 => {
                    builder.ins().istore32(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder
                            .ins()
                            .istore32(store_mem_flag, mask_xz, addr, nb_i32);
                    }
                }
                64 => {
                    builder.ins().store(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder.ins().store(store_mem_flag, mask_xz, addr, nb_i32);
                    }
                }
                _ => {
                    if self.dst_width >= 64 {
                        return None;
                    }
                    let mask = (1u64 << self.dst_width) - 1;
                    let payload = builder.ins().band_imm(payload, mask as i64);
                    if nb == 4 {
                        builder.ins().istore32(store_mem_flag, payload, addr, 0);
                    } else {
                        builder.ins().store(store_mem_flag, payload, addr, 0);
                    }
                    if let Some(mask_xz) = mask_xz {
                        let mask_xz = builder.ins().band_imm(mask_xz, mask as i64);
                        if nb == 4 {
                            builder
                                .ins()
                                .istore32(store_mem_flag, mask_xz, addr, nb_i32);
                        } else {
                            builder.ins().store(store_mem_flag, mask_xz, addr, nb_i32);
                        }
                    }
                }
            }
        }

        Some(())
    }
}

/// A pre-compiled JIT block reused from a cached module instance.
/// Stores the function pointer and byte deltas so that the same compiled
/// code can be called with adjusted ff/comb base pointers.
#[derive(Clone, Debug)]
pub struct CompiledBlockStatement {
    pub func: FuncPtr,
    pub ff_delta_bytes: isize,
    pub comb_delta_bytes: isize,
    pub input_offsets: Vec<(bool, isize)>,
    pub output_offsets: Vec<(bool, isize)>,
}

#[derive(Clone, Debug)]
pub enum ProtoStatement {
    Assign(ProtoAssignStatement),
    AssignDynamic(ProtoAssignDynamicStatement),
    If(ProtoIfStatement),
    SystemFunctionCall(ProtoSystemFunctionCall),
    CompiledBlock(CompiledBlockStatement),
    TbMethodCall {
        inst: StrId,
        method: ProtoTbMethodKind,
    },
}

impl ProtoStatement {
    pub fn can_build_binary(&self) -> bool {
        match self {
            ProtoStatement::Assign(x) => x.can_build_binary(),
            ProtoStatement::AssignDynamic(x) => x.can_build_binary(),
            ProtoStatement::If(x) => x.can_build_binary(),
            ProtoStatement::SystemFunctionCall(_) => false,
            ProtoStatement::CompiledBlock(_) => false,
            ProtoStatement::TbMethodCall { .. } => false,
        }
    }

    /// Split a ProtoStatement::If with no condition (i.e. if_reset) into its two sides.
    pub fn split_if_reset(self) -> Option<(Vec<ProtoStatement>, Vec<ProtoStatement>)> {
        if let ProtoStatement::If(x) = self {
            if x.cond.is_some() {
                return None;
            }
            Some((x.true_side, x.false_side))
        } else {
            None
        }
    }

    pub fn gather_variable_offsets(
        &self,
        inputs: &mut Vec<(bool, isize)>,
        outputs: &mut Vec<(bool, isize)>,
    ) {
        match self {
            ProtoStatement::Assign(x) => {
                x.expr.gather_variable_offsets(inputs);
                outputs.push((x.dst_is_ff, x.dst_offset));
            }
            ProtoStatement::AssignDynamic(x) => {
                x.dst_index_expr.gather_variable_offsets(inputs);
                x.expr.gather_variable_offsets(inputs);
                for i in 0..x.dst_num_elements {
                    let offset = x.dst_base_offset + x.dst_stride * i as isize;
                    outputs.push((x.dst_is_ff, offset));
                }
            }
            ProtoStatement::If(x) => {
                if let Some(cond) = &x.cond {
                    cond.gather_variable_offsets(inputs);
                }
                for s in &x.true_side {
                    s.gather_variable_offsets(inputs, outputs);
                }
                for s in &x.false_side {
                    s.gather_variable_offsets(inputs, outputs);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. } => {
                    for arg in args {
                        arg.gather_variable_offsets(inputs);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { .. } => {}
                ProtoSystemFunctionCall::Assert { condition, .. } => {
                    condition.gather_variable_offsets(inputs);
                }
                ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(x) => {
                inputs.extend_from_slice(&x.input_offsets);
                outputs.extend_from_slice(&x.output_offsets);
            }
            ProtoStatement::TbMethodCall { .. } => {}
        }
    }

    /// Returns the set of canonical (current) FF byte offsets written by this statement.
    pub fn gather_ff_canonical_offsets(&self) -> HashSet<isize> {
        let mut result = HashSet::default();
        match self {
            ProtoStatement::Assign(x) => {
                if x.dst_is_ff {
                    result.insert(x.dst_ff_current_offset);
                }
            }
            ProtoStatement::AssignDynamic(x) => {
                if x.dst_is_ff {
                    for i in 0..x.dst_num_elements {
                        let current_stride = x.dst_stride;
                        result.insert(x.dst_ff_current_base_offset + current_stride * i as isize);
                    }
                }
            }
            ProtoStatement::If(x) => {
                for s in &x.true_side {
                    result.extend(s.gather_ff_canonical_offsets());
                }
                for s in &x.false_side {
                    result.extend(s.gather_ff_canonical_offsets());
                }
            }
            ProtoStatement::SystemFunctionCall(_) => {}
            ProtoStatement::CompiledBlock(x) => {
                for (is_ff, off) in &x.output_offsets {
                    if *is_ff {
                        result.insert(*off);
                    }
                }
            }
            ProtoStatement::TbMethodCall { .. } => {}
        }
        result
    }

    /// # Safety
    /// `ff_values_ptr` and `comb_values_ptr` must point to valid buffers.
    pub unsafe fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut u8,
        comb_values_ptr: *mut u8,
        use_4state: bool,
    ) -> Statement {
        unsafe {
            match self {
                ProtoStatement::Assign(x) => Statement::Assign(x.apply_values_ptr(
                    ff_values_ptr,
                    comb_values_ptr,
                    use_4state,
                )),
                ProtoStatement::AssignDynamic(x) => {
                    let nb = calc_native_bytes(x.dst_width);
                    let dst_base_ptr = if x.dst_is_ff {
                        ff_values_ptr.offset(x.dst_base_offset)
                    } else {
                        comb_values_ptr.offset(x.dst_base_offset)
                    };
                    let dst_index_expr = x.dst_index_expr.apply_values_ptr(
                        ff_values_ptr,
                        comb_values_ptr,
                        use_4state,
                    );
                    let expr = x
                        .expr
                        .apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
                    Statement::AssignDynamic(AssignDynamicStatement {
                        dst_base_ptr,
                        dst_stride: x.dst_stride,
                        dst_num_elements: x.dst_num_elements,
                        dst_index_expr,
                        dst_width: x.dst_width,
                        dst_native_bytes: nb,
                        dst_use_4state: use_4state,
                        select: x.select,
                        rhs_select: x.rhs_select,
                        expr,
                    })
                }
                ProtoStatement::If(x) => {
                    Statement::If(x.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state))
                }
                ProtoStatement::SystemFunctionCall(x) => match x {
                    ProtoSystemFunctionCall::Display { format_str, args } => {
                        let args = args
                            .iter()
                            .map(|a| a.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state))
                            .collect();
                        Statement::SystemFunctionCall(SystemFunctionCall::Display {
                            format_str: format_str.clone(),
                            args,
                        })
                    }
                    ProtoSystemFunctionCall::Readmemh {
                        filename,
                        elements,
                        width,
                    } => {
                        let nb = calc_native_bytes(*width);
                        let resolved: Vec<_> = elements
                            .iter()
                            .map(|elem| {
                                let current = if elem.is_ff {
                                    ff_values_ptr.offset(elem.current_offset)
                                } else {
                                    comb_values_ptr.offset(elem.current_offset)
                                };
                                let next = elem.next_offset.map(|off| ff_values_ptr.offset(off));
                                (current, next, nb, use_4state)
                            })
                            .collect();
                        Statement::SystemFunctionCall(SystemFunctionCall::Readmemh {
                            filename: filename.clone(),
                            elements: resolved,
                            width: *width,
                        })
                    }
                    ProtoSystemFunctionCall::Assert { condition, message } => {
                        let condition =
                            condition.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);
                        Statement::SystemFunctionCall(SystemFunctionCall::Assert {
                            condition,
                            message: message.clone(),
                        })
                    }
                    ProtoSystemFunctionCall::Finish => {
                        Statement::SystemFunctionCall(SystemFunctionCall::Finish)
                    }
                },
                ProtoStatement::CompiledBlock(x) => {
                    let adjusted_ff = (ff_values_ptr as *const u8).offset(x.ff_delta_bytes);
                    let adjusted_comb = (comb_values_ptr as *const u8).offset(x.comb_delta_bytes);
                    Statement::Binary(x.func, adjusted_ff, adjusted_comb)
                }
                ProtoStatement::TbMethodCall { inst, method } => {
                    let method = match method {
                        ProtoTbMethodKind::ClockNext { count } => {
                            let count = count.as_ref().map(|e| {
                                e.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state)
                            });
                            TbMethodKind::ClockNext { count }
                        }
                        ProtoTbMethodKind::ResetAssert => TbMethodKind::ResetAssert,
                    };
                    Statement::TbMethodCall {
                        inst: *inst,
                        method,
                    }
                }
            }
        }
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) -> Option<()> {
        match self {
            ProtoStatement::Assign(x) => x.build_binary(context, builder),
            ProtoStatement::AssignDynamic(x) => {
                let result = x.build_binary(context, builder);
                // Dynamic assigns write to runtime-computed addresses; invalidate all cached loads
                context.load_cache.clear();
                result
            }
            ProtoStatement::If(x) => x.build_binary(context, builder, is_last),
            ProtoStatement::SystemFunctionCall(_) => None,
            ProtoStatement::CompiledBlock(_) => None,
            ProtoStatement::TbMethodCall { .. } => None,
        }
    }
}

#[derive(Clone)]
pub struct AssignStatement {
    pub dst: *mut u8,
    pub dst_width: usize,
    pub dst_native_bytes: usize,
    pub dst_use_4state: bool,
    pub select: Option<(usize, usize)>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: Expression,
}

impl AssignStatement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        let value = self.expr.eval(mask_cache);
        let value = if let Some((beg, end)) = self.rhs_select {
            value.select(beg, end)
        } else {
            value
        };
        if let Some((beg, end)) = self.select {
            let mut current = unsafe {
                read_native_value(
                    self.dst,
                    self.dst_native_bytes,
                    self.dst_use_4state,
                    self.dst_width as u32,
                    false,
                )
            };
            current.assign(value, beg, end);
            unsafe {
                write_native_value(
                    self.dst,
                    self.dst_native_bytes,
                    self.dst_use_4state,
                    &current,
                );
            }
        } else {
            let mut value = value;
            value.trunc(self.dst_width);
            unsafe {
                write_native_value(self.dst, self.dst_native_bytes, self.dst_use_4state, &value);
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>, outputs: &mut Vec<*const u8>) {
        self.expr.gather_variable(inputs, outputs);
        outputs.push(self.dst);
    }
}

impl AssignDynamicStatement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        let idx_val = self.dst_index_expr.eval(mask_cache);
        let idx = idx_val
            .to_usize()
            .unwrap_or(0)
            .min(self.dst_num_elements - 1);
        let dst = unsafe { self.dst_base_ptr.offset(self.dst_stride * idx as isize) };

        let value = self.expr.eval(mask_cache);
        let value = if let Some((beg, end)) = self.rhs_select {
            value.select(beg, end)
        } else {
            value
        };
        if let Some((beg, end)) = self.select {
            let mut current = unsafe {
                read_native_value(
                    dst,
                    self.dst_native_bytes,
                    self.dst_use_4state,
                    self.dst_width as u32,
                    false,
                )
            };
            current.assign(value, beg, end);
            unsafe {
                write_native_value(dst, self.dst_native_bytes, self.dst_use_4state, &current)
            };
        } else {
            let mut value = value;
            value.trunc(self.dst_width);
            unsafe { write_native_value(dst, self.dst_native_bytes, self.dst_use_4state, &value) };
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>, outputs: &mut Vec<*const u8>) {
        self.dst_index_expr.gather_variable(inputs, &mut vec![]);
        self.expr.gather_variable(inputs, &mut vec![]);
        for i in 0..self.dst_num_elements {
            let ptr =
                unsafe { self.dst_base_ptr.offset(self.dst_stride * i as isize) as *const u8 };
            outputs.push(ptr);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProtoAssignStatement {
    pub dst_offset: isize,
    pub dst_is_ff: bool,
    pub dst_width: usize,
    pub select: Option<(usize, usize)>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: ProtoExpression,
    /// Canonical (current) byte offset for FF variables.
    /// Used by sort_ff_event to identify which FF slots are written.
    pub dst_ff_current_offset: isize,
}

impl ProtoAssignStatement {
    pub fn can_build_binary(&self) -> bool {
        if !self.expr.can_build_binary() {
            return false;
        }
        true
    }

    /// # Safety
    /// `ff_values_ptr` and `comb_values_ptr` must point to valid buffers.
    pub unsafe fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut u8,
        comb_values_ptr: *mut u8,
        use_4state: bool,
    ) -> AssignStatement {
        unsafe {
            let nb = calc_native_bytes(self.dst_width);
            let dst = if self.dst_is_ff {
                ff_values_ptr.add(self.dst_offset as usize)
            } else {
                comb_values_ptr.add(self.dst_offset as usize)
            };

            let expr = self
                .expr
                .apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state);

            AssignStatement {
                dst,
                dst_width: self.dst_width,
                dst_native_bytes: nb,
                dst_use_4state: use_4state,
                select: self.select,
                rhs_select: self.rhs_select,
                expr,
            }
        }
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        // Wide path: >128-bit destination
        if self.dst_width > 128 {
            return self.build_binary_wide(context, builder);
        }

        let known_bits = self.expr.effective_bits();
        let wide = self.dst_width > 64;

        let (mut payload, mut mask_xz) = self.expr.build_binary(context, builder)?;
        let nb = calc_native_bytes(self.dst_width);
        let nb_i32 = nb as i32;

        // Widen expression result to I128 if destination is 128-bit
        if wide && self.expr.width() <= 64 {
            payload = builder.ins().uextend(I128, payload);
            if let Some(mxz) = mask_xz {
                mask_xz = Some(builder.ins().uextend(I128, mxz));
            }
        }

        // Narrow known_bits if rhs_select is applied
        let known_bits = if let Some((beg, end)) = self.rhs_select {
            beg - end + 1
        } else {
            known_bits
        };

        if let Some((beg, end)) = self.rhs_select {
            let select_width = beg - end + 1;
            let mask = gen_mask_for_width(select_width);

            payload = builder.ins().ushr_imm(payload, end as i64);
            payload = band_const(builder, payload, mask, wide);

            if let Some(mxz) = mask_xz {
                let mxz = builder.ins().ushr_imm(mxz, end as i64);
                let mxz = band_const(builder, mxz, mask, wide);
                mask_xz = Some(mxz);
            }
        }

        let load_mem_flag = MemFlags::trusted().with_readonly();
        let store_mem_flag = MemFlags::trusted();

        let base_addr = if self.dst_is_ff {
            context.ff_values
        } else {
            context.comb_values
        };

        let dst_offset = self.dst_offset as i32;
        let cache_key = (self.dst_is_ff, dst_offset);

        if let Some((beg, end)) = self.select {
            // Read-modify-write with native width
            let payload = builder.ins().ishl_imm(payload, end as i64);

            let load_type = if nb == 16 {
                I128
            } else if nb == 4 {
                I32
            } else {
                I64
            };

            // Use cached value if available, otherwise load from memory
            let (org_payload, org_mask_xz) =
                if let Some(&(cached_p, cached_m)) = context.load_cache.get(&cache_key) {
                    (cached_p, cached_m)
                } else {
                    let p = builder
                        .ins()
                        .load(load_type, load_mem_flag, base_addr, dst_offset);
                    let p = if nb == 4 {
                        builder.ins().uextend(I64, p)
                    } else {
                        p
                    };
                    let m = if context.use_4state {
                        let m = builder.ins().load(
                            load_type,
                            load_mem_flag,
                            base_addr,
                            dst_offset + nb_i32,
                        );
                        Some(if nb == 4 {
                            builder.ins().uextend(I64, m)
                        } else {
                            m
                        })
                    } else {
                        None
                    };
                    (p, m)
                };

            let not_mask = if wide {
                let mask = gen_mask_range_128(beg, end);
                iconst_128(builder, !mask)
            } else {
                let mask = ValueU64::gen_mask_range(beg, end);
                builder.ins().iconst(I64, !mask as i64)
            };
            let org = builder.ins().band(org_payload, not_mask);
            let result = builder.ins().bor(payload, org);
            if nb == 4 {
                builder
                    .ins()
                    .istore32(store_mem_flag, result, base_addr, dst_offset);
            } else {
                builder
                    .ins()
                    .store(store_mem_flag, result, base_addr, dst_offset);
            }

            let result_mask_xz = if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm(mask_xz, end as i64);
                let z = if wide { context.zero_128 } else { context.zero };
                let org_m = org_mask_xz.unwrap_or(z);
                let org_m = builder.ins().band(org_m, not_mask);
                let result_m = builder.ins().bor(mask_xz, org_m);
                if nb == 4 {
                    builder.ins().istore32(
                        store_mem_flag,
                        result_m,
                        base_addr,
                        dst_offset + nb_i32,
                    );
                } else {
                    builder
                        .ins()
                        .store(store_mem_flag, result_m, base_addr, dst_offset + nb_i32);
                }
                Some(result_m)
            } else {
                None
            };

            // Forward stored value to load cache
            context
                .load_cache
                .insert(cache_key, (result, result_mask_xz));
        } else {
            // Store elimination: skip memory store for internal comb variables,
            // keeping only load_cache forwarding.
            let skip_store =
                context.store_elim_enabled && context.store_elim_offsets.contains(&cache_key);

            if !skip_store {
                let needs_trunc = known_bits > self.dst_width;

                match self.dst_width {
                    8 => {
                        builder
                            .ins()
                            .istore8(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().istore8(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    16 => {
                        builder
                            .ins()
                            .istore16(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().istore16(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    32 => {
                        builder
                            .ins()
                            .istore32(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().istore32(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    64 => {
                        builder
                            .ins()
                            .store(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().store(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    128 => {
                        builder
                            .ins()
                            .store(store_mem_flag, payload, base_addr, dst_offset);
                        if let Some(mask_xz) = mask_xz {
                            builder.ins().store(
                                store_mem_flag,
                                mask_xz,
                                base_addr,
                                dst_offset + nb_i32,
                            );
                        }
                    }
                    _ => {
                        if self.dst_width > 128 {
                            return None;
                        }
                        let payload = if needs_trunc {
                            let mask = gen_mask_for_width(self.dst_width);
                            band_const(builder, payload, mask, wide)
                        } else {
                            payload
                        };
                        // Store using the appropriate native width
                        if nb == 4 {
                            builder
                                .ins()
                                .istore32(store_mem_flag, payload, base_addr, dst_offset);
                        } else {
                            builder
                                .ins()
                                .store(store_mem_flag, payload, base_addr, dst_offset);
                        }
                        if let Some(mask_xz) = mask_xz {
                            let mask_xz = if needs_trunc {
                                let mask = gen_mask_for_width(self.dst_width);
                                band_const(builder, mask_xz, mask, wide)
                            } else {
                                mask_xz
                            };
                            if nb == 4 {
                                builder.ins().istore32(
                                    store_mem_flag,
                                    mask_xz,
                                    base_addr,
                                    dst_offset + nb_i32,
                                );
                            } else {
                                builder.ins().store(
                                    store_mem_flag,
                                    mask_xz,
                                    base_addr,
                                    dst_offset + nb_i32,
                                );
                            }
                        }
                    }
                }
            }

            // Forward value to load cache (always, even when store is eliminated)
            context.load_cache.insert(cache_key, (payload, mask_xz));
        }

        Some(())
    }

    /// Wide (>128-bit) store: copy from expression pointer to destination memory.
    fn build_binary_wide(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        use crate::ir::expression::{emit_wide_apply_mask, is_wide_ptr};

        // Select on wide destination: fall back to interpreter
        if self.select.is_some() || self.rhs_select.is_some() {
            return None;
        }

        let expr_width = self.expr.width();
        let (payload, mask_xz) = self.expr.build_binary(context, builder)?;
        let nb = calc_native_bytes(self.dst_width);
        let n_words = nb / 8;
        let flags = MemFlags::trusted();

        let base_addr = if self.dst_is_ff {
            context.ff_values
        } else {
            context.comb_values
        };
        let dst_offset = self.dst_offset as i32;

        // Source is a wide pointer (for >128-bit expressions)
        // Use expr_width captured before build_binary to determine representation.
        // build_binary returns a pointer for width > 128, register otherwise.
        let src_ptr = if is_wide_ptr(expr_width) {
            payload
        } else {
            // Expression was narrow but dest is wide — store into temp slot
            use crate::ir::expression::ensure_wide_ptr_val;
            ensure_wide_ptr_val(builder, payload, expr_width, nb)
        };

        // Apply width mask to the source to truncate extra bits
        emit_wide_apply_mask(context, builder, src_ptr, nb, self.dst_width);

        // Copy word by word from source to destination
        for i in 0..n_words {
            let off = (i * 8) as i32;
            let val = builder.ins().load(I64, flags, src_ptr, off);
            builder.ins().store(flags, val, base_addr, dst_offset + off);
        }

        // 4-state mask
        if let Some(mask_xz) = mask_xz {
            let mask_ptr = if is_wide_ptr(self.expr.width()) {
                mask_xz
            } else {
                use crate::ir::expression::ensure_wide_ptr_val;
                ensure_wide_ptr_val(builder, mask_xz, self.expr.width(), nb)
            };
            emit_wide_apply_mask(context, builder, mask_ptr, nb, self.dst_width);
            for i in 0..n_words {
                let off = (i * 8) as i32;
                let val = builder.ins().load(I64, flags, mask_ptr, off);
                builder
                    .ins()
                    .store(flags, val, base_addr, dst_offset + nb as i32 + off);
            }
        }

        Some(())
    }
}

#[derive(Clone)]
pub struct IfStatement {
    pub cond: Option<Expression>,
    pub true_side: Vec<Statement>,
    pub false_side: Vec<Statement>,
}

impl IfStatement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        let cond = if let Some(x) = &self.cond {
            let cond = x.eval(mask_cache);
            match &cond {
                Value::U64(x) => (x.payload & !x.mask_xz) != 0,
                Value::BigUint(x) => {
                    use veryl_analyzer::value::biguint_to_u128;
                    let payload = biguint_to_u128(&x.payload);
                    let mask_xz = biguint_to_u128(&x.mask_xz);
                    (payload & !mask_xz) != 0
                }
            }
        } else {
            false
        };

        if cond {
            for x in &self.true_side {
                x.eval_step(mask_cache);
            }
        } else {
            for x in &self.false_side {
                x.eval_step(mask_cache);
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>, outputs: &mut Vec<*const u8>) {
        if let Some(x) = &self.cond {
            x.gather_variable(inputs, outputs);
        }

        for x in &self.true_side {
            x.gather_variable(inputs, outputs);
        }
        for x in &self.false_side {
            x.gather_variable(inputs, outputs);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProtoIfStatement {
    pub cond: Option<ProtoExpression>,
    pub true_side: Vec<ProtoStatement>,
    pub false_side: Vec<ProtoStatement>,
}

impl ProtoIfStatement {
    pub fn can_build_binary(&self) -> bool {
        if let Some(cond) = &self.cond
            && !cond.can_build_binary()
        {
            return false;
        }
        self.true_side.iter().all(|s| s.can_build_binary())
            && self.false_side.iter().all(|s| s.can_build_binary())
    }

    /// # Safety
    /// `ff_values_ptr` and `comb_values_ptr` must point to valid buffers.
    pub unsafe fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut u8,
        comb_values_ptr: *mut u8,
        use_4state: bool,
    ) -> IfStatement {
        unsafe {
            let cond = self
                .cond
                .as_ref()
                .map(|x| x.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state));
            let true_side: Vec<_> = self
                .true_side
                .iter()
                .map(|x| x.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state))
                .collect();
            let false_side: Vec<_> = self
                .false_side
                .iter()
                .map(|x| x.apply_values_ptr(ff_values_ptr, comb_values_ptr, use_4state))
                .collect();

            IfStatement {
                cond,
                true_side,
                false_side,
            }
        }
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) -> Option<()> {
        let true_block = builder.create_block();
        let false_block = builder.create_block();
        let final_block = builder.create_block();

        // Evaluate condition
        if let Some(x) = &self.cond {
            let (cond_payload, cond_mask_xz) = x.build_binary(context, builder)?;
            let effective_cond = if let Some(mask_xz) = cond_mask_xz {
                builder.ins().band_not(cond_payload, mask_xz)
            } else {
                cond_payload
            };
            builder
                .ins()
                .brif(effective_cond, true_block, &[], false_block, &[]);
        }

        context.load_cache.clear();
        // Disable store elimination inside If blocks: load_cache is cleared
        // at block boundaries, so eliminated stores would leave stale values.
        let prev_store_elim = context.store_elim_enabled;
        context.store_elim_enabled = false;

        builder.switch_to_block(true_block);
        let len = self.true_side.len();
        for (i, x) in self.true_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        context.load_cache.clear();

        builder.switch_to_block(false_block);
        let len = self.true_side.len();
        for (i, x) in self.false_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        builder.switch_to_block(final_block);

        context.load_cache.clear();
        context.store_elim_enabled = prev_store_elim;

        Some(())
    }
}

fn extract_display_args(
    context: &mut ConvContext,
    inputs: &[SystemFunctionInput],
) -> Option<(String, Vec<ProtoExpression>)> {
    let mut format_str = String::new();
    let mut exprs = Vec::new();
    let mut iter = inputs.iter();

    // Check if the first argument is a string literal
    if let Some(first) = iter.next() {
        if is_string_literal(&first.0) {
            format_str = extract_string_value(&first.0)?;
        } else {
            // Not a string literal; treat as expression argument
            let proto: ProtoExpression = Conv::conv(context, &first.0)?;
            exprs.push(proto);
        }
    }

    for input in iter {
        let proto: ProtoExpression = Conv::conv(context, &input.0)?;
        exprs.push(proto);
    }

    Some((format_str, exprs))
}

fn is_string_literal(expr: &air::Expression) -> bool {
    if let air::Expression::Term(factor) = expr
        && let air::Factor::Value(comptime) = factor.as_ref()
    {
        return comptime.r#type.kind == TypeKind::String;
    }
    false
}

fn extract_string_value(expr: &air::Expression) -> Option<String> {
    if let air::Expression::Term(factor) = expr
        && let air::Factor::Value(comptime) = factor.as_ref()
        && let ValueVariant::Numeric(value) = &comptime.value
    {
        let str_id_raw = value.to_usize()?;
        let str_id = veryl_parser::resource_table::StrId(str_id_raw);
        return veryl_parser::resource_table::get_str_value(str_id);
    }
    None
}

impl Conv<&air::Statement> for Vec<ProtoStatement> {
    fn conv(context: &mut ConvContext, src: &air::Statement) -> Option<Self> {
        let mut result = match src {
            air::Statement::Assign(x) => Conv::conv(context, x)?,
            air::Statement::FunctionCall(x) => Conv::conv(context, x.as_ref())?,
            air::Statement::If(x) => {
                let x: ProtoIfStatement = Conv::conv(context, x)?;
                vec![ProtoStatement::If(x)]
            }
            air::Statement::IfReset(x) => {
                let x: ProtoIfStatement = Conv::conv(context, x)?;
                vec![ProtoStatement::If(x)]
            }
            air::Statement::SystemFunctionCall(x) => match &x.kind {
                SystemFunctionKind::Display(inputs) => {
                    let (format_str, exprs) = extract_display_args(context, inputs)?;
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Display {
                            format_str,
                            args: exprs,
                        },
                    )]
                }
                SystemFunctionKind::Readmemh(input, output) => {
                    let raw = extract_string_value(&input.0)?;
                    let filename = raw.trim_matches('"').to_string();
                    let dst = &output.0[0];
                    let id = dst.id;
                    let scope = context.scope();
                    let meta = scope.variable_meta.get(&id)?;
                    let width = meta.width;
                    let elements: Vec<ReadmemhElement> = meta
                        .elements
                        .iter()
                        .map(|elem| ReadmemhElement {
                            current_offset: elem.current_offset,
                            next_offset: if elem.is_ff {
                                Some(elem.next_offset)
                            } else {
                                None
                            },
                            is_ff: elem.is_ff,
                        })
                        .collect();
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Readmemh {
                            filename,
                            elements,
                            width,
                        },
                    )]
                }
                SystemFunctionKind::Assert(cond_input, msg_input) => {
                    let condition: ProtoExpression = Conv::conv(context, &cond_input.0)?;
                    let message = msg_input.as_ref().and_then(|m| extract_string_value(&m.0));
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Assert { condition, message },
                    )]
                }
                SystemFunctionKind::Finish => {
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Finish,
                    )]
                }
                _ => return None,
            },
            air::Statement::TbMethodCall(x) => {
                let method = match &x.method {
                    air::TbMethod::ClockNext { count } => {
                        let count = if let Some(expr) = count {
                            Some(Conv::conv(context, expr)?)
                        } else {
                            None
                        };
                        ProtoTbMethodKind::ClockNext { count }
                    }
                    air::TbMethod::ResetAssert => ProtoTbMethodKind::ResetAssert,
                };
                vec![ProtoStatement::TbMethodCall {
                    inst: x.inst,
                    method,
                }]
            }
            _ => return None,
        };

        // Drain pending statements from function calls within expressions
        let mut pending = std::mem::take(&mut context.pending_statements);
        if !pending.is_empty() {
            pending.append(&mut result);
            result = pending;
        }

        Some(result)
    }
}

impl Conv<&air::AssignStatement> for Vec<ProtoStatement> {
    fn conv(context: &mut ConvContext, src: &air::AssignStatement) -> Option<Self> {
        if matches!(src.expr, air::Expression::ArrayLiteral(..)) {
            let dst = &src.dst[0];
            let scope = context.scope();
            let meta = scope.variable_meta.get(&dst.id)?;
            let dst_type = meta.r#type.clone();
            let mut expr_clone = src.expr.clone();

            let array_exprs = eval_array_literal(
                &mut scope.analyzer_context,
                Some(&dst_type.array),
                Some(&dst_type.width),
                &mut expr_clone,
            )
            .ok()??;

            let mut result = Vec::new();
            for array_expr in array_exprs {
                let index = array_expr.to_var_index();
                let select = array_expr.to_var_select();
                let mut new_dst = dst.clone();
                new_dst.index.append(&index);
                new_dst.select = select;

                let element_assign = air::AssignStatement {
                    dst: vec![new_dst],
                    width: src.width,
                    expr: array_expr.expr,
                    token: src.token,
                };
                let proto: ProtoAssignStatement = Conv::conv(context, &element_assign)?;
                result.push(ProtoStatement::Assign(proto));
            }
            return Some(result);
        }

        if src.dst.len() <= 1 {
            let stmt: ProtoStatement = Conv::conv(context, src)?;
            return Some(vec![stmt]);
        }

        let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

        let mut result = Vec::new();
        let mut remaining = src.width?;

        for dst in &src.dst {
            let scope = context.scope();
            let id = dst.id;
            let meta = scope.variable_meta.get(&id).unwrap();

            let select = if !dst.select.is_empty() {
                dst.select
                    .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
            } else {
                None
            };

            let dst_elem_width = if let Some((beg, end)) = select {
                beg - end + 1
            } else {
                dst.comptime.r#type.total_width()?
            };

            let rhs_select = Some((remaining - 1, remaining - dst_elem_width));
            remaining -= dst_elem_width;

            let const_index = if dst.index.is_const() {
                dst.index.eval_value(&mut scope.analyzer_context)
            } else {
                None
            };

            if let Some(idx_vals) = const_index {
                let index = meta.r#type.array.calc_index(&idx_vals)?;
                let element = &meta.elements[index];
                let dst_is_ff = element.is_ff;
                let dst_width = meta.width;
                let dst_offset = if dst_is_ff {
                    element.next_offset
                } else {
                    element.current_offset
                };

                result.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst_offset,
                    dst_is_ff,
                    dst_width,
                    select,
                    rhs_select,
                    expr: expr.clone(),
                    dst_ff_current_offset: element.current_offset,
                }));
            } else {
                let array_shape = meta.r#type.array.clone();
                let dyn_info = meta.dynamic_index_info()?;
                let num_elements = meta.elements.len();
                let (base_current, base_next, stride, is_ff) = dyn_info;
                let dst_base_offset = if is_ff { base_next } else { base_current };
                let dst_width = meta.width;

                let index_proto = build_linear_index_expr(context, &array_shape, &dst.index)?;

                result.push(ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
                    dst_base_offset,
                    dst_stride: stride,
                    dst_is_ff: is_ff,
                    dst_num_elements: num_elements,
                    dst_index_expr: index_proto,
                    dst_width,
                    select,
                    rhs_select,
                    expr: expr.clone(),
                    dst_ff_current_base_offset: base_current,
                }));
            }
        }

        Some(result)
    }
}

impl Conv<&air::AssignStatement> for ProtoStatement {
    fn conv(context: &mut ConvContext, src: &air::AssignStatement) -> Option<Self> {
        // TODO multiple dst
        let dst = &src.dst[0];
        let id = dst.id;

        let scope = context.scope();
        let meta = scope.variable_meta.get(&id).unwrap();
        let select = if !dst.select.is_empty() {
            dst.select
                .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
        } else {
            None
        };
        let dst_width = meta.width;
        let const_index = if dst.index.is_const() {
            dst.index.eval_value(&mut scope.analyzer_context)
        } else {
            None
        };

        if let Some(idx_vals) = const_index {
            let index = meta.r#type.array.calc_index(&idx_vals)?;
            let element = &meta.elements[index];
            let dst_is_ff = element.is_ff;
            let current_offset = element.current_offset;
            // FF assignment writes to next
            let dst_offset = if dst_is_ff {
                element.next_offset
            } else {
                current_offset
            };

            let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

            Some(ProtoStatement::Assign(ProtoAssignStatement {
                dst_offset,
                dst_is_ff,
                dst_width,
                select,
                rhs_select: None,
                expr,
                dst_ff_current_offset: current_offset,
            }))
        } else {
            // Dynamic index
            let array_shape = meta.r#type.array.clone();
            let dyn_info = meta.dynamic_index_info()?;
            let num_elements = meta.elements.len();
            let (base_current, base_next, stride, is_ff) = dyn_info;
            // FF assignment writes to next
            let dst_base_offset = if is_ff { base_next } else { base_current };

            let index_proto = build_linear_index_expr(context, &array_shape, &dst.index)?;
            let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

            Some(ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
                dst_base_offset,
                dst_stride: stride,
                dst_is_ff: is_ff,
                dst_num_elements: num_elements,
                dst_index_expr: index_proto,
                dst_width,
                select,
                rhs_select: None,
                expr,
                dst_ff_current_base_offset: base_current,
            }))
        }
    }
}

impl Conv<&air::AssignStatement> for ProtoAssignStatement {
    fn conv(context: &mut ConvContext, src: &air::AssignStatement) -> Option<Self> {
        let scope = context.scope();

        // TODO multiple dst
        let dst = &src.dst[0];

        let id = dst.id;
        let meta = scope.variable_meta.get(&id).unwrap();

        let index = dst.index.eval_value(&mut scope.analyzer_context)?;
        let index = meta.r#type.array.calc_index(&index)?;

        let select = if !dst.select.is_empty() {
            dst.select
                .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
        } else {
            None
        };

        let element = &meta.elements[index];
        let dst_is_ff = element.is_ff;
        let current_offset = element.current_offset;
        let dst_width = meta.width;
        // FF assignment writes to next
        let dst_offset = if dst_is_ff {
            element.next_offset
        } else {
            current_offset
        };

        let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

        Some(ProtoAssignStatement {
            dst_offset,
            dst_is_ff,
            dst_width,
            select,
            rhs_select: None,
            expr,
            dst_ff_current_offset: current_offset,
        })
    }
}

impl Conv<&air::IfStatement> for ProtoIfStatement {
    fn conv(context: &mut ConvContext, src: &air::IfStatement) -> Option<Self> {
        let cond: ProtoExpression = Conv::conv(context, &src.cond)?;

        let mut true_side = vec![];
        for x in &src.true_side {
            let stmts: Vec<ProtoStatement> = Conv::conv(context, x)?;
            true_side.extend(stmts);
        }

        let mut false_side = vec![];
        for x in &src.false_side {
            let stmts: Vec<ProtoStatement> = Conv::conv(context, x)?;
            false_side.extend(stmts);
        }

        Some(ProtoIfStatement {
            cond: Some(cond),
            true_side,
            false_side,
        })
    }
}

impl Conv<&air::IfResetStatement> for ProtoIfStatement {
    fn conv(context: &mut ConvContext, src: &air::IfResetStatement) -> Option<Self> {
        let mut true_side = vec![];
        for x in &src.true_side {
            let stmts: Vec<ProtoStatement> = Conv::conv(context, x)?;
            true_side.extend(stmts);
        }

        let mut false_side = vec![];
        for x in &src.false_side {
            let stmts: Vec<ProtoStatement> = Conv::conv(context, x)?;
            false_side.extend(stmts);
        }

        Some(ProtoIfStatement {
            cond: None,
            true_side,
            false_side,
        })
    }
}

impl Conv<&FunctionCall> for Vec<ProtoStatement> {
    fn conv(context: &mut ConvContext, src: &FunctionCall) -> Option<Self> {
        let mut result = Vec::new();

        // Clone to avoid borrow conflict with context
        let func = context
            .scope()
            .analyzer_context
            .functions
            .get(&src.id)?
            .clone();
        let body = if let Some(ref idx) = src.index {
            func.get_function(idx)?
        } else {
            func.get_function(&[])?
        };

        for (var_path, expr) in &src.inputs {
            let arg_var_id = body.arg_map.get(var_path)?;
            let proto_expr: ProtoExpression = Conv::conv(context, expr)?;
            let scope = context.scope();
            let meta = scope.variable_meta.get(arg_var_id)?;
            let element = &meta.elements[0];
            result.push(ProtoStatement::Assign(ProtoAssignStatement {
                dst_offset: element.current_offset,
                dst_is_ff: false,
                dst_width: meta.width,
                select: None,
                rhs_select: None,
                expr: proto_expr,
                dst_ff_current_offset: 0, // not FF
            }));
        }

        // Drain pending statements from nested function calls in input expressions
        let mut pending = std::mem::take(&mut context.pending_statements);
        pending.append(&mut result);
        result = pending;

        for stmt in &body.statements {
            let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
            result.extend(stmts);
        }

        for (var_path, destinations) in &src.outputs {
            let arg_var_id = body.arg_map.get(var_path)?;
            let scope = context.scope();
            let arg_meta = scope.variable_meta.get(arg_var_id)?;
            let arg_element = &arg_meta.elements[0];
            let arg_expr = ProtoExpression::Variable {
                offset: arg_element.current_offset,
                is_ff: false,
                select: None,
                width: arg_meta.width,
                expr_context: ExpressionContext {
                    width: arg_meta.width,
                    signed: false,
                },
            };
            for dst in destinations {
                let scope = context.scope();
                let dst_meta = scope.variable_meta.get(&dst.id)?;
                let dst_index = dst.index.eval_value(&mut scope.analyzer_context)?;
                let dst_index = dst_meta.r#type.array.calc_index(&dst_index)?;
                let dst_element = &dst_meta.elements[dst_index];

                let select = if !dst.select.is_empty() {
                    dst.select
                        .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
                } else {
                    None
                };

                let (dst_offset, dst_is_ff) = if dst_element.is_ff {
                    (dst_element.next_offset, true)
                } else {
                    (dst_element.current_offset, false)
                };

                result.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst_offset,
                    dst_is_ff,
                    dst_width: dst_meta.width,
                    select,
                    rhs_select: None,
                    expr: arg_expr.clone(),
                    dst_ff_current_offset: dst_element.current_offset,
                }));
            }
        }

        Some(result)
    }
}

fn parse_hex_file(filename: &str, width: usize) -> Vec<veryl_analyzer::value::Value> {
    let content = match std::fs::read_to_string(filename) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("$readmemh: failed to read '{}': {}", filename, e);
            return vec![];
        }
    };
    parse_hex_content(&content, width)
}

pub fn parse_hex_content(content: &str, width: usize) -> Vec<veryl_analyzer::value::Value> {
    let mut result = Vec::new();
    let mut s = content.to_string();

    // Remove block comments
    while let Some(start) = s.find("/*") {
        if let Some(end) = s[start..].find("*/") {
            s.replace_range(start..start + end + 2, " ");
        } else {
            s.truncate(start);
            break;
        }
    }

    for line in s.lines() {
        let line = if let Some(pos) = line.find("//") {
            &line[..pos]
        } else {
            line
        };

        for token in line.split_whitespace() {
            let cleaned: String = token.chars().filter(|&c| c != '_').collect();
            if cleaned.is_empty() {
                continue;
            }
            if let Ok(val) = u64::from_str_radix(&cleaned, 16) {
                result.push(veryl_analyzer::value::Value::new(val, width, false));
            }
        }
    }

    result
}
