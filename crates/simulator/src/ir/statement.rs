use crate::HashMap;
use crate::HashSet;
use crate::assert_buffer;
use crate::backend::ChunkArtifact;
use crate::ir::context::{Context, Conv};
use crate::ir::expression::{
    DynamicBitSelect, ExpressionContext, ProtoDynamicBitSelect, build_dynamic_bit_select,
    build_linear_index_expr,
};
use crate::ir::partial_index::partial_index_base;
use crate::ir::variable::{
    VarOffset, native_bytes as calc_native_bytes, read_native_value, write_native_value,
};
use crate::ir::write_log::{
    WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES, event_write_log_push_static, event_write_log_push_wide,
};
use crate::ir::{Expression, ProtoExpression, Value};
use crate::output_buffer;
use crate::simulator_error::SimulatorError;
use std::sync::Arc;
use veryl_analyzer::conv::utils::eval_array_literal;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::{
    AssertKind, SystemFunctionInput, SystemFunctionKind, TypeKind, ValueVariant,
};
use veryl_analyzer::ir::{ControlFlow, FunctionCall, VarId};
use veryl_analyzer::value::{MaskCache, Value as AnalyzerValue};
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

/// Per-statement dependency: (input offsets, output offsets).
pub type StmtDep = (Vec<VarOffset>, Vec<VarOffset>);

pub enum ProtoStatementBlock {
    Interpreted(Vec<ProtoStatement>),
    Compiled(Arc<ChunkArtifact>),
}

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
                ProtoStatementBlock::Compiled(artifact) => {
                    // log_buf populated by `Ir::install_write_log_ptr` after
                    // WriteLogBuffer allocation; null until then.
                    result.push(Statement::Compiled(CompiledStmt {
                        artifact: Arc::clone(artifact),
                        ff: ff_ptr as *const u8,
                        comb: comb_ptr as *const u8,
                        log_buf: std::ptr::null_mut(),
                        ff_delta: 0,
                    }));
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
    Write {
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
        kind: AssertKind,
        condition: Expression,
        format_str: String,
        args: Vec<Expression>,
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
    /// Write-log push uses (`ff_log_base_current_offset` + `idx *
    /// dst_stride`) at runtime.  `ff_log_base_current_offset` is
    /// `dst_base.raw() - value_size` for the element-0 current slot.
    /// `None` outside the FF-2state-narrow emit gate (see
    /// `apply_values_ptr`).
    pub ff_log_base_current_offset: Option<u32>,
    /// `dst_base` is the FF current slot (packed, single-slot array). eval_step
    /// then skips the in-place write so another block's same-edge read stays
    /// read-OLD (NBA), not read-NEW; the write-log commit delivers the value.
    /// Unpacked (dst_base = next slot) keeps the store for multi-RMW forwarding.
    /// Mirrors AssignStatement::ff_is_packed.
    pub ff_is_packed: bool,
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
    FileOpen {
        path: String,
        append: bool,
    },
    FileWrite {
        format_str: String,
        args: Vec<ProtoExpression>,
    },
    FileClose,
    FileFlush,
    Component {
        method: StrId,
        args: Vec<ProtoComponentArg>,
        /// Assignment form: destination variable for the return value.
        ret: Option<(VarId, RetWidthCheck)>,
    },
}

/// How a component method's returned width is validated before it lands
/// in the destination variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetWidthCheck {
    /// Direct assignment to a user variable: truncated to its width.
    Dst,
    /// Undeclared width into a hoist temporary: at most 64 bits.
    Max64,
    /// Declared width: the returned value must match exactly.
    Exact(u32),
}

/// Argument of a user-defined component method call: string literals are
/// extracted at conv time, everything else stays an evaluatable expression.
#[derive(Clone, Debug)]
pub enum ProtoComponentArg {
    Str(String),
    Expr(ProtoExpression),
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
    FileOpen {
        path: String,
        append: bool,
    },
    FileWrite {
        format_str: String,
        args: Vec<Expression>,
    },
    FileClose,
    FileFlush,
    Component {
        method: StrId,
        args: Vec<ComponentArg>,
        ret: Option<(VarId, RetWidthCheck)>,
    },
}

/// Pointer-bound form of [`ProtoComponentArg`].
#[derive(Clone)]
pub enum ComponentArg {
    Str(String),
    Expr(Expression),
}

#[derive(Clone)]
pub enum RuntimeForBound {
    Const(u64),
    Dynamic(Box<Expression>),
}

// SAFETY: Same as Expression — raw pointers are used for memory access.
unsafe impl Send for RuntimeForBound {}

impl RuntimeForBound {
    pub fn eval(&self, mask_cache: &mut MaskCache) -> u64 {
        match self {
            RuntimeForBound::Const(v) => *v,
            RuntimeForBound::Dynamic(expr) => {
                let val = expr.eval(mask_cache);
                val.to_usize().unwrap_or(0) as u64
            }
        }
    }
}

#[derive(Clone)]
pub struct RuntimeForRange {
    pub start: RuntimeForBound,
    pub end: RuntimeForBound,
    pub inclusive: bool,
    pub step: u64,
    pub op: Option<air::Op>,
    pub reverse: bool,
}

#[derive(Clone)]
pub struct ForStatement {
    pub var_ptr: *mut u8,
    pub var_native_bytes: usize,
    pub var_use_4state: bool,
    pub var_width: usize,
    pub var_signed: bool,
    pub range: RuntimeForRange,
    pub body: Vec<Statement>,
}

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    AssignDynamic(AssignDynamicStatement),
    If(IfStatement),
    For(ForStatement),
    Break,
    /// `(artifact.func)(ff, comb, log_buf)` — `log_buf` filled in by
    /// `Ir::install_write_log_ptr`.
    Compiled(CompiledStmt),
    /// Consecutive `Compiled` chunks sharing the same `artifact`.
    CompiledBatch(CompiledBatchStmt),
    SequentialBlock(Vec<Statement>),
    SystemFunctionCall(SystemFunctionCall),
    TbMethodCall {
        inst: StrId,
        method: TbMethodKind,
    },
}

#[derive(Clone)]
pub struct CompiledStmt {
    pub artifact: Arc<ChunkArtifact>,
    pub ff: *const u8,
    pub comb: *const u8,
    pub log_buf: *mut u8,
    /// FF byte delta passed to the chunk (4th `FuncPtr` arg) so it records
    /// absolute write-log offsets. 0 unless this is a relocated cache reuse.
    pub ff_delta: isize,
}

#[derive(Clone)]
pub struct CompiledBatchStmt {
    pub artifact: Arc<ChunkArtifact>,
    pub log_buf: *mut u8,
    /// `(ff, comb, ff_delta)` per batched instance.
    pub args: Vec<(*const u8, *const u8, isize)>,
}

// SAFETY: Raw pointers point into the owning Ir's exclusively-owned buffers.
// No cross-thread aliasing when each thread operates on a distinct Ir.
unsafe impl Send for Statement {}

/// Fill the `log_buf` slot in every nested `Compiled` / `CompiledBatch`.
/// Called once at end of `Ir::from_module` so emitted code can perform
/// inline log pushes via the 3rd `FuncPtr` argument.
pub fn patch_stmt_log_buf(s: &mut Statement, log_buf: *mut u8) {
    match s {
        Statement::Compiled(c) => {
            c.log_buf = log_buf;
        }
        Statement::CompiledBatch(c) => {
            c.log_buf = log_buf;
        }
        Statement::If(if_stmt) => {
            for s in &mut if_stmt.true_side {
                patch_stmt_log_buf(s, log_buf);
            }
            for s in &mut if_stmt.false_side {
                patch_stmt_log_buf(s, log_buf);
            }
        }
        Statement::For(for_stmt) => {
            for s in &mut for_stmt.body {
                patch_stmt_log_buf(s, log_buf);
            }
        }
        Statement::SequentialBlock(body) => {
            for s in body {
                patch_stmt_log_buf(s, log_buf);
            }
        }
        Statement::Assign(_)
        | Statement::AssignDynamic(_)
        | Statement::Break
        | Statement::SystemFunctionCall(_)
        | Statement::TbMethodCall { .. } => {}
    }
}
unsafe impl Send for AssignStatement {}
unsafe impl Send for AssignDynamicStatement {}
unsafe impl Send for IfStatement {}
unsafe impl Send for ForStatement {}
unsafe impl Send for SystemFunctionCall {}

impl Statement {
    pub fn is_compiled(&self) -> bool {
        matches!(self, Statement::Compiled(_) | Statement::CompiledBatch(_))
    }

    pub fn eval_step(&self, mask_cache: &mut MaskCache) -> ControlFlow {
        match self {
            Statement::Assign(x) => {
                x.eval_step(mask_cache);
                ControlFlow::Continue
            }
            Statement::AssignDynamic(x) => {
                x.eval_step(mask_cache);
                ControlFlow::Continue
            }
            Statement::If(x) => x.eval_step(mask_cache),
            Statement::For(x) => {
                let r = &x.range;
                let start = r.start.eval(mask_cache);
                let mut end = r.end.eval(mask_cache);
                if r.inclusive {
                    end = end.saturating_add(1);
                }
                let mut step_body = |i: u64| -> ControlFlow {
                    let val = Value::new(i, x.var_width, x.var_signed);
                    unsafe {
                        write_native_value(x.var_ptr, x.var_native_bytes, x.var_use_4state, &val);
                    }
                    for s in &x.body {
                        if s.eval_step(mask_cache) == ControlFlow::Break {
                            return ControlFlow::Break;
                        }
                    }
                    ControlFlow::Continue
                };
                if r.reverse {
                    // Mirror the emitted SV `for (int i = hi - 1; i >= lo;
                    // i -= step)`; i64 makes underflow past lo terminate.
                    let mut i = end as i64 - 1;
                    let lo = start as i64;
                    let step = r.step as i64;
                    while i >= lo {
                        if step_body(i as u64) == ControlFlow::Break {
                            break;
                        }
                        i -= step;
                    }
                } else if let Some(op) = &r.op {
                    let mut i = start;
                    while i < end {
                        if step_body(i) == ControlFlow::Break {
                            break;
                        }
                        // Progress guard: a stalled or faulting step would
                        // spin this delta step forever (const-bound cases are
                        // rejected at analysis; runtime bounds reach here).
                        match op.eval(i as usize, r.step as usize) {
                            Some(n) if n as u64 > i => i = n as u64,
                            _ => break,
                        }
                    }
                } else {
                    let mut i = start;
                    while i < end {
                        if step_body(i) == ControlFlow::Break {
                            break;
                        }
                        i += r.step;
                    }
                }
                ControlFlow::Continue
            }
            Statement::Break => ControlFlow::Break,
            Statement::Compiled(c) => unsafe {
                (c.artifact.func)(c.ff, c.comb, c.log_buf, c.ff_delta);
                ControlFlow::Continue
            },
            Statement::CompiledBatch(c) => unsafe {
                let f = c.artifact.func;
                for &(ff, comb, ff_delta) in &c.args {
                    f(ff, comb, c.log_buf, ff_delta);
                }
                ControlFlow::Continue
            },
            Statement::SequentialBlock(body) => {
                for s in body {
                    if s.eval_step(mask_cache) == ControlFlow::Break {
                        return ControlFlow::Break;
                    }
                }
                ControlFlow::Continue
            }
            Statement::SystemFunctionCall(x) => {
                x.eval_step(mask_cache);
                ControlFlow::Continue
            }
            Statement::TbMethodCall { .. } => ControlFlow::Continue,
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>, outputs: &mut Vec<*const u8>) {
        match self {
            Statement::Assign(x) => x.gather_variable(inputs, outputs),
            Statement::AssignDynamic(x) => x.gather_variable(inputs, outputs),
            Statement::If(x) => x.gather_variable(inputs, outputs),
            Statement::For(x) => {
                for s in &x.body {
                    s.gather_variable(inputs, outputs);
                }
            }
            Statement::Compiled(_) | Statement::CompiledBatch(_) | Statement::Break => {}
            Statement::SequentialBlock(body) => {
                for s in body {
                    s.gather_variable(inputs, outputs);
                }
            }
            Statement::SystemFunctionCall(x) => x.gather_variable(inputs),
            Statement::TbMethodCall { .. } => (),
        }
    }
}

pub fn format_assert_message(
    format_str: &str,
    args: &[Expression],
    mask_cache: &mut MaskCache,
) -> String {
    if format_str.is_empty() && args.is_empty() {
        return "assertion failed".to_string();
    }
    if format_str.is_empty() {
        let values: Vec<_> = args.iter().map(|e| e.eval(mask_cache)).collect();
        let parts: Vec<String> = values.iter().map(|v| v.format_hex()).collect();
        return parts.join(" ");
    }
    if args.is_empty() {
        return format_str.to_string();
    }
    let values: Vec<_> = args.iter().map(|e| e.eval(mask_cache)).collect();
    format_display_string(format_str, &values)
}

fn format_display_string(format_str: &str, values: &[AnalyzerValue]) -> String {
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

/// Format a `$display`/`$write`-style argument list, shared with the
/// `$tb::file` write path so its output matches `$display`.
pub fn format_output(format_str: &str, values: &[AnalyzerValue]) -> String {
    if format_str.is_empty() {
        values
            .iter()
            .map(|v| v.format_hex())
            .collect::<Vec<_>>()
            .join(" ")
    } else if values.is_empty() {
        format_str.to_string()
    } else {
        format_display_string(format_str, values)
    }
}

/// Callback invoked by AOT-C event `.so` code for `$display` / `$write`.
/// Mirrors `SystemFunctionCall::{Display,Write}::eval_step` so emitted output is
/// byte-identical and flows through the same thread-local `output_buffer` (which
/// prevents interleaving across parallel tests).  `fmt`/`fmt_len` carry the Veryl
/// format-string bytes; `vals`/`widths` carry `n` argument payloads (≤ 64 bits —
/// the emitter bails to Cranelift for wider args); `newline` is 1 for `$display`,
/// 0 for `$write`.  The pointer is handed to the `.so` via `veryl_set_sysfn_cb`
/// in `compile_source`.
///
/// # Safety
/// `fmt` must point to `fmt_len` readable bytes (or be null with `fmt_len` 0),
/// and `vals`/`widths` to `n` readable elements (or null with `n` 0).  Emitted
/// code always satisfies this.
pub unsafe extern "C" fn veryl_aot_sysfn_print(
    fmt: *const u8,
    fmt_len: usize,
    vals: *const u64,
    widths: *const u32,
    n: usize,
    newline: u32,
) {
    let fmt = if fmt.is_null() || fmt_len == 0 {
        ""
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(fmt, fmt_len) };
        std::str::from_utf8(bytes).unwrap_or("")
    };
    let values: Vec<AnalyzerValue> = if n == 0 || vals.is_null() || widths.is_null() {
        Vec::new()
    } else {
        (0..n)
            .map(|i| {
                let v = unsafe { *vals.add(i) };
                // `widths[i]` packs the bit width (low 16 bits) with the
                // signedness flag (bit 16); see `emit_event_print`.  Signedness
                // changes %d/%s output, so it must match the interpreter's
                // eval() value.
                let packed = unsafe { *widths.add(i) } as usize;
                let width = packed & 0xffff;
                let signed = (packed >> 16) & 1 != 0;
                AnalyzerValue::new(v, width, signed)
            })
            .collect()
    };
    // Mirror eval_step's Display/Write formatting precisely.
    let output = if fmt.is_empty() {
        values
            .iter()
            .map(|v| v.format_hex())
            .collect::<Vec<_>>()
            .join(" ")
    } else if values.is_empty() {
        fmt.to_string()
    } else {
        format_display_string(fmt, &values)
    };
    if newline != 0 {
        output_buffer::println(&output);
    } else {
        output_buffer::print(&output);
    }
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
            SystemFunctionCall::Write { format_str, args } => {
                let values: Vec<_> = args.iter().map(|e| e.eval(mask_cache)).collect();
                if format_str.is_empty() {
                    let parts: Vec<String> = values.iter().map(|v| v.format_hex()).collect();
                    output_buffer::print(&parts.join(" "));
                } else if values.is_empty() {
                    output_buffer::print(format_str);
                } else {
                    let output = format_display_string(format_str, &values);
                    output_buffer::print(&output);
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
            SystemFunctionCall::Assert {
                kind,
                condition,
                format_str,
                args,
            } => {
                let val = condition.eval(mask_cache);
                if val.payload_u64() == 0 {
                    let msg = format_assert_message(format_str, args, mask_cache);
                    match kind {
                        AssertKind::Fatal => assert_buffer::record_fatal(msg),
                        AssertKind::Continue => assert_buffer::record_continue(msg),
                    }
                }
            }
            SystemFunctionCall::Finish => {
                // Handled by testbench driver
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<*const u8>) {
        match self {
            SystemFunctionCall::Display { args, .. } | SystemFunctionCall::Write { args, .. } => {
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
    Write {
        format_str: String,
        args: Vec<ProtoExpression>,
    },
    Readmemh {
        filename: String,
        elements: Vec<ReadmemhElement>,
        width: usize,
    },
    Assert {
        kind: AssertKind,
        condition: ProtoExpression,
        format_str: String,
        args: Vec<ProtoExpression>,
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

/// Reused compiled block from a cached module instance.  Byte deltas
/// let the same compiled code run with adjusted ff/comb base pointers.
#[derive(Clone, Debug)]
pub struct CompiledBlockStatement {
    pub artifact: Arc<ChunkArtifact>,
    pub ff_delta_bytes: isize,
    pub comb_delta_bytes: isize,
    pub input_offsets: Vec<VarOffset>,
    pub output_offsets: Vec<VarOffset>,
    /// Canonical offsets for FF variables written by this block.
    pub ff_canonical_offsets: Vec<isize>,
    /// Per-statement (inputs, outputs) from the pre-JIT originals.
    /// `analyze_dependency` uses this for fine-grained DAG analysis to
    /// avoid false combinational loops from coarse lumping.
    pub stmt_deps: Vec<StmtDep>,
    /// Pre-JIT originals, expanded by `analyze_dependency` when a
    /// CompiledBlock causes a false cycle.
    pub original_stmts: Vec<ProtoStatement>,
}

#[derive(Clone, Debug)]
pub enum ProtoForBound {
    Const(u64),
    Dynamic(ProtoExpression),
}

#[derive(Clone, Debug)]
pub enum ProtoForRange {
    Forward {
        start: ProtoForBound,
        end: ProtoForBound,
        inclusive: bool,
        step: u64,
    },
    Reverse {
        start: ProtoForBound,
        end: ProtoForBound,
        inclusive: bool,
        step: u64,
    },
    Stepped {
        start: ProtoForBound,
        end: ProtoForBound,
        inclusive: bool,
        step: u64,
        op: air::Op,
    },
}

impl ProtoForRange {
    pub(crate) fn is_const(&self) -> bool {
        let (s, e) = match self {
            ProtoForRange::Forward { start, end, .. }
            | ProtoForRange::Reverse { start, end, .. }
            | ProtoForRange::Stepped { start, end, .. } => (start, end),
        };
        matches!(s, ProtoForBound::Const(_)) && matches!(e, ProtoForBound::Const(_))
    }

    /// The dynamic (runtime) start/end bound expressions, if any; the
    /// variables they read are inputs of the enclosing statement.
    pub(crate) fn dynamic_bounds(&self) -> impl Iterator<Item = &ProtoExpression> {
        let (s, e) = match self {
            ProtoForRange::Forward { start, end, .. }
            | ProtoForRange::Reverse { start, end, .. }
            | ProtoForRange::Stepped { start, end, .. } => (start, end),
        };
        [s, e].into_iter().filter_map(|b| match b {
            ProtoForBound::Dynamic(e) => Some(e),
            ProtoForBound::Const(_) => None,
        })
    }
}

impl ProtoForBound {
    pub fn as_const(&self) -> Option<u64> {
        match self {
            ProtoForBound::Const(v) => Some(*v),
            ProtoForBound::Dynamic(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProtoForStatement {
    pub var_offset: VarOffset,
    pub var_width: usize,
    pub var_native_bytes: usize,
    pub var_signed: bool,
    pub range: ProtoForRange,
    pub body: Vec<ProtoStatement>,
}

#[derive(Clone, Debug)]
pub enum ProtoStatement {
    Assign(ProtoAssignStatement),
    AssignDynamic(ProtoAssignDynamicStatement),
    If(ProtoIfStatement),
    For(ProtoForStatement),
    Break,
    SystemFunctionCall(ProtoSystemFunctionCall),
    CompiledBlock(CompiledBlockStatement),
    /// Sequential statement group (always_comb / inlined function body).
    /// Dependency analysis sees only external I/O; internal variables are hidden.
    SequentialBlock(Vec<ProtoStatement>),
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
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
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
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    condition.adjust_offsets(ff_delta, comb_delta);
                    for arg in args {
                        arg.adjust_offsets(ff_delta, comb_delta);
                    }
                }
                ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(_) => {
                // CompiledBlocks use ff_delta_bytes/comb_delta_bytes at runtime.
                // Their original_stmts should be adjusted separately if needed.
            }
            ProtoStatement::For(x) => {
                x.var_offset = x.var_offset.adjust(ff_delta, comb_delta);
                let adjust_bound = |b: &mut ProtoForBound| {
                    if let ProtoForBound::Dynamic(expr) = b {
                        expr.adjust_offsets(ff_delta, comb_delta);
                    }
                };
                match &mut x.range {
                    ProtoForRange::Forward { start, end, .. }
                    | ProtoForRange::Reverse { start, end, .. }
                    | ProtoForRange::Stepped { start, end, .. } => {
                        adjust_bound(start);
                        adjust_bound(end);
                    }
                }
                for s in &mut x.body {
                    s.adjust_offsets(ff_delta, comb_delta);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                for s in body {
                    s.adjust_offsets(ff_delta, comb_delta);
                }
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
                ProtoTbMethodKind::FileWrite { args, .. } => {
                    for a in args {
                        a.adjust_offsets(ff_delta, comb_delta);
                    }
                }
                ProtoTbMethodKind::Component { args, .. } => {
                    for a in args {
                        if let ProtoComponentArg::Expr(e) = a {
                            e.adjust_offsets(ff_delta, comb_delta);
                        }
                    }
                }
                ProtoTbMethodKind::FileOpen { .. }
                | ProtoTbMethodKind::FileClose
                | ProtoTbMethodKind::FileFlush => {}
            },
            ProtoStatement::Break => {}
        }
    }

    /// Byte offsets this statement WRITES, recursing into `If`/`For`/block
    /// bodies. Read-only references are intentionally excluded — see the
    /// per-call-site relocation in expression.rs for why.
    pub fn collect_written_offsets(&self, out: &mut Vec<VarOffset>) {
        match self {
            ProtoStatement::Assign(x) => out.push(x.dst),
            ProtoStatement::AssignDynamic(x) => out.push(x.dst_base),
            ProtoStatement::If(x) => {
                for s in &x.true_side {
                    s.collect_written_offsets(out);
                }
                for s in &x.false_side {
                    s.collect_written_offsets(out);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => {
                if let ProtoSystemFunctionCall::Readmemh { elements, .. } = x {
                    for elem in elements {
                        out.push(elem.current);
                    }
                }
            }
            ProtoStatement::CompiledBlock(_) => {}
            ProtoStatement::For(x) => {
                // The loop counter is written by the loop driver each iteration.
                out.push(x.var_offset);
                for s in &x.body {
                    s.collect_written_offsets(out);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                for s in body {
                    s.collect_written_offsets(out);
                }
            }
            ProtoStatement::TbMethodCall { .. } => {}
            ProtoStatement::Break => {}
        }
    }

    /// Replace embedded byte offsets present in `map`. Mirrors `adjust_offsets`'s
    /// walk; `map` holds only comb offsets, so FF offsets are never affected.
    pub fn remap_offsets(&mut self, map: &HashMap<VarOffset, VarOffset>) {
        match self {
            ProtoStatement::Assign(x) => {
                if let Some(&n) = map.get(&x.dst) {
                    x.dst = n;
                }
                x.expr.remap_offsets(map);
                if let Some(dyn_sel) = &mut x.dynamic_select {
                    dyn_sel.index_expr.remap_offsets(map);
                }
            }
            ProtoStatement::AssignDynamic(x) => {
                if let Some(&n) = map.get(&x.dst_base) {
                    x.dst_base = n;
                }
                x.dst_index_expr.remap_offsets(map);
                x.expr.remap_offsets(map);
                if let Some(dyn_sel) = &mut x.dynamic_select {
                    dyn_sel.index_expr.remap_offsets(map);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(cond) = &mut x.cond {
                    cond.remap_offsets(map);
                }
                for s in &mut x.true_side {
                    s.remap_offsets(map);
                }
                for s in &mut x.false_side {
                    s.remap_offsets(map);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
                    for arg in args {
                        arg.remap_offsets(map);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { elements, .. } => {
                    for elem in elements {
                        if let Some(&n) = map.get(&elem.current) {
                            elem.current = n;
                        }
                    }
                }
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    condition.remap_offsets(map);
                    for arg in args {
                        arg.remap_offsets(map);
                    }
                }
                ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(_) => {}
            ProtoStatement::For(x) => {
                if let Some(&n) = map.get(&x.var_offset) {
                    x.var_offset = n;
                }
                let remap_bound = |b: &mut ProtoForBound| {
                    if let ProtoForBound::Dynamic(expr) = b {
                        expr.remap_offsets(map);
                    }
                };
                match &mut x.range {
                    ProtoForRange::Forward { start, end, .. }
                    | ProtoForRange::Reverse { start, end, .. }
                    | ProtoForRange::Stepped { start, end, .. } => {
                        remap_bound(start);
                        remap_bound(end);
                    }
                }
                for s in &mut x.body {
                    s.remap_offsets(map);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                for s in body {
                    s.remap_offsets(map);
                }
            }
            ProtoStatement::TbMethodCall { method, .. } => match method {
                ProtoTbMethodKind::ClockNext { count, period } => {
                    if let Some(c) = count {
                        c.remap_offsets(map);
                    }
                    if let Some(p) = period {
                        p.remap_offsets(map);
                    }
                }
                ProtoTbMethodKind::ResetAssert { duration, .. } => {
                    if let Some(d) = duration {
                        d.remap_offsets(map);
                    }
                }
                ProtoTbMethodKind::FileWrite { args, .. } => {
                    for a in args {
                        a.remap_offsets(map);
                    }
                }
                ProtoTbMethodKind::Component { args, .. } => {
                    for a in args {
                        if let ProtoComponentArg::Expr(e) = a {
                            e.remap_offsets(map);
                        }
                    }
                }
                ProtoTbMethodKind::FileOpen { .. }
                | ProtoTbMethodKind::FileClose
                | ProtoTbMethodKind::FileFlush => {}
            },
            ProtoStatement::Break => {}
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
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
                    for arg in args {
                        arg.gather_variable_offsets(inputs);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { .. } => {}
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    condition.gather_variable_offsets(inputs);
                    for arg in args {
                        arg.gather_variable_offsets(inputs);
                    }
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
            ProtoStatement::For(x) => {
                // Dynamic loop bounds are reads of the enclosing statement.
                for e in x.range.dynamic_bounds() {
                    e.gather_variable_offsets(inputs);
                }
                for s in &x.body {
                    s.gather_variable_offsets(inputs, outputs);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                let mut all_ins = vec![];
                let mut all_outs = vec![];
                for s in body {
                    s.gather_variable_offsets(&mut all_ins, &mut all_outs);
                }
                let input_set: HashSet<VarOffset> = all_ins.iter().cloned().collect();
                let output_set: HashSet<VarOffset> = all_outs.iter().cloned().collect();
                let internal: HashSet<VarOffset> =
                    input_set.intersection(&output_set).cloned().collect();
                for off in all_ins {
                    if !internal.contains(&off) {
                        inputs.push(off);
                    }
                }
                outputs.extend(all_outs);
            }
            ProtoStatement::TbMethodCall { .. } => {}
            ProtoStatement::Break => {}
        }
    }

    /// Read-side counterpart of `gather_bit_aware_outputs`: every read
    /// carries its static bit range when known (`None` = full width /
    /// runtime-determined).  Mirrors `gather_variable_offsets`' walk,
    /// including the SequentialBlock internal-read filtering and the
    /// CompiledBlock comb-only encoding.
    pub fn gather_reads_with_ranges(&self, out: &mut Vec<(VarOffset, Option<(usize, usize)>)>) {
        match self {
            ProtoStatement::Assign(x) => {
                x.expr.gather_reads_with_ranges(out);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_reads_with_ranges(out);
                }
            }
            ProtoStatement::AssignDynamic(x) => {
                x.dst_index_expr.gather_reads_with_ranges(out);
                x.expr.gather_reads_with_ranges(out);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_reads_with_ranges(out);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(cond) = &x.cond {
                    cond.gather_reads_with_ranges(out);
                }
                for s in &x.true_side {
                    s.gather_reads_with_ranges(out);
                }
                for s in &x.false_side {
                    s.gather_reads_with_ranges(out);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
                    for arg in args {
                        arg.gather_reads_with_ranges(out);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { .. } => {}
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    condition.gather_reads_with_ranges(out);
                    for arg in args {
                        arg.gather_reads_with_ranges(out);
                    }
                }
                ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(x) => {
                if !x.stmt_deps.is_empty() {
                    for (ins, _) in &x.stmt_deps {
                        for &off in ins {
                            if !off.is_ff() {
                                out.push((VarOffset::Comb(off.raw()), None));
                            }
                        }
                    }
                } else {
                    for &off in &x.input_offsets {
                        if !off.is_ff() {
                            out.push((VarOffset::Comb(off.raw()), None));
                        }
                    }
                }
            }
            ProtoStatement::For(x) => {
                for e in x.range.dynamic_bounds() {
                    e.gather_reads_with_ranges(out);
                }
                for s in &x.body {
                    s.gather_reads_with_ranges(out);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                let mut all_reads = vec![];
                let mut all_outs = vec![];
                for s in body {
                    s.gather_reads_with_ranges(&mut all_reads);
                    let mut ins = vec![];
                    s.gather_variable_offsets(&mut ins, &mut all_outs);
                }
                let output_set: HashSet<VarOffset> = all_outs.into_iter().collect();
                for (off, range) in all_reads {
                    if !output_set.contains(&off) {
                        out.push((off, range));
                    }
                }
            }
            ProtoStatement::TbMethodCall { .. } => {}
            ProtoStatement::Break => {}
        }
    }

    /// Same as `gather_variable_offsets` but fully expands dynamic reads
    /// and writes to every element offset. Used by dead-store elimination
    /// (`dup_assign_dce`) so a runtime-indexed read keeps every element it
    /// could touch alive. `analyze_dependency` keeps using the base+last
    /// encoding.
    pub fn gather_variable_offsets_expanded(
        &self,
        inputs: &mut Vec<VarOffset>,
        outputs: &mut Vec<VarOffset>,
    ) {
        match self {
            ProtoStatement::Assign(x) => {
                x.expr.gather_variable_offsets_expanded(inputs);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets_expanded(inputs);
                }
                outputs.push(x.dst);
            }
            ProtoStatement::AssignDynamic(x) => {
                x.dst_index_expr.gather_variable_offsets_expanded(inputs);
                x.expr.gather_variable_offsets_expanded(inputs);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_variable_offsets_expanded(inputs);
                }
                for i in 0..x.dst_num_elements {
                    let off = VarOffset::new(
                        x.dst_base.is_ff(),
                        x.dst_base.raw() + x.dst_stride * (i as isize),
                    );
                    outputs.push(off);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(cond) = &x.cond {
                    cond.gather_variable_offsets_expanded(inputs);
                }
                for s in &x.true_side {
                    s.gather_variable_offsets_expanded(inputs, outputs);
                }
                for s in &x.false_side {
                    s.gather_variable_offsets_expanded(inputs, outputs);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
                    for arg in args {
                        arg.gather_variable_offsets_expanded(inputs);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { .. } => {}
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    condition.gather_variable_offsets_expanded(inputs);
                    // Assert message args are also reads (mirror the
                    // non-expanded variant).
                    for arg in args {
                        arg.gather_variable_offsets_expanded(inputs);
                    }
                }
                ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(x) => {
                // Prefer walking the original statements so AssignDynamic /
                // DynamicVariable expansions are applied; fall back to the
                // cached base+last input_offsets / output_offsets if the
                // originals weren't retained.
                if !x.original_stmts.is_empty() {
                    for s in &x.original_stmts {
                        s.gather_variable_offsets_expanded(inputs, outputs);
                    }
                } else if !x.stmt_deps.is_empty() {
                    for (ins, outs) in &x.stmt_deps {
                        for &off in ins {
                            inputs.push(off);
                        }
                        for &off in outs {
                            outputs.push(off);
                        }
                    }
                } else {
                    for &off in &x.input_offsets {
                        inputs.push(off);
                    }
                    for &off in &x.output_offsets {
                        outputs.push(off);
                    }
                }
            }
            ProtoStatement::For(x) => {
                for e in x.range.dynamic_bounds() {
                    e.gather_variable_offsets_expanded(inputs);
                }
                for s in &x.body {
                    s.gather_variable_offsets_expanded(inputs, outputs);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                // No "internal" filter here: a `SequentialBlock` that writes
                // to an array it also reads would silently drop its outside
                // read dependency under the intersection filter, hiding a
                // genuine read.  Extra local-temp entries are harmless (they
                // just have no external readers), so we keep the full
                // input/output sets.
                for s in body {
                    s.gather_variable_offsets_expanded(inputs, outputs);
                }
            }
            ProtoStatement::TbMethodCall { .. } => {}
            ProtoStatement::Break => {}
        }
    }

    /// Read-side `DynamicVariable` accesses recorded as ranges (see the
    /// `ProtoExpression` method).  Used by DCE to commit pending writes a
    /// dynamic read may touch without expanding to every element.  Walks only
    /// the read positions; write dsts are handled by the base+last `outs` set.
    pub fn gather_dynamic_read_ranges(&self, ranges: &mut Vec<(bool, isize, isize, usize)>) {
        match self {
            ProtoStatement::Assign(x) => {
                x.expr.gather_dynamic_read_ranges(ranges);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_dynamic_read_ranges(ranges);
                }
            }
            ProtoStatement::AssignDynamic(x) => {
                x.dst_index_expr.gather_dynamic_read_ranges(ranges);
                x.expr.gather_dynamic_read_ranges(ranges);
                if let Some(dyn_sel) = &x.dynamic_select {
                    dyn_sel.index_expr.gather_dynamic_read_ranges(ranges);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(cond) = &x.cond {
                    cond.gather_dynamic_read_ranges(ranges);
                }
                for s in &x.true_side {
                    s.gather_dynamic_read_ranges(ranges);
                }
                for s in &x.false_side {
                    s.gather_dynamic_read_ranges(ranges);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
                    for arg in args {
                        arg.gather_dynamic_read_ranges(ranges);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { .. } => {}
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    condition.gather_dynamic_read_ranges(ranges);
                    for arg in args {
                        arg.gather_dynamic_read_ranges(ranges);
                    }
                }
                ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(x) => {
                // Prefer the originals so their DynamicVariable reads register
                // as ranges; without them only cached base+last point offsets
                // exist (covered by the non-expanded read set), so nothing to
                // add here.
                if !x.original_stmts.is_empty() {
                    for s in &x.original_stmts {
                        s.gather_dynamic_read_ranges(ranges);
                    }
                }
            }
            ProtoStatement::For(x) => {
                for e in x.range.dynamic_bounds() {
                    e.gather_dynamic_read_ranges(ranges);
                }
                for s in &x.body {
                    s.gather_dynamic_read_ranges(ranges);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                for s in body {
                    s.gather_dynamic_read_ranges(ranges);
                }
            }
            ProtoStatement::TbMethodCall { .. } => {}
            ProtoStatement::Break => {}
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
            ProtoStatement::For(x) => {
                for s in &x.body {
                    result.extend(s.gather_ff_canonical_offsets());
                }
            }
            ProtoStatement::SystemFunctionCall(_) => {}
            ProtoStatement::CompiledBlock(x) => {
                for off in &x.ff_canonical_offsets {
                    result.insert(*off);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                for s in body {
                    result.extend(s.gather_ff_canonical_offsets());
                }
            }
            ProtoStatement::TbMethodCall { .. } => {}
            ProtoStatement::Break => {}
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
                    // Element-0 current offset used as the base for
                    // runtime index math in eval_step.  Same gate as
                    // AssignStatement.  For packed arrays `dst_base`
                    // points at `Ff(current)`, so naive `dst_base.raw()
                    // - nb` would land OOB; the canonical current base
                    // in `dst_ff_current_base_offset` is correct in both
                    // packed and unpacked layouts.
                    let _ = nb;
                    // No width cap: `eval_step` logs via `emit_ff_log`, which
                    // splits wide elements into wide entries.  (Was gated
                    // `<= 64` when the push was hand-rolled narrow-only and
                    // silently dropped wider writes.)
                    let ff_log_base_current_offset = if x.dst_base.is_ff() {
                        Some(x.dst_ff_current_base_offset as u32)
                    } else {
                        None
                    };
                    // is_ff() so a comb-target dynamic write keeps its in-place store.
                    let ff_is_packed =
                        x.dst_base.is_ff() && (x.dst_base.raw() == x.dst_ff_current_base_offset);
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
                            window: dyn_sel.window,
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
                        ff_log_base_current_offset,
                        ff_is_packed,
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
                    ProtoSystemFunctionCall::Assert {
                        kind,
                        condition,
                        format_str,
                        args,
                    } => {
                        let condition = condition.apply_values_ptr(
                            ff_values_ptr,
                            ff_len,
                            comb_values_ptr,
                            comb_len,
                            use_4state,
                        );
                        let args: Vec<_> = args
                            .iter()
                            .map(|e| {
                                e.apply_values_ptr(
                                    ff_values_ptr,
                                    ff_len,
                                    comb_values_ptr,
                                    comb_len,
                                    use_4state,
                                )
                            })
                            .collect();
                        Statement::SystemFunctionCall(SystemFunctionCall::Assert {
                            kind: *kind,
                            condition,
                            format_str: format_str.clone(),
                            args,
                        })
                    }
                    ProtoSystemFunctionCall::Write { format_str, args } => {
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
                        Statement::SystemFunctionCall(SystemFunctionCall::Write {
                            format_str: format_str.clone(),
                            args,
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
                    Statement::Compiled(CompiledStmt {
                        artifact: Arc::clone(&x.artifact),
                        ff: adjusted_ff,
                        comb: adjusted_comb,
                        log_buf: std::ptr::null_mut(),
                        ff_delta: x.ff_delta_bytes,
                    })
                }
                ProtoStatement::For(x) => {
                    let var_ptr = if x.var_offset.is_ff() {
                        ff_values_ptr.offset(x.var_offset.raw())
                    } else {
                        comb_values_ptr.offset(x.var_offset.raw())
                    };
                    let body = x
                        .body
                        .iter()
                        .map(|s| {
                            s.apply_values_ptr(
                                ff_values_ptr,
                                ff_len,
                                comb_values_ptr,
                                comb_len,
                                use_4state,
                            )
                        })
                        .collect();
                    let convert_bound = |b: &ProtoForBound| -> RuntimeForBound {
                        match b {
                            ProtoForBound::Const(v) => RuntimeForBound::Const(*v),
                            ProtoForBound::Dynamic(proto_expr) => {
                                RuntimeForBound::Dynamic(Box::new(proto_expr.apply_values_ptr(
                                    ff_values_ptr,
                                    ff_len,
                                    comb_values_ptr,
                                    comb_len,
                                    use_4state,
                                )))
                            }
                        }
                    };
                    let range = match &x.range {
                        ProtoForRange::Forward {
                            start,
                            end,
                            inclusive,
                            step,
                        } => RuntimeForRange {
                            start: convert_bound(start),
                            end: convert_bound(end),
                            inclusive: *inclusive,
                            step: *step,
                            op: None,
                            reverse: false,
                        },
                        ProtoForRange::Reverse {
                            start,
                            end,
                            inclusive,
                            step,
                        } => RuntimeForRange {
                            start: convert_bound(start),
                            end: convert_bound(end),
                            inclusive: *inclusive,
                            step: *step,
                            op: None,
                            reverse: true,
                        },
                        ProtoForRange::Stepped {
                            start,
                            end,
                            inclusive,
                            step,
                            op,
                        } => RuntimeForRange {
                            start: convert_bound(start),
                            end: convert_bound(end),
                            inclusive: *inclusive,
                            step: *step,
                            op: Some(*op),
                            reverse: false,
                        },
                    };
                    Statement::For(ForStatement {
                        var_ptr,
                        var_native_bytes: x.var_native_bytes,
                        var_use_4state: use_4state,
                        var_width: x.var_width,
                        var_signed: x.var_signed,
                        range,
                        body,
                    })
                }
                ProtoStatement::SequentialBlock(body) => {
                    let stmts = body
                        .iter()
                        .map(|s| {
                            s.apply_values_ptr(
                                ff_values_ptr,
                                ff_len,
                                comb_values_ptr,
                                comb_len,
                                use_4state,
                            )
                        })
                        .collect();
                    Statement::SequentialBlock(stmts)
                }
                ProtoStatement::Break => Statement::Break,
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
                        ProtoTbMethodKind::FileOpen { path, append } => TbMethodKind::FileOpen {
                            path: path.clone(),
                            append: *append,
                        },
                        ProtoTbMethodKind::FileWrite { format_str, args } => {
                            let args = args
                                .iter()
                                .map(|e| {
                                    e.apply_values_ptr(
                                        ff_values_ptr,
                                        ff_len,
                                        comb_values_ptr,
                                        comb_len,
                                        use_4state,
                                    )
                                })
                                .collect();
                            TbMethodKind::FileWrite {
                                format_str: format_str.clone(),
                                args,
                            }
                        }
                        ProtoTbMethodKind::FileClose => TbMethodKind::FileClose,
                        ProtoTbMethodKind::FileFlush => TbMethodKind::FileFlush,
                        ProtoTbMethodKind::Component { method, args, ret } => {
                            let args = args
                                .iter()
                                .map(|a| match a {
                                    ProtoComponentArg::Str(s) => ComponentArg::Str(s.clone()),
                                    ProtoComponentArg::Expr(e) => {
                                        ComponentArg::Expr(e.apply_values_ptr(
                                            ff_values_ptr,
                                            ff_len,
                                            comb_values_ptr,
                                            comb_len,
                                            use_4state,
                                        ))
                                    }
                                })
                                .collect();
                            TbMethodKind::Component {
                                method: *method,
                                args,
                                ret: *ret,
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
    /// FF current byte offset for the write-log push helper.
    /// Populated by `apply_values_ptr` only when the destination is a
    /// 2-state FF with width ≤ 64.  `None` for wide/4-state paths;
    /// those continue to use only the direct store.
    pub ff_log_offset: Option<u32>,
    /// True when `dst` points to the FF current slot (packed layout).
    /// In packed mode the eval_step skips the write_native_value call —
    /// the FF current slot is owned by `ff_commit_from_log` replay at
    /// cycle end.  False for dual-slot multi-RMW FFs (dst points to
    /// next slot, direct write is the intermediate state for cache
    /// forwarding).
    pub ff_is_packed: bool,
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
            let beg = end + dyn_sel.window - 1;
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
            if !self.ff_is_packed {
                unsafe {
                    write_native_value(
                        self.dst,
                        self.dst_native_bytes,
                        self.dst_use_4state,
                        &current,
                    );
                }
            }
            // dynamic_select RMW log push.  Offset is static (dyn_sel
            // only selects bit ranges within a packed-bitfield FF dst).
            // Wide FFs split into per-word entries; see `emit_ff_log`.
            if let Some(offset) = self.ff_log_offset {
                emit_ff_log(&current, offset, self.dst_native_bytes, self.dst_use_4state);
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
            if !self.ff_is_packed {
                unsafe {
                    write_native_value(
                        self.dst,
                        self.dst_native_bytes,
                        self.dst_use_4state,
                        &current,
                    );
                }
            }
            // select RMW log push.  `current` after assign holds the
            // final merged value the direct store deposited.  Wide FFs
            // split into per-word entries; see `emit_ff_log`.
            if let Some(offset) = self.ff_log_offset {
                emit_ff_log(&current, offset, self.dst_native_bytes, self.dst_use_4state);
            }
        } else {
            let mut value = value;
            value.trunc(self.dst_width);
            if !self.ff_is_packed {
                unsafe {
                    write_native_value(
                        self.dst,
                        self.dst_native_bytes,
                        self.dst_use_4state,
                        &value,
                    );
                }
            }
            // Interpret-path log push.  Mirrors the JIT-side emit at
            // build_binary so both paths produce matching WriteLogEntry
            // sequences.  Narrow FFs (dst_width ≤ 64) emit one payload
            // entry plus an optional 4-state mask entry at `offset + nb`.
            // Wide FFs (dst_width > 64) emit one entry per 8-byte word,
            // with parallel mask entries when use_4state is set.
            if let Some(offset) = self.ff_log_offset {
                emit_ff_log(&value, offset, self.dst_native_bytes, self.dst_use_4state);
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
        // Runtime offset for write-log push:
        //   ff_log_base_current_offset + idx * dst_stride.
        // Compute once here; emit after the matching write.  4-state
        // FFs additionally push a mask_xz entry at `offset + nb`.
        let log_offset = self.ff_log_base_current_offset.map(|base| {
            let runtime = base as isize + self.dst_stride * idx as isize;
            runtime as u32
        });
        let use_4state = self.dst_use_4state;
        // `emit_ff_log` covers wide elements; the prior hand-rolled push tagged
        // the narrow entry `width_class = nb`, which `ff_commit_from_log` drops
        // for nb ∉ {1,2,4,8} — losing every wide dynamic-index FF write.
        let push_log = |current: &Value| {
            if let Some(offset) = log_offset {
                emit_ff_log(current, offset, self.dst_native_bytes, use_4state);
            }
        };
        if let Some(dyn_sel) = &self.dynamic_select {
            let dyn_idx = dyn_sel
                .index_expr
                .eval(mask_cache)
                .to_usize()
                .unwrap_or(0)
                .min(dyn_sel.num_elements.saturating_sub(1));
            let end = dyn_idx * dyn_sel.elem_width;
            let beg = end + dyn_sel.window - 1;
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
            if !self.ff_is_packed {
                unsafe {
                    write_native_value(dst, self.dst_native_bytes, self.dst_use_4state, &current)
                };
            }
            push_log(&current);
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
            if !self.ff_is_packed {
                unsafe {
                    write_native_value(dst, self.dst_native_bytes, self.dst_use_4state, &current)
                };
            }
            push_log(&current);
        } else {
            let mut value = value;
            value.trunc(self.dst_width);
            if !self.ff_is_packed {
                unsafe {
                    write_native_value(dst, self.dst_native_bytes, self.dst_use_4state, &value)
                };
            }
            push_log(&value);
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
                    window: dyn_sel.window,
                    num_elements: dyn_sel.num_elements,
                });

            // FF write log offset = canonical current byte offset.
            // Packed FFs have dst == Ff(current), dual-slot FFs have
            // dst == Ff(next); the canonical offset is always
            // `dst_ff_current_offset`.
            // 4-state FFs emit a second log entry for the mask_xz portion
            // at `dst_ff_current_offset + nb` (see eval_step).
            // Wide FFs (>64 bits) emit one log entry per 8-byte word; the
            // ff_log_offset records the canonical base and eval_step / JIT
            // codegen splits per word.
            let emit_log = self.dst.is_ff();
            let ff_log_offset = if emit_log {
                Some(self.dst_ff_current_offset as u32)
            } else {
                None
            };
            let ff_is_packed = emit_log && (self.dst.raw() == self.dst_ff_current_offset);

            AssignStatement {
                dst,
                dst_width: self.dst_width,
                dst_native_bytes: nb,
                dst_use_4state: use_4state,
                select: self.select,
                dynamic_select,
                rhs_select: self.rhs_select,
                expr,
                ff_log_offset,
                ff_is_packed,
            }
        }
    }
}

#[derive(Clone)]
pub struct IfStatement {
    pub cond: Option<Expression>,
    pub true_side: Vec<Statement>,
    pub false_side: Vec<Statement>,
}

impl IfStatement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) -> ControlFlow {
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
                if x.eval_step(mask_cache) == ControlFlow::Break {
                    return ControlFlow::Break;
                }
            }
        } else {
            for x in &self.false_side {
                if x.eval_step(mask_cache) == ControlFlow::Break {
                    return ControlFlow::Break;
                }
            }
        }
        ControlFlow::Continue
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
}

fn extract_display_args(
    context: &mut Context,
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

fn factor_comptime(factor: &air::Factor) -> Option<&veryl_analyzer::ir::Comptime> {
    match factor {
        air::Factor::Value(x) => Some(x),
        // A const parameter reference whose comptime value has already
        // been folded in by the analyzer.
        air::Factor::Variable(_, _, _, x) if x.is_const && x.evaluated => Some(x),
        _ => None,
    }
}

fn is_string_literal(expr: &air::Expression) -> bool {
    if let air::Expression::Term(factor) = expr
        && let Some(comptime) = factor_comptime(factor.as_ref())
    {
        return comptime.r#type.kind == TypeKind::String;
    }
    false
}

fn extract_string_value(expr: &air::Expression) -> Option<String> {
    if let air::Expression::Term(factor) = expr
        && let Some(comptime) = factor_comptime(factor.as_ref())
        && let ValueVariant::Numeric(value) = &comptime.value
    {
        return veryl_analyzer::value::byte_value_to_string(value);
    }
    None
}

/// Strips exactly the delimiting quotes of a string-literal token (one
/// leading, one trailing); quote characters inside the content survive.
fn strip_string_quotes(s: &str) -> &str {
    let s = s.strip_prefix('"').unwrap_or(s);
    s.strip_suffix('"').unwrap_or(s)
}

impl Conv<&air::Statement> for Vec<ProtoStatement> {
    fn conv(context: &mut Context, src: &air::Statement) -> Result<Self, SimulatorError> {
        let mut result = match src {
            air::Statement::Assign(x) => Conv::conv(context, x)?,
            air::Statement::FunctionCall(x) => Conv::conv(context, x.as_ref())?,
            air::Statement::If(x) => {
                let x: ProtoIfStatement = Conv::conv(context, x)?;
                vec![ProtoStatement::If(x)]
            }
            air::Statement::Case(c) => {
                let lowered = c.lower_to_nested_if();
                let mut out: Vec<ProtoStatement> = Vec::new();
                for s in &lowered {
                    let v: Vec<ProtoStatement> = Conv::conv(context, s)?;
                    out.extend(v);
                }
                out
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
                SystemFunctionKind::Write(inputs) => {
                    let (format_str, exprs) = extract_display_args(context, inputs).unwrap();
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Write {
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
                SystemFunctionKind::Assert { kind, cond, args } => {
                    let condition: ProtoExpression = Conv::conv(context, &cond.0)?;
                    let (format_str, exprs) = extract_display_args(context, args).unwrap();
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Assert {
                            kind: *kind,
                            condition,
                            format_str,
                            args: exprs,
                        },
                    )]
                }
                SystemFunctionKind::Finish => {
                    vec![ProtoStatement::SystemFunctionCall(
                        ProtoSystemFunctionCall::Finish,
                    )]
                }
                _ => {
                    return Err(SimulatorError::unsupported_description(&x.comptime.token));
                }
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
                            Some(Conv::conv(context, expr.as_ref())?)
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
                    air::TbMethod::FileOpen { name, append } => {
                        let path = extract_string_value(&name.0)
                            .ok_or_else(|| {
                                SimulatorError::unsupported_description(&name.0.token_range())
                            })?
                            .trim_matches('"')
                            .to_string();
                        ProtoTbMethodKind::FileOpen {
                            path,
                            append: *append,
                        }
                    }
                    air::TbMethod::FileWrite { args } => {
                        let (format_str, exprs) =
                            extract_display_args(context, args).ok_or_else(|| {
                                let token =
                                    args.first().map(|a| a.0.token_range()).unwrap_or_default();
                                SimulatorError::unsupported_description(&token)
                            })?;
                        ProtoTbMethodKind::FileWrite {
                            format_str,
                            args: exprs,
                        }
                    }
                    air::TbMethod::FileClose => ProtoTbMethodKind::FileClose,
                    air::TbMethod::FileFlush => ProtoTbMethodKind::FileFlush,
                    air::TbMethod::Component { method, args } => {
                        let args = args
                            .iter()
                            .map(|a| {
                                if is_string_literal(&a.0)
                                    && let Some(s) = extract_string_value(&a.0)
                                {
                                    Ok(ProtoComponentArg::Str(strip_string_quotes(&s).to_string()))
                                } else {
                                    let expr: ProtoExpression = Conv::conv(context, &a.0)?;
                                    Ok(ProtoComponentArg::Expr(expr))
                                }
                            })
                            .collect::<Result<Vec<_>, SimulatorError>>()?;
                        let ret = match &x.ret {
                            None => None,
                            Some(dst) => {
                                if dst.index.0.is_empty()
                                    && dst.select.0.is_empty()
                                    && dst.select.1.is_none()
                                {
                                    let check = if let Some(w) = x.ret_width {
                                        RetWidthCheck::Exact(w)
                                    } else if x.ret_strict {
                                        RetWidthCheck::Max64
                                    } else {
                                        RetWidthCheck::Dst
                                    };
                                    Some((dst.id, check))
                                } else {
                                    // Only a plain variable can receive a
                                    // method return value.
                                    return Err(SimulatorError::unsupported_description(
                                        &dst.token,
                                    ));
                                }
                            }
                        };
                        ProtoTbMethodKind::Component {
                            method: *method,
                            args,
                            ret,
                        }
                    }
                };
                vec![ProtoStatement::TbMethodCall {
                    inst: x.inst,
                    method,
                }]
            }
            air::Statement::For(x) => {
                let scope = context.scope();
                let meta = scope
                    .variable_meta
                    .get(&x.var_id)
                    .ok_or_else(|| SimulatorError::unsupported_description(&x.token))?;
                let var_offset = meta.elements[0].current;
                let var_width = meta.width;
                let var_native_bytes = meta.native_bytes;
                let var_signed = x.var_type.signed;

                let mut body = vec![];
                for stmt in &x.body {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    body.extend(stmts);
                }

                let token = x.token;
                let resolve_bound = |b: &air::ForBound,
                                     ctx: &mut Context|
                 -> Result<ProtoForBound, SimulatorError> {
                    match b {
                        air::ForBound::Const(v) => Ok(ProtoForBound::Const(*v as u64)),
                        air::ForBound::Expression(expr) => {
                            // Must not fold via eval_value: the analyzer's
                            // Context holds the last-tracked value of any
                            // mutable variable in `expr`, not its value at
                            // each runtime iteration of an enclosing loop.
                            let proto_expr: ProtoExpression = Conv::conv(ctx, expr.as_ref())
                                .map_err(|_| SimulatorError::unsupported_description(&token))?;
                            Ok(ProtoForBound::Dynamic(proto_expr))
                        }
                    }
                };
                let range = match &x.range {
                    air::ForRange::Forward {
                        start,
                        end,
                        inclusive,
                        step,
                    } => ProtoForRange::Forward {
                        start: resolve_bound(start, context)?,
                        end: resolve_bound(end, context)?,
                        inclusive: *inclusive,
                        step: *step as u64,
                    },
                    air::ForRange::Reverse {
                        start,
                        end,
                        inclusive,
                        step,
                    } => ProtoForRange::Reverse {
                        start: resolve_bound(start, context)?,
                        end: resolve_bound(end, context)?,
                        inclusive: *inclusive,
                        step: *step as u64,
                    },
                    air::ForRange::Stepped {
                        start,
                        end,
                        inclusive,
                        step,
                        op,
                    } => ProtoForRange::Stepped {
                        start: resolve_bound(start, context)?,
                        end: resolve_bound(end, context)?,
                        inclusive: *inclusive,
                        step: *step as u64,
                        op: *op,
                    },
                };

                vec![ProtoStatement::For(ProtoForStatement {
                    var_offset,
                    var_width,
                    var_native_bytes,
                    var_signed,
                    range,
                    body,
                })]
            }
            air::Statement::Unsupported(token) => {
                return Err(SimulatorError::unsupported_description(token));
            }
            air::Statement::Break => vec![ProtoStatement::Break],
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
    fn conv(context: &mut Context, src: &air::AssignStatement) -> Result<Self, SimulatorError> {
        let in_initial = context.in_initial;

        // Whole-array assignment (`assign out = arr;`): analyzer emits dst
        // with empty index but non-empty array shape, which the single-stmt
        // conv path can't address. Expand to N element-wise assigns.
        if src.dst.len() == 1 {
            let dst0 = &src.dst[0];
            if dst0.index.dimension() == 0 {
                let dst_array = {
                    let scope = context.scope();
                    scope
                        .variable_meta
                        .get(&dst0.id)
                        .map(|m| m.r#type.array.clone())
                };
                if let Some(arr_shape) = dst_array
                    && !arr_shape.is_empty()
                    && let air::Expression::Term(factor) = &src.expr
                    && let air::Factor::Variable(_, vidx, _, _) = factor.as_ref()
                    && vidx.dimension() == 0
                {
                    let total: usize = arr_shape.iter().map(|d| d.unwrap_or(1)).product();
                    let mut result = Vec::with_capacity(total);
                    for i in 0..total {
                        let elem_idx = air::VarIndex::from_index(i, &arr_shape);
                        let mut new_dst = dst0.clone();
                        new_dst.index = elem_idx.clone();
                        let mut new_expr = src.expr.clone();
                        if let air::Expression::Term(ref mut fbox) = new_expr
                            && let air::Factor::Variable(_, vidx2, _, _) = fbox.as_mut()
                        {
                            *vidx2 = elem_idx;
                        }
                        let element_assign = air::AssignStatement {
                            dst: vec![new_dst],
                            width: src.width,
                            expr: new_expr,
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
            }
        }

        if matches!(src.expr, air::Expression::ArrayLiteral(..)) {
            let dst = &src.dst[0];
            let scope = context.scope();
            let meta = scope.variable_meta.get(&dst.id).unwrap();
            let dst_type = meta.r#type.clone();
            let mut expr_clone = src.expr.clone();

            let array_exprs = eval_array_literal(
                &mut scope.analyzer_context,
                Some(&dst_type.array),
                Some(dst_type.width()),
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

        let mut expr: ProtoExpression = Conv::conv(context, &src.expr)?;
        // Expand an unsized fill literal (`{a, b} = '1`) to the concat
        // width before the windows slice it; a width-0 sentinel sliced by
        // a window reads as zero.
        size_fill_literal_rhs(&mut expr, None, None, src.width.unwrap());

        let mut result = Vec::new();
        let mut remaining = src.width.unwrap();

        for dst in &src.dst {
            let id = dst.id;
            let (select, need_dynamic, const_index, width_shape, kind_width) = {
                let scope = context.scope();
                let meta = scope.variable_meta.get(&id).unwrap();

                let select = if !dst.select.is_empty() {
                    dst.select
                        .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
                } else {
                    None
                };
                // A runtime (non-constant) bit-select destination uses the
                // dynamic-select path, which writes only the selected window.
                let need_dynamic = !dst.select.is_empty() && !dst.select.is_const();
                let const_index = if dst.index.is_const() {
                    dst.index.eval_value(&mut scope.analyzer_context)
                } else {
                    None
                };
                let width_shape = meta.r#type.width().clone();
                let kind_width = meta.r#type.kind.width().unwrap_or(1);
                (select, need_dynamic, const_index, width_shape, kind_width)
            };

            let dynamic_select = if need_dynamic {
                Some(build_dynamic_bit_select(
                    context,
                    &width_shape,
                    &dst.select,
                    kind_width,
                )?)
            } else {
                None
            };

            let dst_elem_width = if let Some(d) = &dynamic_select {
                d.window
            } else if let Some((beg, end)) = select {
                beg - end + 1
            } else {
                dst.comptime.r#type.total_width().unwrap()
            };

            let rhs_select = Some(msb_first_window(&mut remaining, dst_elem_width));

            let scope = context.scope();
            let meta = scope.variable_meta.get(&id).unwrap();
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
                    dynamic_select,
                    rhs_select,
                    expr: expr.clone(),
                    dst_ff_current_offset: element.current_offset(),
                    token: src.token,
                }));
            } else {
                let array_shape = meta.r#type.array.clone();
                let dyn_info = meta
                    .dynamic_index_info()
                    .ok_or_else(|| SimulatorError::unsupported_description(&src.token))?;
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
                    dynamic_select,
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

/// Push FF write-log entries for `value` at canonical `base_offset`.
/// Narrow FFs emit a payload (+ optional mask in 4-state) at `base_offset`
/// and `base_offset + nb`.  Wide FFs split into per-word entries that
/// `ff_commit_from_log` reassembles.
///
/// SAFETY: requires an `EVENT_WRITE_LOG` buffer to be installed; no-op
/// otherwise.
fn emit_ff_log(value: &Value, base_offset: u32, nb: usize, use_4state: bool) {
    let nb_u16 = nb as u16;
    let nb_u32 = nb as u32;
    if nb <= 8 {
        let payload = value.to_u64().unwrap_or(0);
        unsafe {
            event_write_log_push_static(base_offset, payload, nb_u16);
            if use_4state {
                let mask = value.mask_xz_u128() as u64;
                event_write_log_push_static(base_offset + nb_u32, mask, nb_u16);
            }
        }
        return;
    }
    // Wide: contiguous byte buffer per side, split into wide entries
    // of at most WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES.
    let n_words = nb / 8;
    let payload_digits: Vec<u64> = match value {
        Value::U64(v) => vec![v.payload],
        Value::BigUint(v) => v.payload.to_u64_digits(),
    };
    let mut payload_bytes: Vec<u8> = Vec::with_capacity(n_words * 8);
    for i in 0..n_words {
        let p = payload_digits.get(i).copied().unwrap_or(0);
        payload_bytes.extend_from_slice(&p.to_le_bytes());
    }
    let mut written: usize = 0;
    while written < nb {
        let chunk = std::cmp::min(WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES, nb - written);
        unsafe {
            event_write_log_push_wide(
                base_offset + written as u32,
                payload_bytes.as_ptr().add(written),
                chunk,
            );
        }
        written += chunk;
    }
    if use_4state {
        let mask_digits: Vec<u64> = match value {
            Value::U64(v) => vec![v.mask_xz],
            Value::BigUint(v) => v.mask_xz.to_u64_digits(),
        };
        let mut mask_bytes: Vec<u8> = Vec::with_capacity(n_words * 8);
        for i in 0..n_words {
            let m = mask_digits.get(i).copied().unwrap_or(0);
            mask_bytes.extend_from_slice(&m.to_le_bytes());
        }
        let mut written: usize = 0;
        while written < nb {
            let chunk = std::cmp::min(WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES, nb - written);
            unsafe {
                event_write_log_push_wide(
                    base_offset + nb_u32 + written as u32,
                    mask_bytes.as_ptr().add(written),
                    chunk,
                );
            }
            written += chunk;
        }
    }
}

/// Duplicate FF assigns in initial blocks to also write the `next`
/// slot so the value persists across ff_swap (same pattern as
/// $readmemh dual write).
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

/// Size an unsized all-bit fill literal (`'0`/`'1`/`'x`/`'z`) on an
/// assignment RHS to the destination width.
///
/// The analyzer leaves these literals unsized (`width` 0) and lets the
/// assignment context resolve the width; the interpret path fills them
/// lazily at `assign`/`trunc` time.  The JIT backends (Cranelift / AOT-C)
/// materialize the value eagerly, so an unsized sentinel would be filled to
/// width 0 — all zeros — silently dropping the value (e.g. `f.a = '1` reads
/// back 0).  Expanding here keeps every backend consistent.
pub(crate) fn size_fill_literal_rhs(
    expr: &mut ProtoExpression,
    select: Option<(usize, usize)>,
    dyn_window: Option<usize>,
    dst_width: usize,
) {
    let ProtoExpression::Value {
        value,
        width,
        expr_context,
    } = expr
    else {
        return;
    };
    // Only the unsized fill sentinels have width 0; sized literals
    // (`1'b1`, `8'hff`, …) already carry their width.
    if value.width() != 0 {
        return;
    }
    // Target is the number of bits actually written: the dynamic-select
    // window or the static bit-select width for a partial write, else the
    // full destination width.
    let target = dyn_window
        .or_else(|| select.map(|(beg, end)| beg - end + 1))
        .unwrap_or(dst_width);
    if target == 0 {
        return;
    }
    *value = value.expand(target, false).into_owned();
    *width = target;
    expr_context.width = target;
}

/// MSB-first concat-destructure window: take `elem_width` bits off the
/// top of the `remaining` unassigned RHS bits, as an inclusive (msb, lsb)
/// rhs_select window.
pub(crate) fn msb_first_window(remaining: &mut usize, elem_width: usize) -> (usize, usize) {
    *remaining -= elem_width;
    (*remaining + elem_width - 1, *remaining)
}

impl Conv<&air::AssignStatement> for ProtoStatement {
    fn conv(context: &mut Context, src: &air::AssignStatement) -> Result<Self, SimulatorError> {
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
            let width_shape = meta.r#type.width().clone();
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
            let index = meta
                .r#type
                .array
                .calc_index(&idx_vals)
                .ok_or_else(|| SimulatorError::unsupported_description(&dst.token))?;
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

            let mut expr: ProtoExpression = Conv::conv(context, &src.expr)?;
            size_fill_literal_rhs(
                &mut expr,
                select,
                dynamic_select.as_ref().map(|d| d.window),
                dst_width,
            );

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
            let dyn_info = meta
                .dynamic_index_info()
                .ok_or_else(|| SimulatorError::unsupported_description(&src.token))?;
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
            let mut expr: ProtoExpression = Conv::conv(context, &src.expr)?;
            size_fill_literal_rhs(
                &mut expr,
                select,
                dynamic_select.as_ref().map(|d| d.window),
                dst_width,
            );

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
    fn conv(context: &mut Context, src: &air::AssignStatement) -> Result<Self, SimulatorError> {
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
            let width_shape = meta.r#type.width().clone();
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

        let mut expr: ProtoExpression = Conv::conv(context, &src.expr)?;
        size_fill_literal_rhs(
            &mut expr,
            select,
            dynamic_select.as_ref().map(|d| d.window),
            dst_width,
        );

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
    fn conv(context: &mut Context, src: &air::IfStatement) -> Result<Self, SimulatorError> {
        let cond: ProtoExpression = Conv::conv(context, &src.cond)?;

        // A function call in the condition inlined its body into
        // `pending_statements` (these compute the condition). Capture it so the
        // per-statement drain below can't pull it into a branch, past the
        // condition read — `if f(x) {…}` would then read an uninitialised return
        // slot and never take. Restored before the `if`.
        let cond_pending = std::mem::take(&mut context.pending_statements);

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

        // Branch conversions drain their own per-statement pending, so anything
        // left now belongs after the condition's: keep condition-pending first.
        let mut pending = cond_pending;
        pending.append(&mut context.pending_statements);
        context.pending_statements = pending;

        Ok(ProtoIfStatement {
            cond: Some(cond),
            true_side,
            false_side,
        })
    }
}

impl Conv<&air::IfResetStatement> for ProtoIfStatement {
    fn conv(context: &mut Context, src: &air::IfResetStatement) -> Result<Self, SimulatorError> {
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
    fn conv(context: &mut Context, src: &FunctionCall) -> Result<Self, SimulatorError> {
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

            // Array argument fed by a bare or constant partial-index
            // variable (`arr` or `arr[0]`): copy per-element. The generic
            // path treats the argument as a scalar and panics at
            // `calc_index().unwrap()` on a whole-array reference.
            let arg_meta_clone = context.scope().variable_meta.get(arg_var_id).cloned();
            if let Some(arg_meta) = arg_meta_clone.as_ref()
                && arg_meta.elements.len() > 1
                && let air::Expression::Term(factor) = expr
                && let air::Factor::Variable(parent_id, index, select, _) = factor.as_ref()
                && select.is_empty()
            {
                let parent_scope = context.scope();
                if let Some(parent_meta) = parent_scope.variable_meta.get(parent_id).cloned() {
                    let base_index = if index.0.is_empty() {
                        Some(0)
                    } else if let Some(idx_vals) =
                        index.eval_value(&mut parent_scope.analyzer_context)
                    {
                        partial_index_base(
                            parent_meta.r#type.array.as_slice(),
                            &idx_vals,
                            arg_meta.elements.len(),
                            parent_meta.elements.len(),
                        )
                    } else {
                        None
                    };

                    if let Some(base) = base_index {
                        for i in 0..arg_meta.elements.len() {
                            let arg_element = &arg_meta.elements[i];
                            let parent_element = &parent_meta.elements[base + i];
                            let parent_expr = ProtoExpression::Variable {
                                var_offset: parent_element.current,
                                select: None,
                                dynamic_select: None,
                                width: arg_meta.width,
                                var_full_width: arg_meta.width,
                                expr_context: ExpressionContext {
                                    width: arg_meta.width,
                                    signed: false,
                                },
                            };
                            result.push(ProtoStatement::Assign(ProtoAssignStatement {
                                dst: arg_element.current,
                                dst_width: arg_meta.width,
                                select: None,
                                dynamic_select: None,
                                rhs_select: None,
                                expr: parent_expr,
                                dst_ff_current_offset: 0, // not FF
                                token: TokenRange::default(),
                            }));
                        }
                        continue;
                    }
                }
            }

            let mut proto_expr: ProtoExpression = Conv::conv(context, expr)?;
            let scope = context.scope();
            let meta = scope.variable_meta.get(arg_var_id).unwrap();
            let element = &meta.elements[0];
            // Size an unsized all-bit literal (`'1` etc.) to the parameter
            // width — there is no assignment statement here to do it.
            size_fill_literal_rhs(&mut proto_expr, None, None, meta.width);
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
                var_full_width: arg_meta.width,
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

fn parse_hex_file(filename: &str, width: usize) -> Vec<AnalyzerValue> {
    let content = match std::fs::read_to_string(filename) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("$readmemh: failed to read '{}': {}", filename, e);
            return vec![];
        }
    };
    parse_hex_content(&content, width)
}

pub fn parse_hex_content(content: &str, width: usize) -> Vec<AnalyzerValue> {
    let bytes = content.as_bytes();
    let mut result: Vec<AnalyzerValue> = Vec::with_capacity(bytes.len() / 4 + 1);
    let mut i = 0usize;
    let len = bytes.len();
    let mut digits: Vec<u8> = Vec::with_capacity(32);

    while i < len {
        let c = bytes[i];
        if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < len {
            let n = bytes[i + 1];
            if n == b'/' {
                i += 2;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if n == b'*' {
                i += 2;
                while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                }
                continue;
            }
        }
        let start = i;
        while i < len {
            let c = bytes[i];
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                break;
            }
            if c == b'/' && i + 1 < len && (bytes[i + 1] == b'/' || bytes[i + 1] == b'*') {
                break;
            }
            i += 1;
        }
        if start == i {
            continue;
        }
        let tok = &bytes[start..i];
        let parsed = if tok.contains(&b'_') {
            digits.clear();
            for &b in tok {
                if b != b'_' {
                    digits.push(b);
                }
            }
            if digits.is_empty() {
                continue;
            }
            std::str::from_utf8(&digits)
                .ok()
                .and_then(|s| u64::from_str_radix(s, 16).ok())
        } else {
            std::str::from_utf8(tok)
                .ok()
                .and_then(|s| u64::from_str_radix(s, 16).ok())
        };
        if let Some(val) = parsed {
            result.push(AnalyzerValue::new(val, width, false));
        }
    }
    result
}
