use crate::FuncPtr;
use crate::HashSet;
#[cfg(not(target_family = "wasm"))]
use crate::cranelift::Context as CraneliftContext;
use crate::ir::context::{Context as ConvContext, Conv};
use crate::ir::expression::{
    DynamicBitSelect, ExpressionContext, ProtoDynamicBitSelect, build_dynamic_bit_select,
    build_linear_index_expr,
};
#[cfg(not(target_family = "wasm"))]
use crate::ir::expression::{
    band_const, build_dynamic_select_shift, gen_mask_for_width, gen_mask_range_128, iconst_128,
};
use crate::ir::variable::{
    VarOffset, native_bytes as calc_native_bytes, read_native_value, write_native_value,
};
use crate::ir::{Expression, ProtoExpression, Value};
use crate::output_buffer;
use crate::simulator_error::SimulatorError;
#[cfg(not(target_family = "wasm"))]
use cranelift::prelude::types::{I32, I64, I128};
#[cfg(not(target_family = "wasm"))]
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::conv::utils::eval_array_literal;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::FunctionCall;
use veryl_analyzer::ir::{SystemFunctionInput, SystemFunctionKind, TypeKind, ValueVariant};
use veryl_analyzer::value::MaskCache;
#[cfg(not(target_family = "wasm"))]
use veryl_analyzer::value::ValueU64;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

/// Per-statement dependency: (input offsets, output offsets).
pub type StmtDep = (Vec<VarOffset>, Vec<VarOffset>);

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
        ff_len: usize,
        comb_ptr: *mut u8,
        comb_len: usize,
        use_4state: bool,
    ) -> Vec<Statement> {
        let mut result = Vec::new();
        for block in &self.0 {
            match block {
                ProtoStatementBlock::Interpreted(proto) => {
                    for s in proto {
                        result.push(unsafe {
                            s.apply_values_ptr(ff_ptr, ff_len, comb_ptr, comb_len, use_4state)
                        });
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
    pub current: VarOffset,
    pub next_offset: Option<isize>,
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
    pub dynamic_select: Option<DynamicBitSelect>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: Expression,
}

#[derive(Clone, Debug)]
pub enum ProtoTbMethodKind {
    ClockNext {
        count: Option<ProtoExpression>,
        period: Option<ProtoExpression>,
    },
    ResetAssert {
        clock: StrId,
        duration: Option<ProtoExpression>,
    },
}

#[derive(Clone)]
pub enum TbMethodKind {
    ClockNext {
        count: Option<Expression>,
        period: Option<Expression>,
    },
    ResetAssert {
        clock: StrId,
        duration: Option<Expression>,
    },
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

// SAFETY: Raw pointers point into the owning Ir's exclusively-owned buffers.
// No cross-thread aliasing when each thread operates on a distinct Ir.
unsafe impl Send for Statement {}
unsafe impl Send for AssignStatement {}
unsafe impl Send for AssignDynamicStatement {}
unsafe impl Send for IfStatement {}
unsafe impl Send for SystemFunctionCall {}

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
                    output_buffer::println(&parts.join(" "));
                } else if values.is_empty() {
                    output_buffer::println(format_str);
                } else {
                    let output = format_display_string(format_str, &values);
                    output_buffer::println(&output);
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
    pub dst_base: VarOffset,
    pub dst_stride: isize,
    pub dst_num_elements: usize,
    pub dst_index_expr: ProtoExpression,
    pub dst_width: usize,
    pub select: Option<(usize, usize)>,
    pub dynamic_select: Option<ProtoDynamicBitSelect>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: ProtoExpression,
    /// Canonical (current) base byte offset for FF variables.
    pub dst_ff_current_base_offset: isize,
}

impl ProtoAssignDynamicStatement {
    #[cfg(not(target_family = "wasm"))]
    pub fn can_build_binary(&self) -> bool {
        if !self.expr.can_build_binary() || !self.dst_index_expr.can_build_binary() {
            return false;
        }
        if let Some(dyn_sel) = &self.dynamic_select {
            let full_width = dyn_sel.elem_width * dyn_sel.num_elements;
            if full_width > 128 || !dyn_sel.index_expr.can_build_binary() {
                return false;
            }
            full_width <= 64
        } else {
            self.dst_width <= 64
        }
    }

    #[cfg(not(target_family = "wasm"))]
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

        let base_addr = if self.dst_base.is_ff() {
            context.ff_values
        } else {
            context.comb_values
        };
        let static_offset = builder.ins().iconst(I64, self.dst_base.raw() as i64);
        let addr = builder.ins().iadd(base_addr, static_offset);
        let addr = builder.ins().iadd(addr, byte_offset);

        let load_mem_flag = MemFlags::trusted();
        let store_mem_flag = MemFlags::trusted();

        if let Some(dyn_sel) = &self.dynamic_select {
            let shift = build_dynamic_select_shift(dyn_sel, context, builder)?;
            let load_type = if nb == 4 { I32 } else { I64 };

            let payload = builder.ins().ishl(payload, shift);

            let elem_mask = gen_mask_for_width(dyn_sel.elem_width);
            let mask_val = builder.ins().iconst(I64, elem_mask as i64);
            let dyn_mask = builder.ins().ishl(mask_val, shift);
            let not_mask = builder.ins().bnot(dyn_mask);

            let org = builder.ins().load(load_type, load_mem_flag, addr, 0);
            let org = if nb == 4 {
                builder.ins().uextend(I64, org)
            } else {
                org
            };
            let org = builder.ins().band(org, not_mask);
            let result = builder.ins().bor(payload, org);
            if nb == 4 {
                builder.ins().istore32(store_mem_flag, result, addr, 0);
            } else {
                builder.ins().store(store_mem_flag, result, addr, 0);
            }
            if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl(mask_xz, shift);
                let org = builder.ins().load(load_type, load_mem_flag, addr, nb_i32);
                let org = if nb == 4 {
                    builder.ins().uextend(I64, org)
                } else {
                    org
                };
                let org = builder.ins().band(org, not_mask);
                let result = builder.ins().bor(mask_xz, org);
                if nb == 4 {
                    builder.ins().istore32(store_mem_flag, result, addr, nb_i32);
                } else {
                    builder.ins().store(store_mem_flag, result, addr, nb_i32);
                }
            }
        } else if let Some((beg, end)) = self.select {
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
    pub input_offsets: Vec<VarOffset>,
    pub output_offsets: Vec<VarOffset>,
    /// Canonical (current) offsets for FF variables written by this block.
    /// Used by gather_ff_canonical_offsets for dependency analysis.
    pub ff_canonical_offsets: Vec<isize>,
    /// Per-statement (inputs, outputs) from the original individual
    /// statements before JIT compilation. Used by analyze_dependency
    /// for fine-grained DAG analysis that avoids false combinational
    /// loop detection when independent signal paths are lumped into
    /// a single CompiledBlock.
    pub stmt_deps: Vec<StmtDep>,
    /// Original individual statements before JIT compilation.
    /// Used by analyze_dependency to expand CompiledBlocks that cause
    /// false cycles back into individual statements for correct ordering.
    pub original_stmts: Vec<ProtoStatement>,
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
    /// Adjust all embedded byte offsets by the given deltas.
    /// FF offsets are shifted by `ff_delta`, comb offsets by `comb_delta`.
    pub fn adjust_offsets(&mut self, ff_delta: isize, comb_delta: isize) {
        match self {
            ProtoStatement::Assign(x) => {
                x.dst = x.dst.adjust(ff_delta, comb_delta);
                if x.dst.is_ff() {
                    x.dst_ff_current_offset += ff_delta;
                }
                x.expr.adjust_offsets(ff_delta, comb_delta);
                if let Some(dyn_sel) = &mut x.dynamic_select {
                    dyn_sel.index_expr.adjust_offsets(ff_delta, comb_delta);
                }
            }
            ProtoStatement::AssignDynamic(x) => {
                x.dst_base = x.dst_base.adjust(ff_delta, comb_delta);
                if x.dst_base.is_ff() {
                    x.dst_ff_current_base_offset += ff_delta;
                }
                x.dst_index_expr.adjust_offsets(ff_delta, comb_delta);
                x.expr.adjust_offsets(ff_delta, comb_delta);
                if let Some(dyn_sel) = &mut x.dynamic_select {
                    dyn_sel.index_expr.adjust_offsets(ff_delta, comb_delta);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(cond) = &mut x.cond {
                    cond.adjust_offsets(ff_delta, comb_delta);
                }
                for s in &mut x.true_side {
                    s.adjust_offsets(ff_delta, comb_delta);
                }
                for s in &mut x.false_side {
                    s.adjust_offsets(ff_delta, comb_delta);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. } => {
                    for arg in args {
                        arg.adjust_offsets(ff_delta, comb_delta);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { elements, .. } => {
                    for elem in elements {
                        elem.current = elem.current.adjust(ff_delta, comb_delta);
                        if let Some(next) = &mut elem.next_offset {
                            *next += if elem.current.is_ff() {
                                ff_delta
                            } else {
                                comb_delta
                            };
                        }
                    }
                }
                ProtoSystemFunctionCall::Assert { condition, .. } => {
                    condition.adjust_offsets(ff_delta, comb_delta);
                }
                ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(_) => {
                // CompiledBlocks use ff_delta_bytes/comb_delta_bytes at runtime.
                // Their original_stmts should be adjusted separately if needed.
            }
            ProtoStatement::TbMethodCall { method, .. } => match method {
                ProtoTbMethodKind::ClockNext { count, period } => {
                    if let Some(c) = count {
                        c.adjust_offsets(ff_delta, comb_delta);
                    }
                    if let Some(p) = period {
                        p.adjust_offsets(ff_delta, comb_delta);
                    }
                }
                ProtoTbMethodKind::ResetAssert { duration, .. } => {
                    if let Some(d) = duration {
                        d.adjust_offsets(ff_delta, comb_delta);
                    }
                }
            },
        }
    }

    #[cfg(not(target_family = "wasm"))]
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

    pub fn token(&self) -> Option<TokenRange> {
        match self {
            ProtoStatement::Assign(x) => Some(x.token),
            _ => None,
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
        inputs: &mut Vec<VarOffset>,
        outputs: &mut Vec<VarOffset>,
    ) {
        match self {
            ProtoStatement::Assign(x) => {
                x.expr.gather_variable_offsets(inputs);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets(inputs);
                }
                outputs.push(x.dst);
            }
            ProtoStatement::AssignDynamic(x) => {
                x.dst_index_expr.gather_variable_offsets(inputs);
                x.expr.gather_variable_offsets(inputs);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets(inputs);
                }
                // Emit only base + last offset to represent the entire array as
                // a single dependency unit.  Per-element expansion caused O(N²)
                // blowup in analyze_dependency for large arrays.
                outputs.push(x.dst_base);
                if x.dst_num_elements > 1 {
                    let last_offset = VarOffset::new(
                        x.dst_base.is_ff(),
                        x.dst_base.raw() + x.dst_stride * (x.dst_num_elements as isize - 1),
                    );
                    outputs.push(last_offset);
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
                // Only include comb (non-FF) offsets for dependency analysis.
                // FF reads/writes go through the current/next buffer swap and
                // don't participate in combinational dependency chains.
                // Including FF offsets creates false CombinationalLoop errors
                // in analyze_dependency (e.g., hazard_unit stall → pipeline FF
                // → hazard_unit input appears as a comb cycle).
                if !x.stmt_deps.is_empty() {
                    // Use fine-grained per-statement deps if available
                    for (ins, outs) in &x.stmt_deps {
                        for &off in ins {
                            if !off.is_ff() {
                                inputs.push(VarOffset::Comb(off.raw()));
                            }
                        }
                        for &off in outs {
                            if !off.is_ff() {
                                outputs.push(VarOffset::Comb(off.raw()));
                            }
                        }
                    }
                } else {
                    for &off in &x.input_offsets {
                        if !off.is_ff() {
                            inputs.push(VarOffset::Comb(off.raw()));
                        }
                    }
                    for &off in &x.output_offsets {
                        if !off.is_ff() {
                            outputs.push(VarOffset::Comb(off.raw()));
                        }
                    }
                }
            }
            ProtoStatement::TbMethodCall { .. } => {}
        }
    }

    /// Returns the set of canonical (current) FF byte offsets written by this statement.
    pub fn gather_ff_canonical_offsets(&self) -> HashSet<isize> {
        let mut result = HashSet::default();
        match self {
            ProtoStatement::Assign(x) => {
                if x.dst.is_ff() {
                    result.insert(x.dst_ff_current_offset);
                }
            }
            ProtoStatement::AssignDynamic(x) => {
                if x.dst_base.is_ff() {
                    // Emit only base + last canonical offset to represent the
                    // array.  Per-element expansion caused O(N) blowup for
                    // large arrays.
                    result.insert(x.dst_ff_current_base_offset);
                    if x.dst_num_elements > 1 {
                        let last = x.dst_ff_current_base_offset
                            + x.dst_stride * (x.dst_num_elements as isize - 1);
                        result.insert(last);
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
                for off in &x.ff_canonical_offsets {
                    result.insert(*off);
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
        ff_len: usize,
        comb_values_ptr: *mut u8,
        comb_len: usize,
        use_4state: bool,
    ) -> Statement {
        unsafe {
            match self {
                ProtoStatement::Assign(x) => Statement::Assign(x.apply_values_ptr(
                    ff_values_ptr,
                    ff_len,
                    comb_values_ptr,
                    comb_len,
                    use_4state,
                )),
                ProtoStatement::AssignDynamic(x) => {
                    let nb = calc_native_bytes(x.dst_width);
                    let dst_base_ptr = if x.dst_base.is_ff() {
                        ff_values_ptr.offset(x.dst_base.raw())
                    } else {
                        comb_values_ptr.offset(x.dst_base.raw())
                    };
                    let dst_index_expr = x.dst_index_expr.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
                    let expr = x.expr.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    );
                    let dynamic_select =
                        x.dynamic_select.as_ref().map(|dyn_sel| DynamicBitSelect {
                            index_expr: Box::new(dyn_sel.index_expr.apply_values_ptr(
                                ff_values_ptr,
                                ff_len,
                                comb_values_ptr,
                                comb_len,
                                use_4state,
                            )),
                            elem_width: dyn_sel.elem_width,
                            num_elements: dyn_sel.num_elements,
                        });
                    Statement::AssignDynamic(AssignDynamicStatement {
                        dst_base_ptr,
                        dst_stride: x.dst_stride,
                        dst_num_elements: x.dst_num_elements,
                        dst_index_expr,
                        dst_width: x.dst_width,
                        dst_native_bytes: nb,
                        dst_use_4state: use_4state,
                        select: x.select,
                        dynamic_select,
                        rhs_select: x.rhs_select,
                        expr,
                    })
                }
                ProtoStatement::If(x) => Statement::If(x.apply_values_ptr(
                    ff_values_ptr,
                    ff_len,
                    comb_values_ptr,
                    comb_len,
                    use_4state,
                )),
                ProtoStatement::SystemFunctionCall(x) => match x {
                    ProtoSystemFunctionCall::Display { format_str, args } => {
                        let args = args
                            .iter()
                            .map(|a| {
                                a.apply_values_ptr(
                                    ff_values_ptr,
                                    ff_len,
                                    comb_values_ptr,
                                    comb_len,
                                    use_4state,
                                )
                            })
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
                                let current = if elem.current.is_ff() {
                                    ff_values_ptr.offset(elem.current.raw())
                                } else {
                                    comb_values_ptr.offset(elem.current.raw())
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
                        let condition = condition.apply_values_ptr(
                            ff_values_ptr,
                            ff_len,
                            comb_values_ptr,
                            comb_len,
                            use_4state,
                        );
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
                    // Use wrapping_offset because the adjusted pointer may temporarily
                    // go before the buffer start when ff_delta_bytes is negative.
                    // This is safe because the JIT code's actual memory accesses always
                    // land within the buffer (adjusted_ptr + original_offset is in bounds).
                    let adjusted_ff =
                        (ff_values_ptr as *const u8).wrapping_offset(x.ff_delta_bytes);
                    let adjusted_comb =
                        (comb_values_ptr as *const u8).wrapping_offset(x.comb_delta_bytes);
                    Statement::Binary(x.func, adjusted_ff, adjusted_comb)
                }
                ProtoStatement::TbMethodCall { inst, method } => {
                    let method = match method {
                        ProtoTbMethodKind::ClockNext { count, period } => {
                            let count = count.as_ref().map(|e| {
                                e.apply_values_ptr(
                                    ff_values_ptr,
                                    ff_len,
                                    comb_values_ptr,
                                    comb_len,
                                    use_4state,
                                )
                            });
                            let period = period.as_ref().map(|e| {
                                e.apply_values_ptr(
                                    ff_values_ptr,
                                    ff_len,
                                    comb_values_ptr,
                                    comb_len,
                                    use_4state,
                                )
                            });
                            TbMethodKind::ClockNext { count, period }
                        }
                        ProtoTbMethodKind::ResetAssert { clock, duration } => {
                            let duration = duration.as_ref().map(|e| {
                                e.apply_values_ptr(
                                    ff_values_ptr,
                                    ff_len,
                                    comb_values_ptr,
                                    comb_len,
                                    use_4state,
                                )
                            });
                            TbMethodKind::ResetAssert {
                                clock: *clock,
                                duration,
                            }
                        }
                    };
                    Statement::TbMethodCall {
                        inst: *inst,
                        method,
                    }
                }
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
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
    pub dynamic_select: Option<DynamicBitSelect>,
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
        if let Some(dyn_sel) = &self.dynamic_select {
            let idx = dyn_sel
                .index_expr
                .eval(mask_cache)
                .to_usize()
                .unwrap_or(0)
                .min(dyn_sel.num_elements.saturating_sub(1));
            let end = idx * dyn_sel.elem_width;
            let beg = end + dyn_sel.elem_width - 1;
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
        } else if let Some((beg, end)) = self.select {
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
        if let Some(dyn_sel) = &self.dynamic_select {
            dyn_sel.index_expr.gather_variable(inputs, &mut vec![]);
        }
        outputs.push(self.dst);
    }
}

impl AssignDynamicStatement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        if self.dst_num_elements == 0 {
            return;
        }
        let idx_val = self.dst_index_expr.eval(mask_cache);
        let idx = idx_val
            .to_usize()
            .unwrap_or(0)
            .min(self.dst_num_elements.saturating_sub(1));
        let dst = unsafe { self.dst_base_ptr.offset(self.dst_stride * idx as isize) };

        let value = self.expr.eval(mask_cache);
        let value = if let Some((beg, end)) = self.rhs_select {
            value.select(beg, end)
        } else {
            value
        };
        if let Some(dyn_sel) = &self.dynamic_select {
            let dyn_idx = dyn_sel
                .index_expr
                .eval(mask_cache)
                .to_usize()
                .unwrap_or(0)
                .min(dyn_sel.num_elements.saturating_sub(1));
            let end = dyn_idx * dyn_sel.elem_width;
            let beg = end + dyn_sel.elem_width - 1;
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
        } else if let Some((beg, end)) = self.select {
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
        if let Some(dyn_sel) = &self.dynamic_select {
            dyn_sel.index_expr.gather_variable(inputs, &mut vec![]);
        }
        for i in 0..self.dst_num_elements {
            let ptr =
                unsafe { self.dst_base_ptr.offset(self.dst_stride * i as isize) as *const u8 };
            outputs.push(ptr);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProtoAssignStatement {
    pub dst: VarOffset,
    pub dst_width: usize,
    pub select: Option<(usize, usize)>,
    pub dynamic_select: Option<ProtoDynamicBitSelect>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: ProtoExpression,
    /// Canonical (current) byte offset for FF variables.
    /// Used by append_ff_next_copies to compute the next offset and
    /// by gather_ff_canonical_offsets for dependency analysis.
    pub dst_ff_current_offset: isize,
    /// Source location from the original assign statement.
    pub token: TokenRange,
}

impl ProtoAssignStatement {
    #[cfg(not(target_family = "wasm"))]
    pub fn can_build_binary(&self) -> bool {
        if !self.expr.can_build_binary() {
            return false;
        }
        if let Some(dyn_sel) = &self.dynamic_select
            && (dyn_sel.elem_width * dyn_sel.num_elements > 128
                || !dyn_sel.index_expr.can_build_binary())
        {
            return false;
        }
        true
    }

    /// # Safety
    /// `ff_values_ptr` and `comb_values_ptr` must point to valid buffers.
    pub unsafe fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut u8,
        ff_len: usize,
        comb_values_ptr: *mut u8,
        comb_len: usize,
        use_4state: bool,
    ) -> AssignStatement {
        unsafe {
            let nb = calc_native_bytes(self.dst_width);
            let _vs = if use_4state { nb * 2 } else { nb };
            let dst = if self.dst.is_ff() {
                #[cfg(debug_assertions)]
                debug_assert!(
                    (self.dst.raw() as usize) + _vs <= ff_len,
                    "apply_values_ptr Assign: ff dst {} + vs {} > ff_len {} (width={})",
                    self.dst.raw(),
                    _vs,
                    ff_len,
                    self.dst_width,
                );
                ff_values_ptr.add(self.dst.raw() as usize)
            } else {
                #[cfg(debug_assertions)]
                debug_assert!(
                    (self.dst.raw() as usize) + _vs <= comb_len,
                    "apply_values_ptr Assign: comb dst {} + vs {} > comb_len {} (width={})",
                    self.dst.raw(),
                    _vs,
                    comb_len,
                    self.dst_width,
                );
                comb_values_ptr.add(self.dst.raw() as usize)
            };

            let expr = self.expr.apply_values_ptr(
                ff_values_ptr,
                ff_len,
                comb_values_ptr,
                comb_len,
                use_4state,
            );

            let dynamic_select = self
                .dynamic_select
                .as_ref()
                .map(|dyn_sel| DynamicBitSelect {
                    index_expr: Box::new(dyn_sel.index_expr.apply_values_ptr(
                        ff_values_ptr,
                        ff_len,
                        comb_values_ptr,
                        comb_len,
                        use_4state,
                    )),
                    elem_width: dyn_sel.elem_width,
                    num_elements: dyn_sel.num_elements,
                });

            AssignStatement {
                dst,
                dst_width: self.dst_width,
                dst_native_bytes: nb,
                dst_use_4state: use_4state,
                select: self.select,
                dynamic_select,
                rhs_select: self.rhs_select,
                expr,
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
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

        let load_mem_flag = MemFlags::trusted();
        let store_mem_flag = MemFlags::trusted();

        let base_addr = if self.dst.is_ff() {
            context.ff_values
        } else {
            context.comb_values
        };

        let dst_offset = self.dst.raw() as i32;
        let cache_key = self.dst;

        if let Some(dyn_sel) = &self.dynamic_select {
            let shift = build_dynamic_select_shift(dyn_sel, context, builder)?;

            let payload = builder.ins().ishl(payload, shift);

            // Dynamic mask: elem_mask << shift, then invert
            let elem_mask = gen_mask_for_width(dyn_sel.elem_width);
            let mask_val = if wide {
                iconst_128(builder, elem_mask)
            } else {
                builder.ins().iconst(I64, elem_mask as i64)
            };
            let dyn_mask = builder.ins().ishl(mask_val, shift);
            let not_mask = builder.ins().bnot(dyn_mask);

            let load_type = if nb == 16 {
                I128
            } else if nb == 4 {
                I32
            } else {
                I64
            };

            let (org_payload, org_mask_xz) = if !context.disable_load_cache
                && let Some(&(cached_p, cached_m)) = context.load_cache.get(&cache_key)
            {
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
                let mask_xz = builder.ins().ishl(mask_xz, shift);
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

            context
                .load_cache
                .insert(cache_key, (result, result_mask_xz));
        } else if let Some((beg, end)) = self.select {
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
            let (org_payload, org_mask_xz) = if !context.disable_load_cache
                && let Some(&(cached_p, cached_m)) = context.load_cache.get(&cache_key)
            {
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
    #[cfg(not(target_family = "wasm"))]
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

        let base_addr = if self.dst.is_ff() {
            context.ff_values
        } else {
            context.comb_values
        };
        let dst_offset = self.dst.raw() as i32;

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
    #[cfg(not(target_family = "wasm"))]
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
        ff_len: usize,
        comb_values_ptr: *mut u8,
        comb_len: usize,
        use_4state: bool,
    ) -> IfStatement {
        unsafe {
            let cond = self.cond.as_ref().map(|x| {
                x.apply_values_ptr(ff_values_ptr, ff_len, comb_values_ptr, comb_len, use_4state)
            });
            let true_side: Vec<_> = self
                .true_side
                .iter()
                .map(|x| {
                    x.apply_values_ptr(ff_values_ptr, ff_len, comb_values_ptr, comb_len, use_4state)
                })
                .collect();
            let false_side: Vec<_> = self
                .false_side
                .iter()
                .map(|x| {
                    x.apply_values_ptr(ff_values_ptr, ff_len, comb_values_ptr, comb_len, use_4state)
                })
                .collect();

            IfStatement {
                cond,
                true_side,
                false_side,
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
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
            let proto: ProtoExpression = Conv::conv(context, &first.0).ok()?;
            exprs.push(proto);
        }
    }

    for input in iter {
        let proto: ProtoExpression = Conv::conv(context, &input.0).ok()?;
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
        return veryl_analyzer::value::byte_value_to_string(value);
    }
    None
}

impl Conv<&air::Statement> for Vec<ProtoStatement> {
    fn conv(context: &mut ConvContext, src: &air::Statement) -> Result<Self, SimulatorError> {
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
                    let (format_str, exprs) = extract_display_args(context, inputs).unwrap();
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Display {
                            format_str,
                            args: exprs,
                        },
                    )]
                }
                SystemFunctionKind::Readmemh(input, output) => {
                    let raw = extract_string_value(&input.0).unwrap();
                    let filename = raw.trim_matches('"').to_string();
                    let dst = &output.0[0];
                    let id = dst.id;
                    let scope = context.scope();
                    let meta = scope.variable_meta.get(&id).unwrap();
                    let width = meta.width;
                    let elements: Vec<ReadmemhElement> = meta
                        .elements
                        .iter()
                        .map(|elem| ReadmemhElement {
                            current: elem.current,
                            next_offset: if elem.is_ff() {
                                Some(elem.next_offset)
                            } else {
                                None
                            },
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
                _ => panic!("unhandled SystemFunctionKind"),
            },
            air::Statement::TbMethodCall(x) => {
                let method = match &x.method {
                    air::TbMethod::ClockNext { count, period } => {
                        let count = if let Some(expr) = count {
                            Some(Conv::conv(context, expr)?)
                        } else {
                            None
                        };
                        let period = if let Some(expr) = period {
                            Some(Conv::conv(context, expr)?)
                        } else {
                            None
                        };
                        ProtoTbMethodKind::ClockNext { count, period }
                    }
                    air::TbMethod::ResetAssert { clock, duration } => {
                        let duration = if let Some(expr) = duration {
                            Some(Conv::conv(context, expr)?)
                        } else {
                            None
                        };
                        ProtoTbMethodKind::ResetAssert {
                            clock: *clock,
                            duration,
                        }
                    }
                };
                vec![ProtoStatement::TbMethodCall {
                    inst: x.inst,
                    method,
                }]
            }
            air::Statement::Unsupported(token) => {
                return Err(SimulatorError::unsupported_description(token));
            }
            air::Statement::Null => vec![],
        };

        // Drain pending statements from function calls within expressions
        let mut pending = std::mem::take(&mut context.pending_statements);
        if !pending.is_empty() {
            pending.append(&mut result);
            result = pending;
        }

        Ok(result)
    }
}

impl Conv<&air::AssignStatement> for Vec<ProtoStatement> {
    fn conv(context: &mut ConvContext, src: &air::AssignStatement) -> Result<Self, SimulatorError> {
        let in_initial = context.in_initial;
        if matches!(src.expr, air::Expression::ArrayLiteral(..)) {
            let dst = &src.dst[0];
            let scope = context.scope();
            let meta = scope.variable_meta.get(&dst.id).unwrap();
            let dst_type = meta.r#type.clone();
            let mut expr_clone = src.expr.clone();

            let array_exprs = eval_array_literal(
                &mut scope.analyzer_context,
                Some(&dst_type.array),
                Some(&dst_type.width),
                &mut expr_clone,
            )
            .unwrap()
            .unwrap();

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
            if in_initial {
                append_ff_next_copies(&mut result);
            }
            return Ok(result);
        }

        if src.dst.len() <= 1 {
            let stmt: ProtoStatement = Conv::conv(context, src)?;
            let mut result = vec![stmt];
            if in_initial {
                append_ff_next_copies(&mut result);
            }
            return Ok(result);
        }

        let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

        let mut result = Vec::new();
        let mut remaining = src.width.unwrap();

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
                dst.comptime.r#type.total_width().unwrap()
            };

            let rhs_select = Some((remaining - 1, remaining - dst_elem_width));
            remaining -= dst_elem_width;

            let const_index = if dst.index.is_const() {
                dst.index.eval_value(&mut scope.analyzer_context)
            } else {
                None
            };

            if let Some(idx_vals) = const_index {
                let index = meta.r#type.array.calc_index(&idx_vals).unwrap();
                let element = &meta.elements[index];
                let is_ff = element.is_ff();
                let dst_width = meta.width;
                // FF assignment writes to next, but in initial block writes to current
                let dst_var = if is_ff {
                    if in_initial {
                        VarOffset::Ff(element.current_offset())
                    } else {
                        VarOffset::Ff(element.next_offset)
                    }
                } else {
                    VarOffset::Comb(element.current_offset())
                };

                result.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst: dst_var,
                    dst_width,
                    select,
                    dynamic_select: None,
                    rhs_select,
                    expr: expr.clone(),
                    dst_ff_current_offset: element.current_offset(),
                    token: src.token,
                }));
            } else {
                let array_shape = meta.r#type.array.clone();
                let dyn_info = meta.dynamic_index_info().unwrap();
                let num_elements = meta.elements.len();
                let (base_current, base_next, stride, is_ff) = dyn_info;
                // FF assignment writes to next, but in initial block writes to current
                let dst_base = if is_ff {
                    if in_initial {
                        VarOffset::Ff(base_current)
                    } else {
                        VarOffset::Ff(base_next)
                    }
                } else {
                    VarOffset::Comb(base_current)
                };
                let dst_width = meta.width;

                let index_proto = build_linear_index_expr(context, &array_shape, &dst.index)?;

                result.push(ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
                    dst_base,
                    dst_stride: stride,
                    dst_num_elements: num_elements,
                    dst_index_expr: index_proto,
                    dst_width,
                    select,
                    dynamic_select: None,
                    rhs_select,
                    expr: expr.clone(),
                    dst_ff_current_base_offset: base_current,
                }));
            }
        }

        if in_initial {
            append_ff_next_copies(&mut result);
        }
        Ok(result)
    }
}

/// For initial block assignments to FF variables, append duplicate statements
/// that also write to the `next` offset. This ensures the value persists across
/// ff_swap (same pattern as $readmemh dual write).
fn append_ff_next_copies(stmts: &mut Vec<ProtoStatement>) {
    let mut extras = Vec::new();
    for s in stmts.iter() {
        match s {
            ProtoStatement::Assign(a) if a.dst.is_ff() => {
                // FF layout: [current][next] contiguously, each calc_native_bytes wide.
                // next_offset = current_offset + native_bytes.
                let nb = calc_native_bytes(a.dst_width) as isize;
                let next_offset = a.dst_ff_current_offset + nb;
                extras.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst: VarOffset::Ff(next_offset),
                    ..a.clone()
                }));
            }
            ProtoStatement::AssignDynamic(a) if a.dst_base.is_ff() => {
                // For dynamic index FF: base_next = base_current + native_bytes
                let nb = calc_native_bytes(a.dst_width) as isize;
                let next_base = a.dst_ff_current_base_offset + nb;
                extras.push(ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
                    dst_base: VarOffset::Ff(next_base),
                    ..a.clone()
                }));
            }
            _ => {}
        }
    }
    stmts.extend(extras);
}

impl Conv<&air::AssignStatement> for ProtoStatement {
    fn conv(context: &mut ConvContext, src: &air::AssignStatement) -> Result<Self, SimulatorError> {
        // TODO multiple dst
        let dst = &src.dst[0];
        let id = dst.id;
        let in_initial = context.in_initial;

        let (select, dst_width, const_index, need_dynamic_select, width_shape, kind_width) = {
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
            let need_dynamic = !dst.select.is_empty() && !dst.select.is_const();
            let select = if need_dynamic { None } else { select };
            let width_shape = meta.r#type.width.clone();
            let kind_width = meta.r#type.kind.width().unwrap_or(1);
            (
                select,
                dst_width,
                const_index,
                need_dynamic,
                width_shape,
                kind_width,
            )
        };

        let dynamic_select = if need_dynamic_select {
            Some(build_dynamic_bit_select(
                context,
                &width_shape,
                &dst.select,
                kind_width,
            )?)
        } else {
            None
        };

        if let Some(idx_vals) = const_index {
            let scope = context.scope();
            let meta = scope.variable_meta.get(&id).unwrap();
            let index = meta.r#type.array.calc_index(&idx_vals).unwrap();
            let element = &meta.elements[index];
            let is_ff = element.is_ff();
            let current_offset = element.current_offset();
            // FF assignment writes to next, but in initial block writes to current
            let dst = if is_ff {
                if in_initial {
                    VarOffset::Ff(current_offset)
                } else {
                    VarOffset::Ff(element.next_offset)
                }
            } else {
                VarOffset::Comb(current_offset)
            };

            let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

            Ok(ProtoStatement::Assign(ProtoAssignStatement {
                dst,
                dst_width,
                select,
                dynamic_select,
                rhs_select: None,
                expr,
                dst_ff_current_offset: current_offset,
                token: src.token,
            }))
        } else {
            // Dynamic index
            let scope = context.scope();
            let meta = scope.variable_meta.get(&id).unwrap();
            let array_shape = meta.r#type.array.clone();
            let dyn_info = meta.dynamic_index_info().unwrap();
            let num_elements = meta.elements.len();
            let (base_current, base_next, stride, is_ff) = dyn_info;
            // FF assignment writes to next, but in initial block writes to current
            let dst_base = if is_ff {
                if in_initial {
                    VarOffset::Ff(base_current)
                } else {
                    VarOffset::Ff(base_next)
                }
            } else {
                VarOffset::Comb(base_current)
            };

            let index_proto = build_linear_index_expr(context, &array_shape, &dst.index)?;
            let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

            Ok(ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
                dst_base,
                dst_stride: stride,
                dst_num_elements: num_elements,
                dst_index_expr: index_proto,
                dst_width,
                select,
                dynamic_select,
                rhs_select: None,
                expr,
                dst_ff_current_base_offset: base_current,
            }))
        }
    }
}

impl Conv<&air::AssignStatement> for ProtoAssignStatement {
    fn conv(context: &mut ConvContext, src: &air::AssignStatement) -> Result<Self, SimulatorError> {
        let in_initial = context.in_initial;

        // TODO multiple dst
        let dst = &src.dst[0];
        let id = dst.id;

        let (
            _index,
            select,
            is_ff,
            current_offset,
            next_offset,
            dst_width,
            need_dynamic_select,
            width_shape,
            kind_width,
        ) = {
            let scope = context.scope();
            let meta = scope.variable_meta.get(&id).unwrap();

            let index = dst.index.eval_value(&mut scope.analyzer_context).unwrap();
            let index = meta.r#type.array.calc_index(&index).unwrap();

            let select = if !dst.select.is_empty() {
                dst.select
                    .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
            } else {
                None
            };

            let element = &meta.elements[index];
            let is_ff = element.is_ff();
            let current_offset = element.current_offset();
            let next_offset = element.next_offset;
            let dst_width = meta.width;
            let need_dynamic = !dst.select.is_empty() && !dst.select.is_const();
            let select = if need_dynamic { None } else { select };
            let width_shape = meta.r#type.width.clone();
            let kind_width = meta.r#type.kind.width().unwrap_or(1);
            (
                index,
                select,
                is_ff,
                current_offset,
                next_offset,
                dst_width,
                need_dynamic,
                width_shape,
                kind_width,
            )
        };

        let dynamic_select = if need_dynamic_select {
            Some(build_dynamic_bit_select(
                context,
                &width_shape,
                &dst.select,
                kind_width,
            )?)
        } else {
            None
        };

        // FF assignment writes to next, but in initial block writes to current
        let dst_var = if is_ff {
            if in_initial {
                VarOffset::Ff(current_offset)
            } else {
                VarOffset::Ff(next_offset)
            }
        } else {
            VarOffset::Comb(current_offset)
        };

        let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

        Ok(ProtoAssignStatement {
            dst: dst_var,
            dst_width,
            select,
            dynamic_select,
            rhs_select: None,
            expr,
            dst_ff_current_offset: current_offset,
            token: src.token,
        })
    }
}

impl Conv<&air::IfStatement> for ProtoIfStatement {
    fn conv(context: &mut ConvContext, src: &air::IfStatement) -> Result<Self, SimulatorError> {
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

        Ok(ProtoIfStatement {
            cond: Some(cond),
            true_side,
            false_side,
        })
    }
}

impl Conv<&air::IfResetStatement> for ProtoIfStatement {
    fn conv(
        context: &mut ConvContext,
        src: &air::IfResetStatement,
    ) -> Result<Self, SimulatorError> {
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

        Ok(ProtoIfStatement {
            cond: None,
            true_side,
            false_side,
        })
    }
}

impl Conv<&FunctionCall> for Vec<ProtoStatement> {
    fn conv(context: &mut ConvContext, src: &FunctionCall) -> Result<Self, SimulatorError> {
        if !context.expanding_functions.insert(src.id) {
            let name = context
                .scope()
                .analyzer_context
                .functions
                .get(&src.id)
                .unwrap()
                .name
                .to_string();
            return Err(SimulatorError::recursive_function(
                &name,
                &src.comptime.token,
            ));
        }

        let mut result = Vec::new();

        // Clone to avoid borrow conflict with context
        let func = context
            .scope()
            .analyzer_context
            .functions
            .get(&src.id)
            .unwrap()
            .clone();
        let body = if let Some(ref idx) = src.index {
            func.get_function(idx).unwrap()
        } else {
            func.get_function(&[]).unwrap()
        };

        for (var_path, expr) in &src.inputs {
            let arg_var_id = body.arg_map.get(var_path).unwrap();
            let proto_expr: ProtoExpression = Conv::conv(context, expr)?;
            let scope = context.scope();
            let meta = scope.variable_meta.get(arg_var_id).unwrap();
            let element = &meta.elements[0];
            result.push(ProtoStatement::Assign(ProtoAssignStatement {
                dst: element.current,
                dst_width: meta.width,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: proto_expr,
                dst_ff_current_offset: 0, // not FF
                token: TokenRange::default(),
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
            let arg_var_id = body.arg_map.get(var_path).unwrap();
            let scope = context.scope();
            let arg_meta = scope.variable_meta.get(arg_var_id).unwrap();
            let arg_element = &arg_meta.elements[0];
            let arg_expr = ProtoExpression::Variable {
                var_offset: arg_element.current,
                select: None,
                dynamic_select: None,
                width: arg_meta.width,
                expr_context: ExpressionContext {
                    width: arg_meta.width,
                    signed: false,
                },
            };
            for dst in destinations {
                let scope = context.scope();
                let dst_meta = scope.variable_meta.get(&dst.id).unwrap();
                let dst_index = dst.index.eval_value(&mut scope.analyzer_context).unwrap();
                let dst_index = dst_meta.r#type.array.calc_index(&dst_index).unwrap();
                let dst_element = &dst_meta.elements[dst_index];

                let select = if !dst.select.is_empty() {
                    dst.select
                        .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
                } else {
                    None
                };

                let dst_var = if dst_element.is_ff() {
                    VarOffset::Ff(dst_element.next_offset)
                } else {
                    VarOffset::Comb(dst_element.current_offset())
                };

                result.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst: dst_var,
                    dst_width: dst_meta.width,
                    select,
                    dynamic_select: None,
                    rhs_select: None,
                    expr: arg_expr.clone(),
                    dst_ff_current_offset: dst_element.current_offset(),
                    token: TokenRange::default(),
                }));
            }
        }

        context.expanding_functions.remove(&src.id);
        Ok(result)
    }
}

fn parse_hex_file(filename: &str, width: usize) -> Vec<veryl_analyzer::value::Value> {
    let content = match std::fs::read_to_string(filename) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("$readmemh: failed to read '{}': {}", filename, e);
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
