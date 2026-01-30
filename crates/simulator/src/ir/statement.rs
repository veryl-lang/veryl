use crate::cranelift::Context as CraneliftContext;
use crate::cranelift::FuncPtr;
use crate::ir::context::{Context as ConvContext, Conv};
use crate::ir::expression::ExpressionContext;
use crate::ir::expression::build_linear_index_expr;
use crate::ir::{CombValue, Expression, FfValue, ProtoExpression, Value};
use cranelift::prelude::types::I64;
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::conv::utils::eval_array_literal;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::FunctionCall;
use veryl_analyzer::ir::{SystemFunctionInput, SystemFunctionKind, TypeKind, ValueVariant};
use veryl_analyzer::value::{MaskCache, ValueU64, value_u64_offset};

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
        ff_ptr: *mut FfValue,
        comb_ptr: *mut CombValue,
    ) -> Vec<Statement> {
        let mut result = Vec::new();
        for block in &self.0 {
            match block {
                ProtoStatementBlock::Interpreted(proto) => {
                    for s in proto {
                        result.push(s.apply_values_ptr(ff_ptr, comb_ptr));
                    }
                }
                ProtoStatementBlock::Compiled(func) => {
                    result.push(Statement::Binary(
                        *func,
                        ff_ptr as *const FfValue,
                        comb_ptr as *const CombValue,
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
        elements: Vec<(*mut Value, Option<*mut Value>)>,
        width: usize,
    },
}

#[derive(Clone)]
pub struct AssignDynamicStatement {
    pub dst_base_ptr: *mut Value,
    pub dst_stride: isize,
    pub dst_num_elements: usize,
    pub dst_index_expr: Expression,
    pub select: Option<(usize, usize)>,
    pub rhs_select: Option<(usize, usize)>,
    pub expr: Expression,
}

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    AssignDynamic(AssignDynamicStatement),
    If(IfStatement),
    Binary(FuncPtr, *const FfValue, *const CombValue),
    SystemFunctionCall(SystemFunctionCall),
}

impl Statement {
    pub fn is_binary(&self) -> bool {
        matches!(self, Statement::Binary(_, _, _))
    }

    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        match self {
            Statement::Assign(x) => x.eval_step(mask_cache),
            Statement::AssignDynamic(x) => x.eval_step(mask_cache),
            Statement::If(x) => x.eval_step(mask_cache),
            Statement::Binary(func, ff_values, comb_values) => unsafe {
                func(*ff_values, *comb_values);
            },
            Statement::SystemFunctionCall(x) => x.eval_step(mask_cache),
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const Value>, outputs: &mut Vec<*const Value>) {
        match self {
            Statement::Assign(x) => x.gather_variable(inputs, outputs),
            Statement::AssignDynamic(x) => x.gather_variable(inputs, outputs),
            Statement::If(x) => x.gather_variable(inputs, outputs),
            Statement::Binary(_, _, _) => (),
            Statement::SystemFunctionCall(x) => x.gather_variable(inputs),
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
                    let (current, next) = elements[i];
                    unsafe {
                        (*current).set_value(values[i].clone());
                        if let Some(next) = next {
                            (*next).set_value(values[i].clone());
                        }
                    }
                }
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const Value>) {
        match self {
            SystemFunctionCall::Display { args, .. } => {
                for e in args {
                    let mut dummy_outputs = vec![];
                    e.gather_variable(inputs, &mut dummy_outputs);
                }
            }
            SystemFunctionCall::Readmemh { .. } => {}
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
        let static_offset = builder
            .ins()
            .iconst(I64, (self.dst_base_offset + value_u64_offset()) as i64);
        let addr = builder.ins().iadd(base_addr, static_offset);
        let addr = builder.ins().iadd(addr, byte_offset);

        let load_mem_flag = MemFlags::trusted().with_readonly();
        let store_mem_flag = MemFlags::trusted();

        if let Some((beg, end)) = self.select {
            let mask = ValueU64::gen_mask_range(beg, end);

            let payload = builder.ins().ishl_imm(payload, end as i64);
            let org = builder.ins().load(I64, load_mem_flag, addr, 0);
            let org = builder.ins().band_imm(org, !mask as i64);
            let payload = builder.ins().bor(payload, org);

            builder.ins().store(store_mem_flag, payload, addr, 0);
            if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm(mask_xz, end as i64);
                let org = builder.ins().load(I64, load_mem_flag, addr, 8);
                let org = builder.ins().band_imm(org, !mask as i64);
                let mask_xz = builder.ins().bor(mask_xz, org);

                builder.ins().store(store_mem_flag, mask_xz, addr, 8);
            }
        } else {
            match self.dst_width {
                8 => {
                    builder.ins().istore8(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder.ins().istore8(store_mem_flag, mask_xz, addr, 8);
                    }
                }
                16 => {
                    builder.ins().istore16(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder.ins().istore16(store_mem_flag, mask_xz, addr, 8);
                    }
                }
                32 => {
                    builder.ins().istore32(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder.ins().istore32(store_mem_flag, mask_xz, addr, 8);
                    }
                }
                64 => {
                    builder.ins().store(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        builder.ins().store(store_mem_flag, mask_xz, addr, 8);
                    }
                }
                _ => {
                    if self.dst_width >= 64 {
                        return None;
                    }
                    let mask = (1u64 << self.dst_width) - 1;
                    let payload = builder.ins().band_imm(payload, mask as i64);

                    builder.ins().store(store_mem_flag, payload, addr, 0);
                    if let Some(mask_xz) = mask_xz {
                        let mask_xz = builder.ins().band_imm(mask_xz, mask as i64);
                        builder.ins().store(store_mem_flag, mask_xz, addr, 8);
                    }
                }
            }
        }

        Some(())
    }
}

#[derive(Clone, Debug)]
pub enum ProtoStatement {
    Assign(ProtoAssignStatement),
    AssignDynamic(ProtoAssignDynamicStatement),
    If(ProtoIfStatement),
    SystemFunctionCall(ProtoSystemFunctionCall),
}

impl ProtoStatement {
    pub fn can_build_binary(&self) -> bool {
        match self {
            ProtoStatement::Assign(x) => x.can_build_binary(),
            ProtoStatement::AssignDynamic(x) => x.can_build_binary(),
            ProtoStatement::If(x) => x.can_build_binary(),
            ProtoStatement::SystemFunctionCall(_) => false,
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
            },
        }
    }

    pub fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut FfValue,
        comb_values_ptr: *mut CombValue,
    ) -> Statement {
        match self {
            ProtoStatement::Assign(x) => {
                Statement::Assign(x.apply_values_ptr(ff_values_ptr, comb_values_ptr))
            }
            ProtoStatement::AssignDynamic(x) => {
                let dst_base_ptr = if x.dst_is_ff {
                    unsafe { (ff_values_ptr as *mut u8).offset(x.dst_base_offset) as *mut Value }
                } else {
                    unsafe { (comb_values_ptr as *mut u8).offset(x.dst_base_offset) as *mut Value }
                };
                let dst_index_expr = x
                    .dst_index_expr
                    .apply_values_ptr(ff_values_ptr, comb_values_ptr);
                let expr = x.expr.apply_values_ptr(ff_values_ptr, comb_values_ptr);
                Statement::AssignDynamic(AssignDynamicStatement {
                    dst_base_ptr,
                    dst_stride: x.dst_stride,
                    dst_num_elements: x.dst_num_elements,
                    dst_index_expr,
                    select: x.select,
                    rhs_select: x.rhs_select,
                    expr,
                })
            }
            ProtoStatement::If(x) => {
                Statement::If(x.apply_values_ptr(ff_values_ptr, comb_values_ptr))
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { format_str, args } => {
                    let args = args
                        .iter()
                        .map(|a| a.apply_values_ptr(ff_values_ptr, comb_values_ptr))
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
                    let resolved: Vec<_> = elements
                        .iter()
                        .map(|elem| {
                            let current = if elem.is_ff {
                                unsafe {
                                    (ff_values_ptr as *mut u8).offset(elem.current_offset)
                                        as *mut Value
                                }
                            } else {
                                unsafe {
                                    (comb_values_ptr as *mut u8).offset(elem.current_offset)
                                        as *mut Value
                                }
                            };
                            let next = elem.next_offset.map(|off| unsafe {
                                (ff_values_ptr as *mut u8).offset(off) as *mut Value
                            });
                            (current, next)
                        })
                        .collect();
                    Statement::SystemFunctionCall(SystemFunctionCall::Readmemh {
                        filename: filename.clone(),
                        elements: resolved,
                        width: *width,
                    })
                }
            },
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
            ProtoStatement::AssignDynamic(x) => x.build_binary(context, builder),
            ProtoStatement::If(x) => x.build_binary(context, builder, is_last),
            ProtoStatement::SystemFunctionCall(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct AssignStatement {
    pub dst: *mut Value,
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
            unsafe {
                (*self.dst).assign(value, beg, end);
            }
        } else {
            unsafe {
                (*self.dst).set_value(value);
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const Value>, outputs: &mut Vec<*const Value>) {
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
        let dst = unsafe {
            (self.dst_base_ptr as *mut u8).offset(self.dst_stride * idx as isize) as *mut Value
        };

        let value = self.expr.eval(mask_cache);
        let value = if let Some((beg, end)) = self.rhs_select {
            value.select(beg, end)
        } else {
            value
        };
        if let Some((beg, end)) = self.select {
            unsafe {
                (*dst).assign(value, beg, end);
            }
        } else {
            unsafe {
                (*dst).set_value(value);
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const Value>, outputs: &mut Vec<*const Value>) {
        self.dst_index_expr.gather_variable(inputs, &mut vec![]);
        self.expr.gather_variable(inputs, &mut vec![]);
        for i in 0..self.dst_num_elements {
            let ptr = unsafe {
                (self.dst_base_ptr as *const u8).offset(self.dst_stride * i as isize)
                    as *const Value
            };
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
}

impl ProtoAssignStatement {
    pub fn can_build_binary(&self) -> bool {
        if !self.expr.can_build_binary() {
            return false;
        }
        self.dst_width <= 64
    }

    pub fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut FfValue,
        comb_values_ptr: *mut CombValue,
    ) -> AssignStatement {
        // dst_offset is a byte offset: next field for FF, current field for comb
        let dst = if self.dst_is_ff {
            unsafe { (ff_values_ptr as *mut u8).add(self.dst_offset as usize) as *mut Value }
        } else {
            unsafe { (comb_values_ptr as *mut u8).add(self.dst_offset as usize) as *mut Value }
        };

        let expr = self.expr.apply_values_ptr(ff_values_ptr, comb_values_ptr);

        AssignStatement {
            dst,
            select: self.select,
            rhs_select: self.rhs_select,
            expr,
        }
    }

    pub fn build_binary(
        &self,
        context: &mut CraneliftContext,
        builder: &mut FunctionBuilder,
    ) -> Option<()> {
        let (mut payload, mut mask_xz) = self.expr.build_binary(context, builder)?;

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

        let load_mem_flag = MemFlags::trusted().with_readonly();
        let store_mem_flag = MemFlags::trusted();

        let base_addr = if self.dst_is_ff {
            context.ff_values
        } else {
            context.comb_values
        };

        let dst_offset = (self.dst_offset + value_u64_offset()) as i32;

        if let Some((beg, end)) = self.select {
            let mask = ValueU64::gen_mask_range(beg, end);

            let payload = builder.ins().ishl_imm(payload, end as i64);
            let org = builder
                .ins()
                .load(I64, load_mem_flag, base_addr, dst_offset);
            let org = builder.ins().band_imm(org, !mask as i64);
            let payload = builder.ins().bor(payload, org);

            builder
                .ins()
                .store(store_mem_flag, payload, base_addr, dst_offset);
            if let Some(mask_xz) = mask_xz {
                let mask_xz = builder.ins().ishl_imm(mask_xz, end as i64);
                let org = builder
                    .ins()
                    .load(I64, load_mem_flag, base_addr, dst_offset + 8);
                let org = builder.ins().band_imm(org, !mask as i64);
                let mask_xz = builder.ins().bor(mask_xz, org);

                builder
                    .ins()
                    .store(store_mem_flag, mask_xz, base_addr, dst_offset + 8);
            }
        } else {
            match self.dst_width {
                8 => {
                    builder
                        .ins()
                        .istore8(store_mem_flag, payload, base_addr, dst_offset);
                    if let Some(mask_xz) = mask_xz {
                        builder
                            .ins()
                            .istore8(store_mem_flag, mask_xz, base_addr, dst_offset + 8);
                    }
                }
                16 => {
                    builder
                        .ins()
                        .istore16(store_mem_flag, payload, base_addr, dst_offset);
                    if let Some(mask_xz) = mask_xz {
                        builder
                            .ins()
                            .istore16(store_mem_flag, mask_xz, base_addr, dst_offset + 8);
                    }
                }
                32 => {
                    builder
                        .ins()
                        .istore32(store_mem_flag, payload, base_addr, dst_offset);
                    if let Some(mask_xz) = mask_xz {
                        builder
                            .ins()
                            .istore32(store_mem_flag, mask_xz, base_addr, dst_offset + 8);
                    }
                }
                64 => {
                    builder
                        .ins()
                        .store(store_mem_flag, payload, base_addr, dst_offset);
                    if let Some(mask_xz) = mask_xz {
                        builder
                            .ins()
                            .store(store_mem_flag, mask_xz, base_addr, dst_offset + 8);
                    }
                }
                _ => {
                    if self.dst_width >= 64 {
                        return None;
                    }
                    let mask = (1u64 << self.dst_width) - 1;
                    let payload = builder.ins().band_imm(payload, mask as i64);

                    builder
                        .ins()
                        .store(store_mem_flag, payload, base_addr, dst_offset);
                    if let Some(mask_xz) = mask_xz {
                        let mask_xz = builder.ins().band_imm(mask_xz, mask as i64);
                        builder
                            .ins()
                            .store(store_mem_flag, mask_xz, base_addr, dst_offset + 8);
                    }
                }
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
                Value::BigUint(x) => *x.payload != (&*x.payload & &*x.mask_xz),
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

    pub fn gather_variable(&self, inputs: &mut Vec<*const Value>, outputs: &mut Vec<*const Value>) {
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

    pub fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut FfValue,
        comb_values_ptr: *mut CombValue,
    ) -> IfStatement {
        let cond = self
            .cond
            .as_ref()
            .map(|x| x.apply_values_ptr(ff_values_ptr, comb_values_ptr));
        let true_side: Vec<_> = self
            .true_side
            .iter()
            .map(|x| x.apply_values_ptr(ff_values_ptr, comb_values_ptr))
            .collect();
        let false_side: Vec<_> = self
            .false_side
            .iter()
            .map(|x| x.apply_values_ptr(ff_values_ptr, comb_values_ptr))
            .collect();

        IfStatement {
            cond,
            true_side,
            false_side,
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
                _ => return None,
            },
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
            // FF assignment writes to next
            let dst_offset = if dst_is_ff {
                element.next_offset
            } else {
                element.current_offset
            };

            let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

            Some(ProtoStatement::Assign(ProtoAssignStatement {
                dst_offset,
                dst_is_ff,
                dst_width,
                select,
                rhs_select: None,
                expr,
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
        let dst_width = meta.width;
        // FF assignment writes to next
        let dst_offset = if dst_is_ff {
            element.next_offset
        } else {
            element.current_offset
        };

        let expr: ProtoExpression = Conv::conv(context, &src.expr)?;

        Some(ProtoAssignStatement {
            dst_offset,
            dst_is_ff,
            dst_width,
            select,
            rhs_select: None,
            expr,
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
