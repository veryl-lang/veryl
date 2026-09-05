//! Analyzer-IR procedure evaluation for combinational dependency extraction.

use super::region::{
    ArraySpan, BitPartition, NodeKey, PackedSpan, dst_writes, signed_difference,
    translate_position, var_reads,
};
use super::ssa::{
    BranchId, BranchState, Checkpoint, DependencyDag, DependencyDagNode, PathCondition,
    PositionDomain, PositionRelation, SsaStore, VersionId,
};
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, CasePattern, CaseStatement, Expression, Factor, ForBound,
    ForRange, ForStatement, FunctionCall, IfStatement, Module, Op, Statement, SystemFunctionCall,
    SystemFunctionKind, TbMethod, VarIndex, VarPath, VarSelect,
};
use crate::value::Value;
use crate::{HashMap, HashSet};
use std::rc::Rc;
use veryl_parser::token_range::TokenRange;

fn translate_array_span(span: ArraySpan, offset: isize) -> Option<ArraySpan> {
    Some(ArraySpan {
        start: translate_position(span.start, offset)?,
        length: span.length,
    })
}

fn position_domain(array: ArraySpan, packed: PackedSpan) -> PositionDomain {
    PositionDomain {
        array_start: array.start,
        array_length: array.length,
        packed_start: packed.start,
        packed_length: packed.length,
    }
}

fn translate_packed_span(span: PackedSpan, offset: isize) -> Option<PackedSpan> {
    PackedSpan::new(translate_position(span.start, offset)?, span.length)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AffineIndex {
    terms: Vec<(VarId, isize)>,
    constant: isize,
}

impl AffineIndex {
    fn variable(id: VarId) -> Self {
        Self {
            terms: vec![(id, 1)],
            constant: 0,
        }
    }

    fn add_scaled(&mut self, other: &Self, scale: isize) -> Option<()> {
        self.constant = self
            .constant
            .checked_add(other.constant.checked_mul(scale)?)?;
        for &(id, coefficient) in &other.terms {
            let coefficient = coefficient.checked_mul(scale)?;
            match self.terms.binary_search_by_key(&id, |(id, _)| *id) {
                Ok(index) => {
                    self.terms[index].1 = self.terms[index].1.checked_add(coefficient)?;
                }
                Err(index) => self.terms.insert(index, (id, coefficient)),
            }
        }
        self.terms.retain(|(_, coefficient)| *coefficient != 0);
        Some(())
    }

    fn scaled(mut self, scale: isize) -> Option<Self> {
        self.constant = self.constant.checked_mul(scale)?;
        for (_, coefficient) in &mut self.terms {
            *coefficient = coefficient.checked_mul(scale)?;
        }
        Some(self)
    }

    /// Destination position minus source position when both use the same
    /// symbolic coordinates.
    fn destination_offset_from(&self, source: &Self) -> Option<isize> {
        (self.terms == source.terms).then(|| self.constant.checked_sub(source.constant))?
    }
}

fn affine_constant(expression: &Expression, ctx: &mut Context) -> Option<AffineIndex> {
    let constant = expression
        .eval_value(ctx)
        .and_then(|value| value.to_usize())
        .and_then(|value| isize::try_from(value).ok())?;
    Some(AffineIndex {
        terms: Vec::new(),
        constant,
    })
}

fn affine_index(expression: &Expression, ctx: &mut Context) -> Option<AffineIndex> {
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(id, index, select, _) if index.0.is_empty() && select.is_empty() => {
                Some(AffineIndex::variable(*id))
            }
            Factor::Value(_) => affine_constant(expression, ctx),
            _ if expression.comptime().is_const => affine_constant(expression, ctx),
            _ => None,
        },
        Expression::Unary(Op::Add, expression, _) => affine_index(expression, ctx),
        Expression::Unary(Op::Sub, expression, _) => affine_index(expression, ctx)?.scaled(-1),
        Expression::Binary(left, Op::Add | Op::Sub, right, _) => {
            let mut result = affine_index(left, ctx)?;
            let right = affine_index(right, ctx)?;
            result.add_scaled(
                &right,
                if matches!(expression, Expression::Binary(_, Op::Sub, _, _)) {
                    -1
                } else {
                    1
                },
            )?;
            Some(result)
        }
        Expression::Binary(left, Op::Mul, right, _) => {
            let left = affine_index(left, ctx)?;
            let right = affine_index(right, ctx)?;
            if left.terms.is_empty() {
                right.scaled(left.constant)
            } else if right.terms.is_empty() {
                left.scaled(right.constant)
            } else {
                None
            }
        }
        Expression::Binary(left, Op::As, _, _) => affine_index(left, ctx),
        _ if expression.comptime().is_const => affine_constant(expression, ctx),
        _ => None,
    }
}

fn affine_bound(bound: &ForBound, ctx: &mut Context) -> Option<AffineIndex> {
    match bound {
        ForBound::Const(value) => Some(AffineIndex {
            terms: Vec::new(),
            constant: isize::try_from(*value).ok()?,
        }),
        ForBound::Expression(expression) => affine_index(expression, ctx),
    }
}

fn for_range_bounds(range: &ForRange) -> (&ForBound, &ForBound, bool) {
    match range {
        ForRange::Forward {
            start,
            end,
            inclusive,
            ..
        }
        | ForRange::Reverse {
            start,
            end,
            inclusive,
            ..
        }
        | ForRange::Stepped {
            start,
            end,
            inclusive,
            ..
        } => (start, end, *inclusive),
    }
}

fn for_range_has_dynamic_bounds(range: &ForRange) -> bool {
    let (start, end, _) = for_range_bounds(range);
    matches!(start, ForBound::Expression(_)) || matches!(end, ForBound::Expression(_))
}

enum RepeatedProjection {
    Empty,
    Single {
        local_start: usize,
        length: usize,
        output_start: usize,
    },
    Multiple,
}

fn project_repeated_span(
    requested_start: usize,
    requested_length: usize,
    output_start: usize,
    item_length: usize,
    repeat: usize,
) -> RepeatedProjection {
    let Some(output_length) = item_length.checked_mul(repeat) else {
        return RepeatedProjection::Multiple;
    };
    let Some(requested_end) = requested_start.checked_add(requested_length) else {
        return RepeatedProjection::Multiple;
    };
    let Some(output_end) = output_start.checked_add(output_length) else {
        return RepeatedProjection::Multiple;
    };
    let overlap_start = requested_start.max(output_start);
    let overlap_end = requested_end.min(output_end);
    if item_length == 0 || overlap_start >= overlap_end {
        return RepeatedProjection::Empty;
    }
    let relative_start = overlap_start - output_start;
    let relative_end = overlap_end - output_start;
    let first = relative_start / item_length;
    let last = (relative_end - 1) / item_length;
    if first != last {
        return RepeatedProjection::Multiple;
    }
    RepeatedProjection::Single {
        local_start: relative_start % item_length,
        length: overlap_end - overlap_start,
        output_start: output_start + first * item_length,
    }
}

#[cfg(test)]
use std::cell::Cell;

#[derive(Clone)]
struct CallResult {
    region_groups: Vec<(ArraySpan, Vec<(PackedSpan, VersionId)>)>,
    opaque_sources: Vec<VersionId>,
}

// Region-split writes query one RHS several times, but a function call in that
// RHS is one procedural evaluation. `None` is an invocation barrier: temporary
// call nodes in a cloned callee body must never enter the caller's cache.
#[derive(Default)]
struct EvaluationCache {
    calls: HashMap<*const FunctionCall, CallResult>,
    expression_branches: HashMap<*const Expression, BranchId>,
}

type CallCache = Option<EvaluationCache>;

// Module and interface storage is shared by every call. Function-owned
// storage is automatic, so its SSA identity also includes the invocation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SsaKey {
    node: NodeKey,
    call_frame: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcedureFlow {
    Continue,
    Break,
    Return,
}

struct FlowResult {
    flow: ProcedureFlow,
    continuation_controls: Vec<VersionId>,
}

impl FlowResult {
    fn new(flow: ProcedureFlow) -> Self {
        Self {
            flow,
            continuation_controls: Vec::new(),
        }
    }
}

struct FunctionFlow {
    return_id: Option<VarId>,
    checkpoint: Checkpoint,
    returns: Vec<BranchState<SsaKey>>,
}

struct LoopFlow {
    checkpoint: Checkpoint,
    breaks: Vec<FlowState>,
}

struct FlowState {
    state: BranchState<SsaKey>,
    condition: PathCondition,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FunctionSummaryKey {
    id: VarId,
    index: Option<Vec<usize>>,
}

#[derive(Clone)]
struct FunctionSummary {
    arg_map: HashMap<VarPath, VarId>,
    graph: Rc<DependencyDag<SsaKey>>,
    result: FunctionResultSummary,
    writes: Vec<(NodeKey, Option<usize>)>,
    opaque_sources: Vec<NodeKey>,
    status: AnalysisStatus,
}

enum FunctionSummaryLookup {
    Ready(Rc<FunctionSummary>),
    Recursive,
    Missing,
}

type FunctionResultSummary = Vec<(ArraySpan, Vec<(PackedSpan, Option<usize>)>)>;

pub(super) struct FunctionSummaries<'a> {
    pub(super) tracing: bool,
    module: &'a Module,
    bit_part: &'a BitPartition,
    summaries: HashMap<FunctionSummaryKey, Option<Rc<FunctionSummary>>>,
    contexts: Vec<ProcedureContext>,
    module_scope_ids: Rc<HashSet<VarId>>,
}

/// Reusable module-local evaluation context for independent procedural
/// declarations. Its large variable and function maps are built once per
/// active analysis context; SSA and control-flow state remain per analysis.
pub(super) struct ProcedureContext {
    ctx: Option<Context>,
    module_scope_ids: Rc<HashSet<VarId>>,
    summary_scratch: bool,
}

impl ProcedureContext {
    pub(super) fn new(module: &Module) -> Self {
        let mut ctx = Context::default();
        ctx.variables = module.variables.clone();
        ctx.variables.extend(module.interface_members.clone());
        ctx.functions = module.functions.clone();
        let module_scope_ids = ctx
            .variables
            .iter()
            .filter_map(|(&id, variable)| {
                matches!(
                    variable.affiliation,
                    crate::symbol::Affiliation::Module | crate::symbol::Affiliation::Interface
                )
                .then_some(id)
            })
            .collect::<HashSet<_>>();
        #[cfg(test)]
        MODULE_CONTEXT_ENTRIES
            .set(MODULE_CONTEXT_ENTRIES.get() + ctx.variables.len() + ctx.functions.len());
        Self {
            ctx: Some(ctx),
            module_scope_ids: Rc::new(module_scope_ids),
            summary_scratch: false,
        }
    }

    fn new_summary(module_scope_ids: Rc<HashSet<VarId>>) -> Self {
        Self {
            ctx: Some(Context::default()),
            module_scope_ids,
            summary_scratch: true,
        }
    }

    fn take(&mut self) -> (Context, Rc<HashSet<VarId>>) {
        (
            self.ctx.take().expect("procedure context is not reentrant"),
            Rc::clone(&self.module_scope_ids),
        )
    }

    fn restore(&mut self, ctx: Context) {
        debug_assert!(self.ctx.is_none());
        self.ctx = Some(ctx);
    }

    fn prepare_summary(&mut self, module: &Module, id: VarId, index: Option<&[usize]>) {
        debug_assert!(self.summary_scratch);
        let ctx = self.ctx.as_mut().expect("summary context is available");
        debug_assert!(ctx.variables.is_empty());
        debug_assert!(ctx.functions.is_empty());

        let Some(function) = module.functions.get(&id) else {
            return;
        };
        let Some(body) = function.get_function(index.unwrap_or_default()) else {
            return;
        };
        let mut ids = body.arg_map.values().copied().collect::<HashSet<_>>();
        ids.extend(body.ret);
        collect_summary_statement_variables(module, &body.statements, &mut ids);
        ctx.variables.reserve(ids.len());
        for id in ids {
            let variable = module
                .interface_members
                .get(&id)
                .or_else(|| module.variables.get(&id));
            if let Some(variable) = variable {
                ctx.variables.insert(id, variable.clone());
            }
        }
        #[cfg(test)]
        MODULE_CONTEXT_ENTRIES.set(MODULE_CONTEXT_ENTRIES.get() + ctx.variables.len());
    }

    fn install_functions(&mut self, functions: HashMap<VarId, crate::ir::Function>) {
        debug_assert!(self.summary_scratch);
        let ctx = self.ctx.as_mut().expect("summary context is available");
        debug_assert!(ctx.functions.is_empty());
        ctx.functions = functions;
    }

    fn take_functions(&mut self) -> HashMap<VarId, crate::ir::Function> {
        debug_assert!(self.summary_scratch);
        let ctx = self.ctx.as_mut().expect("summary context is available");
        std::mem::take(&mut ctx.functions)
    }

    fn clear_summary(&mut self) {
        debug_assert!(self.summary_scratch);
        let ctx = self.ctx.as_mut().expect("summary context is available");
        debug_assert!(ctx.functions.is_empty());
        ctx.variables.clear();
    }
}

fn collect_summary_statement_variables(
    module: &Module,
    statements: &[Statement],
    ids: &mut HashSet<VarId>,
) {
    for statement in statements {
        match statement {
            Statement::Assign(statement) => {
                for destination in &statement.dst {
                    collect_summary_destination_variables(module, destination, ids);
                }
                collect_summary_expression_variables(module, &statement.expr, ids);
            }
            Statement::If(statement) => {
                collect_summary_expression_variables(module, &statement.cond, ids);
                collect_summary_statement_variables(module, &statement.true_side, ids);
                collect_summary_statement_variables(module, &statement.false_side, ids);
            }
            Statement::IfReset(statement) => {
                collect_summary_statement_variables(module, &statement.true_side, ids);
                collect_summary_statement_variables(module, &statement.false_side, ids);
            }
            Statement::Case(statement) => {
                collect_summary_expression_variables(module, &statement.case_target, ids);
                for arm in &statement.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            CasePattern::Eq(expression) => {
                                collect_summary_expression_variables(module, expression, ids);
                            }
                            CasePattern::Range { lo, hi, .. } => {
                                collect_summary_expression_variables(module, lo, ids);
                                collect_summary_expression_variables(module, hi, ids);
                            }
                        }
                    }
                    collect_summary_statement_variables(module, &arm.body, ids);
                }
                collect_summary_statement_variables(module, &statement.default, ids);
            }
            Statement::For(statement) => {
                ids.insert(statement.var_id);
                let (start, end) = match &statement.range {
                    ForRange::Forward { start, end, .. }
                    | ForRange::Reverse { start, end, .. }
                    | ForRange::Stepped { start, end, .. } => (start, end),
                };
                for bound in [start, end] {
                    if let ForBound::Expression(expression) = bound {
                        collect_summary_expression_variables(module, expression, ids);
                    }
                }
                collect_summary_statement_variables(module, &statement.body, ids);
            }
            Statement::FunctionCall(call) => {
                collect_summary_call_variables(module, call, ids);
            }
            Statement::SystemFunctionCall(call) => {
                collect_summary_system_call_variables(module, call, ids);
            }
            Statement::TbMethodCall(call) => {
                if let Some(destination) = &call.ret {
                    collect_summary_destination_variables(module, destination, ids);
                }
                match &call.method {
                    TbMethod::ClockNext { count, period } => {
                        if let Some(count) = count {
                            collect_summary_expression_variables(module, count, ids);
                        }
                        if let Some(period) = period {
                            collect_summary_expression_variables(module, period, ids);
                        }
                    }
                    TbMethod::ResetAssert { duration, .. } => {
                        if let Some(duration) = duration {
                            collect_summary_expression_variables(module, duration, ids);
                        }
                    }
                    TbMethod::FileOpen { name, .. } => {
                        collect_summary_expression_variables(module, &name.0, ids);
                    }
                    TbMethod::FileWrite { args } | TbMethod::Component { args, .. } => {
                        for argument in args {
                            collect_summary_expression_variables(module, &argument.0, ids);
                        }
                    }
                    TbMethod::RandomSeed { value } => {
                        collect_summary_expression_variables(module, value, ids);
                    }
                    TbMethod::RandomGetRange { min, max, .. } => {
                        collect_summary_expression_variables(module, min, ids);
                        collect_summary_expression_variables(module, max, ids);
                    }
                    TbMethod::FileClose
                    | TbMethod::FileFlush
                    | TbMethod::RandomGet { .. }
                    | TbMethod::RandomGetSeed => {}
                }
            }
            Statement::Break | Statement::Unsupported(_) | Statement::Null => {}
        }
    }
}

fn collect_summary_expression_variables(
    module: &Module,
    expression: &Expression,
    ids: &mut HashSet<VarId>,
) {
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(id, index, select, _) => {
                ids.insert(*id);
                collect_summary_index_variables(module, index, ids);
                collect_summary_select_variables(module, select, ids);
            }
            Factor::HierVariable(reference) => {
                collect_summary_index_variables(module, &reference.index, ids);
                collect_summary_select_variables(module, &reference.select, ids);
            }
            Factor::FunctionCall(call) => collect_summary_call_variables(module, call, ids),
            Factor::SystemFunctionCall(call) => {
                collect_summary_system_call_variables(module, call, ids);
            }
            Factor::Value(_) | Factor::Anonymous(_) | Factor::Unknown(_) => {}
        },
        Expression::Unary(_, expression, _) => {
            collect_summary_expression_variables(module, expression, ids);
        }
        Expression::Binary(left, _, right, _) => {
            collect_summary_expression_variables(module, left, ids);
            collect_summary_expression_variables(module, right, ids);
        }
        Expression::Ternary(condition, left, right, _) => {
            collect_summary_expression_variables(module, condition, ids);
            collect_summary_expression_variables(module, left, ids);
            collect_summary_expression_variables(module, right, ids);
        }
        Expression::Concatenation(parts, _) => {
            for (part, repeat) in parts {
                collect_summary_expression_variables(module, part, ids);
                if let Some(repeat) = repeat {
                    collect_summary_expression_variables(module, repeat, ids);
                }
            }
        }
        Expression::ArrayLiteral(items, _) => {
            for item in items {
                match item {
                    ArrayLiteralItem::Value(value, repeat) => {
                        collect_summary_expression_variables(module, value, ids);
                        if let Some(repeat) = repeat {
                            collect_summary_expression_variables(module, repeat, ids);
                        }
                    }
                    ArrayLiteralItem::Defaul(value) => {
                        collect_summary_expression_variables(module, value, ids);
                    }
                }
            }
        }
        Expression::StructConstructor(_, members, _) => {
            for (_, value) in members {
                collect_summary_expression_variables(module, value, ids);
            }
        }
    }
}

fn collect_summary_call_variables(module: &Module, call: &FunctionCall, ids: &mut HashSet<VarId>) {
    for input in call.inputs.values() {
        collect_summary_expression_variables(module, input, ids);
    }
    for outputs in call.outputs.values() {
        for destination in outputs {
            collect_summary_destination_variables(module, destination, ids);
        }
    }
    if let Some(body) = module
        .functions
        .get(&call.id)
        .and_then(|function| function.get_function(call.index.as_deref().unwrap_or_default()))
    {
        ids.extend(body.arg_map.values().copied());
        ids.extend(body.ret);
    }
}

fn collect_summary_system_call_variables(
    module: &Module,
    call: &SystemFunctionCall,
    ids: &mut HashSet<VarId>,
) {
    match &call.kind {
        SystemFunctionKind::Bits(input)
        | SystemFunctionKind::Size(input)
        | SystemFunctionKind::Clog2(input)
        | SystemFunctionKind::Onehot(input)
        | SystemFunctionKind::Signed(input)
        | SystemFunctionKind::Unsigned(input) => {
            collect_summary_expression_variables(module, &input.0, ids);
        }
        SystemFunctionKind::Readmemh(input, output) => {
            collect_summary_expression_variables(module, &input.0, ids);
            for destination in &output.0 {
                collect_summary_destination_variables(module, destination, ids);
            }
        }
        SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
            for input in inputs {
                collect_summary_expression_variables(module, &input.0, ids);
            }
        }
        SystemFunctionKind::Assert { cond, args, .. } => {
            collect_summary_expression_variables(module, &cond.0, ids);
            for input in args {
                collect_summary_expression_variables(module, &input.0, ids);
            }
        }
        SystemFunctionKind::Finish => {}
    }
}

fn collect_summary_destination_variables(
    module: &Module,
    destination: &AssignDestination,
    ids: &mut HashSet<VarId>,
) {
    ids.insert(destination.id);
    collect_summary_index_variables(module, &destination.index, ids);
    collect_summary_select_variables(module, &destination.select, ids);
}

fn collect_summary_index_variables(module: &Module, index: &VarIndex, ids: &mut HashSet<VarId>) {
    for expression in &index.0 {
        collect_summary_expression_variables(module, expression, ids);
    }
}

fn collect_summary_select_variables(module: &Module, select: &VarSelect, ids: &mut HashSet<VarId>) {
    for expression in &select.0 {
        collect_summary_expression_variables(module, expression, ids);
    }
    if let Some((_, expression)) = &select.1 {
        collect_summary_expression_variables(module, expression, ids);
    }
}

fn module_scope_ids(module: &Module) -> HashSet<VarId> {
    module
        .variables
        .iter()
        .chain(&module.interface_members)
        .filter_map(|(&id, variable)| {
            matches!(
                variable.affiliation,
                crate::symbol::Affiliation::Module | crate::symbol::Affiliation::Interface
            )
            .then_some(id)
        })
        .collect()
}
impl<'a> FunctionSummaries<'a> {
    pub(super) fn new(module: &'a Module, bit_part: &'a BitPartition) -> Self {
        Self {
            tracing: false,
            module,
            bit_part,
            summaries: HashMap::default(),
            // Most modules never need a function summary. Allocate the
            // baseline scratch context lazily so ordinary declarations keep
            // just the reusable top-level procedure context.
            contexts: Vec::new(),
            module_scope_ids: Rc::new(module_scope_ids(module)),
        }
    }

    fn get(&mut self, call: &FunctionCall, caller_ctx: &mut Context) -> FunctionSummaryLookup {
        let key = FunctionSummaryKey {
            id: call.id,
            index: call.index.clone(),
        };
        if let Some(summary) = self.summaries.get(&key).cloned() {
            return summary.map_or(
                FunctionSummaryLookup::Recursive,
                FunctionSummaryLookup::Ready,
            );
        }
        self.summaries.insert(key.clone(), None);
        let mut context = self
            .contexts
            .pop()
            .unwrap_or_else(|| ProcedureContext::new_summary(Rc::clone(&self.module_scope_ids)));
        context.prepare_summary(self.module, call.id, call.index.as_deref());
        // Function IR is immutable during dependency analysis. Move the one
        // module-wide map down the suspended call chain instead of cloning it
        // into every recursive scratch context, then restore it before the
        // caller resumes.
        context.install_functions(std::mem::take(&mut caller_ctx.functions));
        let summary = ProcedureAnalysis::summarize_function(
            self.module,
            self.bit_part,
            call.id,
            call.index.as_deref(),
            &mut context,
            self,
        )
        .map(Rc::new);
        caller_ctx.functions = context.take_functions();
        context.clear_summary();
        self.contexts.push(context);
        if let Some(summary) = summary {
            self.summaries.insert(key, Some(summary.clone()));
            FunctionSummaryLookup::Ready(summary)
        } else {
            self.summaries.remove(&key);
            FunctionSummaryLookup::Missing
        }
    }
}

#[cfg(test)]
thread_local! {
    static FUNCTION_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_RESULT_VERSIONS: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_RESULT_REGION_PROBES: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_BARRIER_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_SUMMARY_GRAPH_NODES: Cell<usize> = const { Cell::new(0) };
    static MODULE_CONTEXT_ENTRIES: Cell<usize> = const { Cell::new(0) };
    static TRACED_PROCEDURE_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    static WRITE_FOOTPRINT_STATEMENT_VISITS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_function_evaluation_count() {
    FUNCTION_EVALUATIONS.set(0);
    FUNCTION_RESULT_VERSIONS.set(0);
    FUNCTION_RESULT_REGION_PROBES.set(0);
    FUNCTION_BARRIER_EVALUATIONS.set(0);
    FUNCTION_SUMMARY_GRAPH_NODES.set(0);
    MODULE_CONTEXT_ENTRIES.set(0);
    WRITE_FOOTPRINT_STATEMENT_VISITS.set(0);
}

#[cfg(test)]
pub(crate) fn function_evaluation_count() -> usize {
    FUNCTION_EVALUATIONS.get()
}

#[cfg(test)]
pub(crate) fn function_result_version_count() -> usize {
    FUNCTION_RESULT_VERSIONS.get()
}

#[cfg(test)]
pub(crate) fn function_result_region_probe_count() -> usize {
    FUNCTION_RESULT_REGION_PROBES.get()
}

#[cfg(test)]
pub(crate) fn function_barrier_evaluation_count() -> usize {
    FUNCTION_BARRIER_EVALUATIONS.get()
}

#[cfg(test)]
pub(crate) fn function_summary_graph_node_count() -> usize {
    FUNCTION_SUMMARY_GRAPH_NODES.get()
}

#[cfg(test)]
pub(crate) fn write_footprint_statement_visits() -> usize {
    WRITE_FOOTPRINT_STATEMENT_VISITS.get()
}

#[cfg(test)]
pub(crate) fn reset_module_context_entries() {
    MODULE_CONTEXT_ENTRIES.set(0);
}

#[cfg(test)]
pub(crate) fn module_context_entries() -> usize {
    MODULE_CONTEXT_ENTRIES.get()
}

#[cfg(test)]
pub(crate) fn reset_traced_procedure_evaluation_count() {
    TRACED_PROCEDURE_EVALUATIONS.set(0);
}

#[cfg(test)]
pub(crate) fn traced_procedure_evaluation_count() -> usize {
    TRACED_PROCEDURE_EVALUATIONS.get()
}

pub(super) fn analyze<'a>(
    bit_part: &'a BitPartition,
    statements: &[Statement],
    branch_namespace: usize,
    context: &mut ProcedureContext,
    summaries: &mut FunctionSummaries<'a>,
) -> ProcedureResult {
    ProcedureAnalysis::analyze(bit_part, statements, branch_namespace, context, summaries)
}

pub(super) struct ProcedureResult {
    pub(super) graph: DependencyDag<NodeKey>,
    pub(super) destinations: Vec<(NodeKey, Option<usize>)>,
    pub(super) status: AnalysisStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum AnalysisStatus {
    #[default]
    Complete,
    Partial,
    Barrier,
}

impl AnalysisStatus {
    pub(super) fn is_complete(self) -> bool {
        self == Self::Complete
    }

    pub(super) fn is_barrier(self) -> bool {
        self == Self::Barrier
    }
}

#[derive(Default)]
struct ExpressionSources {
    sources: Vec<(VersionId, PositionRelation)>,
}

#[derive(Default)]
struct ProjectionContext {
    destination_index: Option<AffineIndex>,
    destination_array: Option<ArraySpan>,
}

impl ExpressionSources {
    fn whole(versions: Vec<VersionId>) -> Self {
        Self {
            sources: versions
                .into_iter()
                .map(|version| (version, PositionRelation::whole()))
                .collect(),
        }
    }

    fn extend(&mut self, other: Self) {
        self.sources.extend(other.sources);
    }

    fn extend_whole(&mut self, versions: impl IntoIterator<Item = VersionId>) {
        self.sources.extend(
            versions
                .into_iter()
                .map(|version| (version, PositionRelation::whole())),
        );
    }

    fn push(&mut self, version: VersionId, relation: PositionRelation) {
        self.sources.push((version, relation));
    }

    fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    fn translate(&mut self, offset: PositionRelation) {
        for (_, current) in &mut self.sources {
            *current = current.compose(offset);
        }
    }

    fn forget_array_position(&mut self) {
        for (_, relation) in &mut self.sources {
            relation.array = None;
        }
    }

    fn forget_packed_position(&mut self) {
        for (_, relation) in &mut self.sources {
            relation.packed = None;
        }
    }

    fn widen_all(&mut self) {
        for (_, relation) in &mut self.sources {
            *relation = PositionRelation::whole();
        }
    }

    fn normalize(&mut self) {
        self.sources.sort_unstable();
        self.sources.dedup();
    }
}

pub(super) struct ExpressionAnalysis<'a, 's> {
    inner: Option<ProcedureAnalysis<'a, 's>>,
}

impl<'a, 's> ExpressionAnalysis<'a, 's> {
    pub(super) fn new(
        bit_part: &'a BitPartition,
        context: &mut ProcedureContext,
        summaries: &'s mut FunctionSummaries<'a>,
    ) -> Self {
        let (mut ctx, module_scope_ids) = context.take();
        ctx.begin_analysis_transaction();
        let mut inner =
            ProcedureAnalysis::from_context(bit_part, summaries.module, ctx, module_scope_ids);
        inner.tracing = summaries.tracing;
        inner.summaries = Some(summaries);
        Self { inner: Some(inner) }
    }

    fn inner(&mut self) -> &mut ProcedureAnalysis<'a, 's> {
        self.inner.as_mut().expect("expression analysis is active")
    }

    pub(super) fn eval(&mut self, expression: &Expression) -> Vec<RegionSource> {
        self.inner().use_expression_namespace(expression);
        self.inner().eval_expression_sources(expression)
    }

    pub(super) fn eval_region(
        &mut self,
        expression: &Expression,
        array: ArraySpan,
        packed: PackedSpan,
        context_width: usize,
    ) -> DependencyDag<NodeKey> {
        let inner = self.inner();
        inner.use_expression_namespace(expression);
        let mut sources = inner.eval_expr_requested(expression, array, packed, context_width);
        sources.normalize();
        let value = inner.ssa.related_definition(sources.sources);
        let value = inner.ssa.projected(value, position_domain(array, packed));
        inner.dependency_dag_for_nodes(&[value], inner.module_scope_keys())
    }

    pub(super) fn dependencies(&mut self) -> ProcedureResult {
        let inner = self.inner();
        let (graph, destinations) = inner.dependency_graph();
        ProcedureResult {
            graph,
            destinations,
            status: inner.status,
        }
    }

    pub(super) fn restore(mut self, context: &mut ProcedureContext) {
        let mut inner = self.inner.take().expect("expression analysis is active");
        inner.ctx.rollback_analysis_transaction();
        context.restore(inner.ctx);
    }

    pub(super) fn is_complete(&self) -> bool {
        self.inner
            .as_ref()
            .expect("expression analysis is active")
            .status
            .is_complete()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RegionSource {
    pub(super) key: NodeKey,
    pub(super) offset: Option<(isize, isize)>,
    pub(super) condition: PathCondition,
}

struct ProcedureAnalysis<'a, 's> {
    bit_part: &'a BitPartition,
    module: &'a Module,
    ctx: Context,
    module_scope_ids: Rc<HashSet<VarId>>,
    ssa: SsaStore<SsaKey>,
    written: HashSet<NodeKey>,
    call_caches: Vec<CallCache>,
    call_frames: Vec<usize>,
    next_call_frame: usize,
    receiver_indices: Vec<Option<VarIndex>>,
    function_flows: Vec<FunctionFlow>,
    loop_flows: Vec<LoopFlow>,
    path_condition: PathCondition,
    branch_namespace: usize,
    next_branch: usize,
    status: AnalysisStatus,
    summaries: Option<&'s mut FunctionSummaries<'a>>,
    causal_write_keys: Vec<NodeKey>,
    tracing: bool,
    active_assignment: Option<TokenRange>,
}

impl<'a, 's> ProcedureAnalysis<'a, 's> {
    fn from_context(
        bit_part: &'a BitPartition,
        module: &'a Module,
        ctx: Context,
        module_scope_ids: Rc<HashSet<VarId>>,
    ) -> Self {
        Self {
            bit_part,
            module,
            ctx,
            module_scope_ids,
            ssa: SsaStore::default(),
            written: HashSet::default(),
            call_caches: Vec::new(),
            call_frames: Vec::new(),
            next_call_frame: 0,
            receiver_indices: Vec::new(),
            function_flows: Vec::new(),
            loop_flows: Vec::new(),
            path_condition: PathCondition::default(),
            branch_namespace: 0,
            next_branch: 0,
            status: AnalysisStatus::Complete,
            summaries: None,
            causal_write_keys: Vec::new(),
            tracing: false,
            active_assignment: None,
        }
    }

    fn analyze(
        bit_part: &'a BitPartition,
        statements: &[Statement],
        branch_namespace: usize,
        context: &mut ProcedureContext,
        summaries: &'s mut FunctionSummaries<'a>,
    ) -> ProcedureResult {
        let (mut ctx, module_scope_ids) = context.take();
        ctx.begin_analysis_transaction();
        let mut this = Self::from_context(bit_part, summaries.module, ctx, module_scope_ids);
        this.tracing = summaries.tracing;
        this.summaries = Some(summaries);
        #[cfg(test)]
        if this.tracing {
            TRACED_PROCEDURE_EVALUATIONS.set(TRACED_PROCEDURE_EVALUATIONS.get() + 1);
        }
        this.causal_write_keys = this.process_write_footprint(statements);
        this.branch_namespace = branch_namespace;
        this.eval_block(statements, &[]);
        let (graph, destinations) = this.dependency_graph();
        let result = ProcedureResult {
            graph,
            destinations,
            status: this.status,
        };
        this.ctx.rollback_analysis_transaction();
        context.restore(this.ctx);
        result
    }

    fn eval_expression_sources(&mut self, expression: &Expression) -> Vec<RegionSource> {
        let versions = self.eval_reachable_expr(expression);
        let value = self.ssa.definition(versions);
        let mut sources = self
            .ssa
            .root_source_keys_guarded(value)
            .into_iter()
            .filter_map(|(source, condition)| {
                source.call_frame.is_none().then_some(RegionSource {
                    key: source.node,
                    offset: None,
                    condition,
                })
            })
            .filter(|source| self.is_module_scope_key(source.key))
            .collect::<Vec<_>>();
        sources.sort_unstable_by_key(|source| (source.key, source.condition.clone()));
        sources.dedup_by(|left, right| left.key == right.key && left.condition == right.condition);
        sources
    }

    fn summarize_function(
        module: &'a Module,
        bit_part: &'a BitPartition,
        id: VarId,
        index: Option<&[usize]>,
        context: &mut ProcedureContext,
        summaries: &'s mut FunctionSummaries<'a>,
    ) -> Option<FunctionSummary> {
        let function = module.functions.get(&id)?;
        let body = function.get_function(index.unwrap_or_default())?;
        let formal_ids = body.arg_map.values().copied().collect::<HashSet<_>>();
        let (mut ctx, module_scope_ids) = context.take();
        ctx.begin_analysis_transaction();
        let mut this = Self::from_context(bit_part, module, ctx, module_scope_ids);
        this.tracing = summaries.tracing;
        this.summaries = Some(summaries);
        this.call_caches.push(None);
        this.receiver_indices.push(
            (!function.path.path.0.is_empty())
                .then(|| index.map(concrete_var_index))
                .flatten(),
        );
        this.eval_function_body(&body.statements, body.ret, &[]);
        this.receiver_indices.pop();
        this.call_caches.pop();

        let mut visible_keys = this.module_scope_keys();
        visible_keys.extend(
            formal_ids
                .iter()
                .flat_map(|formal| this.keys_for_id(*formal)),
        );
        visible_keys.sort_unstable();
        visible_keys.dedup();
        let allowed = visible_keys
            .into_iter()
            .map(|node| SsaKey {
                node,
                call_frame: None,
            })
            .collect::<HashSet<_>>();

        let result_versions: Vec<(ArraySpan, Vec<(PackedSpan, VersionId)>)> = body
            .ret
            .map(|ret| this.current_region_groups_for_id(ret))
            .unwrap_or_default();

        let mut destinations = this
            .written
            .iter()
            .copied()
            .filter(|destination| {
                formal_ids.contains(&destination.0) || this.is_module_scope_key(*destination)
            })
            .collect::<Vec<_>>();
        destinations.sort_unstable();
        let write_versions = destinations
            .into_iter()
            .map(|destination| {
                let version = this.read_key(destination);
                (destination, version)
            })
            .collect::<Vec<_>>();

        let mut roots = result_versions
            .iter()
            .flat_map(|(_, regions)| regions.iter().map(|(_, version)| *version))
            .collect::<Vec<_>>();
        roots.extend(write_versions.iter().map(|(_, version)| *version));
        let graph = this.ssa.dependency_dag(&roots, &allowed);
        let graph = Rc::new(graph);
        #[cfg(test)]
        FUNCTION_SUMMARY_GRAPH_NODES.set(FUNCTION_SUMMARY_GRAPH_NODES.get().max(graph.nodes.len()));
        let mut root = graph.roots.iter().copied();
        let result = result_versions
            .into_iter()
            .map(|(array, regions)| {
                let regions = regions
                    .into_iter()
                    .map(|(span, _)| {
                        (
                            span,
                            root.next().expect("every function result has a DAG root"),
                        )
                    })
                    .collect();
                (array, regions)
            })
            .collect();
        let writes = write_versions
            .into_iter()
            .map(|(destination, _)| {
                (
                    destination,
                    root.next().expect("every function write has a DAG root"),
                )
            })
            .collect();
        debug_assert!(root.next().is_none());

        let opaque_sources = if statements_have_unknown(&body.statements) {
            let mut sources = formal_ids
                .into_iter()
                .flat_map(|formal| this.keys_for_id(formal))
                .collect::<Vec<_>>();
            sources.sort_unstable();
            sources.dedup();
            sources
        } else {
            Vec::new()
        };

        let summary = FunctionSummary {
            arg_map: body.arg_map,
            graph,
            result,
            writes,
            opaque_sources,
            status: this.status,
        };
        this.ctx.rollback_analysis_transaction();
        context.restore(this.ctx);
        Some(summary)
    }

    fn dependency_graph(&mut self) -> (DependencyDag<NodeKey>, Vec<(NodeKey, Option<usize>)>) {
        let mut destinations = self
            .written
            .iter()
            .copied()
            .filter(|key| self.is_module_scope_key(*key))
            .collect::<Vec<_>>();
        destinations.sort_unstable();
        let roots = destinations
            .iter()
            .map(|destination| self.read_key(*destination))
            .collect::<Vec<_>>();
        let allowed = self.module_scope_keys().into_iter().collect::<HashSet<_>>();
        let graph = self.dependency_dag_for_nodes(&roots, allowed);
        let destinations = destinations
            .into_iter()
            .zip(graph.roots.iter().copied())
            .collect();
        (graph, destinations)
    }

    fn is_module_scope_key(&self, key: NodeKey) -> bool {
        self.module_scope_ids.contains(&key.0)
    }

    fn ssa_key(&self, node: NodeKey) -> SsaKey {
        let call_frame = self
            .ctx
            .variables
            .get(&node.0)
            .is_some_and(|variable| variable.affiliation == crate::symbol::Affiliation::Function)
            .then(|| self.call_frames.last().copied())
            .flatten();
        SsaKey { node, call_frame }
    }

    fn read_key(&mut self, node: NodeKey) -> VersionId {
        self.ssa.read(self.ssa_key(node))
    }

    fn bind_key(&mut self, node: NodeKey, version: VersionId) {
        self.ssa.bind(self.ssa_key(node), version);
    }

    fn dependency_dag_for_nodes(
        &self,
        roots: &[VersionId],
        allowed: impl IntoIterator<Item = NodeKey>,
    ) -> DependencyDag<NodeKey> {
        let allowed = allowed
            .into_iter()
            .map(|node| SsaKey {
                node,
                call_frame: None,
            })
            .collect::<HashSet<_>>();
        let graph = self.ssa.dependency_dag(roots, &allowed);
        DependencyDag {
            nodes: graph
                .nodes
                .into_iter()
                .map(|node| match node {
                    DependencyDagNode::External(SsaKey {
                        node,
                        call_frame: None,
                    }) => DependencyDagNode::External(node),
                    DependencyDagNode::External(SsaKey {
                        call_frame: Some(_),
                        ..
                    }) => unreachable!("call-frame storage is not a visible DAG source"),
                    DependencyDagNode::Internal => DependencyDagNode::Internal,
                })
                .collect(),
            edges: graph.edges,
            roots: graph.roots,
            domains: graph.domains,
            sites: graph.sites,
        }
    }

    fn module_scope_keys(&self) -> Vec<NodeKey> {
        let mut keys = self
            .module_scope_ids
            .iter()
            .flat_map(|&id| self.keys_for_id(id))
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn process_write_footprint(&mut self, statements: &[Statement]) -> Vec<NodeKey> {
        let mut keys = HashSet::default();

        // Prefer the sparse IR destinations. Besides covering ordinary writes
        // exactly, this avoids scanning a dense assignment mask merely to add
        // a NodeKey that is already known from the statement itself.
        let mut visited = HashSet::default();
        self.collect_statement_write_footprint(statements, &mut keys, &mut visited);

        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    #[allow(dead_code)]
    fn statement_write_footprint(&mut self, statements: &[Statement]) -> Vec<NodeKey> {
        let mut keys = HashSet::default();
        let mut visited = HashSet::default();
        self.collect_statement_write_footprint(statements, &mut keys, &mut visited);
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    #[allow(dead_code)]
    fn function_call_write_footprint(&mut self, call: &FunctionCall) -> Vec<NodeKey> {
        let mut keys = HashSet::default();
        let mut visited = HashSet::default();
        self.collect_function_call_write_footprint(call, &mut keys, &mut visited);
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    fn collect_statement_write_footprint(
        &mut self,
        statements: &[Statement],
        keys: &mut HashSet<NodeKey>,
        visited: &mut HashSet<FunctionSummaryKey>,
    ) {
        for statement in statements {
            #[cfg(test)]
            WRITE_FOOTPRINT_STATEMENT_VISITS
                .set(WRITE_FOOTPRINT_STATEMENT_VISITS.get().saturating_add(1));
            match statement {
                Statement::Assign(assign) => {
                    self.collect_expression_write_footprint(&assign.expr, keys, visited);
                    for destination in &assign.dst {
                        self.collect_destination_write_footprint(destination, keys, visited);
                    }
                }
                Statement::If(statement) => {
                    self.collect_expression_write_footprint(&statement.cond, keys, visited);
                    self.collect_statement_write_footprint(&statement.true_side, keys, visited);
                    self.collect_statement_write_footprint(&statement.false_side, keys, visited);
                }
                Statement::IfReset(statement) => {
                    self.collect_statement_write_footprint(&statement.true_side, keys, visited);
                    self.collect_statement_write_footprint(&statement.false_side, keys, visited);
                }
                Statement::Case(statement) => {
                    self.collect_expression_write_footprint(&statement.case_target, keys, visited);
                    for arm in &statement.arms {
                        for pattern in &arm.patterns {
                            match pattern {
                                crate::ir::CasePattern::Eq(expression) => self
                                    .collect_expression_write_footprint(expression, keys, visited),
                                crate::ir::CasePattern::Range { lo, hi, .. } => {
                                    self.collect_expression_write_footprint(lo, keys, visited);
                                    self.collect_expression_write_footprint(hi, keys, visited);
                                }
                            }
                        }
                        self.collect_statement_write_footprint(&arm.body, keys, visited);
                    }
                    self.collect_statement_write_footprint(&statement.default, keys, visited);
                }
                Statement::For(statement) => {
                    let (start, end, _) = for_range_bounds(&statement.range);
                    for bound in [start, end] {
                        if let ForBound::Expression(expression) = bound {
                            self.collect_expression_write_footprint(expression, keys, visited);
                        }
                    }
                    self.collect_statement_write_footprint(&statement.body, keys, visited);
                }
                Statement::FunctionCall(call) => {
                    self.collect_function_call_write_footprint(call, keys, visited);
                }
                Statement::SystemFunctionCall(call) => {
                    self.collect_system_call_write_footprint(call, keys, visited);
                }
                Statement::TbMethodCall(_)
                | Statement::Break
                | Statement::Unsupported(_)
                | Statement::Null => {}
            }
        }
    }

    fn collect_destination_write_footprint(
        &mut self,
        destination: &AssignDestination,
        keys: &mut HashSet<NodeKey>,
        visited: &mut HashSet<FunctionSummaryKey>,
    ) {
        let mut resolved = destination.clone();
        resolved.index = self.receiver_index(resolved.id, &resolved.index);
        for (array, packed) in dst_writes(&resolved, &mut self.ctx) {
            keys.extend(self.bit_part.overlapping_access(resolved.id, array, packed));
        }
        for expression in destination
            .index
            .0
            .iter()
            .chain(destination.select.0.iter())
        {
            self.collect_expression_write_footprint(expression, keys, visited);
        }
        if let Some((_, expression)) = &destination.select.1 {
            self.collect_expression_write_footprint(expression, keys, visited);
        }
    }

    fn collect_expression_write_footprint(
        &mut self,
        expression: &Expression,
        keys: &mut HashSet<NodeKey>,
        visited: &mut HashSet<FunctionSummaryKey>,
    ) {
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::Variable(_, index, select, _) => {
                    for expression in index.0.iter().chain(select.0.iter()) {
                        self.collect_expression_write_footprint(expression, keys, visited);
                    }
                    if let Some((_, expression)) = &select.1 {
                        self.collect_expression_write_footprint(expression, keys, visited);
                    }
                }
                Factor::FunctionCall(call) => {
                    self.collect_function_call_write_footprint(call, keys, visited);
                }
                Factor::SystemFunctionCall(call) => {
                    self.collect_system_call_write_footprint(call, keys, visited);
                }
                Factor::HierVariable(reference) => {
                    for expression in reference.index.0.iter().chain(reference.select.0.iter()) {
                        self.collect_expression_write_footprint(expression, keys, visited);
                    }
                    if let Some((_, expression)) = &reference.select.1 {
                        self.collect_expression_write_footprint(expression, keys, visited);
                    }
                }
                Factor::Unknown(_) | Factor::Value(_) | Factor::Anonymous(_) => {}
            },
            Expression::Unary(_, expression, _) => {
                self.collect_expression_write_footprint(expression, keys, visited);
            }
            Expression::Binary(left, _, right, _) => {
                self.collect_expression_write_footprint(left, keys, visited);
                self.collect_expression_write_footprint(right, keys, visited);
            }
            Expression::Ternary(condition, left, right, _) => {
                self.collect_expression_write_footprint(condition, keys, visited);
                self.collect_expression_write_footprint(left, keys, visited);
                self.collect_expression_write_footprint(right, keys, visited);
            }
            Expression::Concatenation(parts, _) => {
                for (expression, repeat) in parts {
                    self.collect_expression_write_footprint(expression, keys, visited);
                    if let Some(repeat) = repeat {
                        self.collect_expression_write_footprint(repeat, keys, visited);
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(expression, repeat) => {
                            self.collect_expression_write_footprint(expression, keys, visited);
                            if let Some(repeat) = repeat {
                                self.collect_expression_write_footprint(repeat, keys, visited);
                            }
                        }
                        ArrayLiteralItem::Defaul(expression) => {
                            self.collect_expression_write_footprint(expression, keys, visited);
                        }
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, expression) in fields {
                    self.collect_expression_write_footprint(expression, keys, visited);
                }
            }
        }
    }

    fn collect_function_call_write_footprint(
        &mut self,
        call: &FunctionCall,
        keys: &mut HashSet<NodeKey>,
        visited: &mut HashSet<FunctionSummaryKey>,
    ) {
        for actual in call.inputs.values() {
            self.collect_expression_write_footprint(actual, keys, visited);
        }
        for destinations in call.outputs.values() {
            for destination in destinations {
                self.collect_destination_write_footprint(destination, keys, visited);
            }
        }

        let summary_key = FunctionSummaryKey {
            id: call.id,
            index: call.index.clone(),
        };
        if !visited.insert(summary_key.clone()) {
            return;
        }
        let Some(statements) = self
            .module
            .functions
            .get(&call.id)
            .and_then(|function| function.get_function(call.index.as_deref().unwrap_or_default()))
            .map(|body| body.statements)
        else {
            visited.remove(&summary_key);
            return;
        };
        self.collect_statement_write_footprint(&statements, keys, visited);
        // The actual expressions and output destinations above remain
        // call-site-specific and are always visited. The selected function
        // body is identical for every later call with the same summary key,
        // so retaining it avoids walking an N-statement body at N call sites.
    }

    fn collect_system_call_write_footprint(
        &mut self,
        call: &crate::ir::SystemFunctionCall,
        keys: &mut HashSet<NodeKey>,
        visited: &mut HashSet<FunctionSummaryKey>,
    ) {
        match &call.kind {
            SystemFunctionKind::Bits(input)
            | SystemFunctionKind::Size(input)
            | SystemFunctionKind::Clog2(input)
            | SystemFunctionKind::Onehot(input)
            | SystemFunctionKind::Signed(input)
            | SystemFunctionKind::Unsigned(input) => {
                self.collect_expression_write_footprint(&input.0, keys, visited);
            }
            SystemFunctionKind::Readmemh(input, output) => {
                self.collect_expression_write_footprint(&input.0, keys, visited);
                for destination in &output.0 {
                    self.collect_destination_write_footprint(destination, keys, visited);
                }
            }
            SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
                for input in inputs {
                    self.collect_expression_write_footprint(&input.0, keys, visited);
                }
            }
            SystemFunctionKind::Assert { cond, args, .. } => {
                self.collect_expression_write_footprint(&cond.0, keys, visited);
                for input in args {
                    self.collect_expression_write_footprint(&input.0, keys, visited);
                }
            }
            SystemFunctionKind::Finish => {}
        }
    }

    #[allow(dead_code)]
    fn opaque_value(&mut self) -> VersionId {
        self.status = self.status.max(AnalysisStatus::Partial);
        self.ssa.definition(Vec::new())
    }

    #[allow(dead_code)]
    fn opaque_kill_keys(&mut self, keys: Vec<NodeKey>, weak: bool) {
        self.status = self.status.max(AnalysisStatus::Partial);
        self.bind_opaque_keys(keys, weak);
    }

    #[allow(dead_code)]
    fn bind_opaque_keys(&mut self, keys: Vec<NodeKey>, weak: bool) {
        for key in keys {
            let opaque = self.ssa.definition(Vec::new());
            self.bind_destination(key, opaque, weak);
        }
    }

    #[allow(dead_code)]
    fn opaque_causal_boundary(&mut self) {
        self.opaque_kill_keys(self.causal_write_keys.clone(), false);
    }

    #[allow(dead_code)]
    fn opaque_function_call_boundary(&mut self, call: &FunctionCall) {
        for input in call.inputs.values() {
            self.eval_expr(input);
        }
        let mut keys = self.function_call_write_footprint(call);
        // This helper is used only when the callee effect summary is missing
        // or recursive. Its hidden writes may target any key owned by the
        // current unit, but never a continuous/other-process-only source.
        keys.extend_from_slice(&self.causal_write_keys);
        keys.sort_unstable();
        keys.dedup();
        self.opaque_kill_keys(keys, false);
        for destinations in call.outputs.values() {
            for destination in destinations {
                // Output lvalue selectors still execute, but an unresolved
                // callee supplies no proven value or control dependency.
                self.write_destination(destination, &[], &[]);
            }
        }
    }

    fn read_keys(&mut self, id: VarId, index: &VarIndex, select: &VarSelect) -> Vec<NodeKey> {
        if !self.ctx.variables.contains_key(&id) && index.0.is_empty() && select.is_empty() {
            return self.keys_for_id(id);
        }
        let mut keys = Vec::new();
        let index = self.receiver_index(id, index);
        let accesses = var_reads(id, &index, select, &mut self.ctx);
        if accesses.is_empty() {
            self.status = self.status.max(AnalysisStatus::Partial);
        }
        for (idx, span) in accesses {
            keys.extend(self.bit_part.overlapping_access(id, idx, span));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn write_keys(&mut self, destination: &AssignDestination) -> Vec<NodeKey> {
        if !self.ctx.variables.contains_key(&destination.id)
            && destination.index.0.is_empty()
            && destination.select.is_empty()
        {
            return self.keys_for_id(destination.id);
        }
        let mut keys = Vec::new();
        let mut destination = destination.clone();
        destination.index = self.receiver_index(destination.id, &destination.index);
        let accesses = dst_writes(&destination, &mut self.ctx);
        if accesses.is_empty() {
            self.status = AnalysisStatus::Barrier;
        }
        for (idx, span) in accesses {
            keys.extend(self.bit_part.overlapping_access(destination.id, idx, span));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn destination_is_dynamic(&self, destination: &AssignDestination) -> bool {
        let index = self.receiver_index(destination.id, &destination.index);
        !index.is_const() || !destination.select.is_const_with_range()
    }

    fn flattened_affine_index(&mut self, id: VarId, index: &VarIndex) -> Option<AffineIndex> {
        let index = self.receiver_index(id, index);
        let variable = self.ctx.variables.get(&id)?;
        if index.dimension() != variable.r#type.array.dims() {
            return None;
        }
        let flattened = variable.r#type.array.calc_index_expr(&index.0)?;
        affine_index(&flattened, &mut self.ctx)
    }

    fn bind_destination(&mut self, key: NodeKey, version: VersionId, dynamic: bool) {
        if dynamic {
            let key = self.ssa_key(key);
            self.ssa.weak_bind(key, version);
        } else {
            self.bind_key(key, version);
        }
        self.written.insert(key);
    }

    fn receiver_index(&self, id: VarId, index: &VarIndex) -> VarIndex {
        if !self.ctx.variables.get(&id).is_some_and(|variable| {
            matches!(
                variable.affiliation,
                crate::symbol::Affiliation::Module | crate::symbol::Affiliation::Interface
            )
        }) {
            return index.clone();
        }
        self.receiver_indices
            .last()
            .and_then(Clone::clone)
            .unwrap_or_else(|| index.clone())
    }

    fn read_variable(&mut self, id: VarId, index: &VarIndex, select: &VarSelect) -> Vec<VersionId> {
        self.read_keys(id, index, select)
            .into_iter()
            .map(|key| self.read_key(key))
            .collect()
    }

    fn eval_destination_selectors(&mut self, destination: &AssignDestination) -> Vec<VersionId> {
        let mut sources = Vec::new();
        for expression in destination
            .index
            .0
            .iter()
            .chain(destination.select.0.iter())
        {
            sources.extend(self.eval_expr(expression));
        }
        if let Some((_, expression)) = &destination.select.1 {
            sources.extend(self.eval_expr(expression));
        }
        sources
    }

    fn write_destination(
        &mut self,
        destination: &AssignDestination,
        sources: &[VersionId],
        controls: &[VersionId],
    ) {
        let mut dependencies = sources.to_vec();
        dependencies.extend_from_slice(controls);
        dependencies.extend(self.eval_destination_selectors(destination));
        self.bind_whole_destination(destination, dependencies, controls);
    }

    fn bind_whole_destination(
        &mut self,
        destination: &AssignDestination,
        dependencies: Vec<VersionId>,
        controls: &[VersionId],
    ) {
        let keys = self.write_keys(destination);
        let dynamic = self.destination_is_dynamic(destination);
        let version = self
            .ssa
            .definition_guarded(dependencies, &self.path_condition);
        for key in keys {
            if let Some(token) = self.active_assignment {
                self.ssa.record_site(version, token, controls);
            }
            self.bind_destination(key, version, dynamic);
        }
    }

    fn destination_width(&mut self, destination: &AssignDestination) -> Option<usize> {
        let variable = self.ctx.variables.get(&destination.id)?.clone();
        let (high, low) = destination
            .select
            .eval_value(&mut self.ctx, &variable.r#type, false)?;
        high.checked_sub(low)?.checked_add(1)
    }

    fn key_span(&self, key: NodeKey) -> Option<PackedSpan> {
        self.bit_part.ranges_of((key.0, key.1)).get(key.2).copied()
    }

    fn write_assignment_destination(
        &mut self,
        destination: &AssignDestination,
        expression: &Expression,
        expression_offset: usize,
        expression_context_width: usize,
        controls: &[VersionId],
    ) {
        let variable = self.ctx.variables.get(&destination.id).cloned();
        let selected = if destination.select.is_const_with_range() {
            variable.as_ref().and_then(|variable| {
                destination
                    .select
                    .eval_value(&mut self.ctx, &variable.r#type, false)
            })
        } else {
            None
        };
        let mut selected_destination = destination.clone();
        selected_destination.index = self.receiver_index(destination.id, &destination.index);
        let destination_array = dst_writes(&selected_destination, &mut self.ctx)
            .into_iter()
            .map(|(array, _)| array)
            .next();
        let destination_offset = destination_array
            .zip(selected)
            .and_then(|(array, (_, low))| {
                Some(PositionRelation {
                    array: Some(isize::try_from(array.start).ok()?),
                    packed: Some(signed_difference(low, expression_offset)?),
                })
            });
        let destination_index = self.flattened_affine_index(destination.id, &destination.index);
        let keys = self.write_keys(destination);
        let dynamic_array = !destination.index.is_const();
        let dynamic_packed = !destination.select.is_const_with_range();
        let dynamic = dynamic_array || dynamic_packed;
        for key in keys {
            let mut whole = controls.to_vec();
            for selector in destination
                .index
                .0
                .iter()
                .chain(destination.select.0.iter())
            {
                whole.extend(self.eval_expr(selector));
            }
            if let Some((_, selector)) = &destination.select.1 {
                whole.extend(self.eval_expr(selector));
            }
            let mut sources = if let (Some(destination_array), Some((_, low)), Some(key_span)) =
                (destination_array, selected, self.key_span(key))
            {
                let destination_region = key.1.intersection(destination_array);
                let expression_array = destination_region.and_then(|array| {
                    if destination_index
                        .as_ref()
                        .is_some_and(|index| !index.terms.is_empty())
                    {
                        Some(ArraySpan {
                            start: 0,
                            length: array.length,
                        })
                    } else {
                        array.translated(destination_array.start, 0)
                    }
                });
                if let (Some(array), Some(destination_region), Some(packed)) = (
                    expression_array,
                    destination_region,
                    key_span.translated(low, expression_offset),
                ) {
                    self.eval_expr_requested_in(
                        expression,
                        array,
                        packed,
                        expression_context_width,
                        &ProjectionContext {
                            destination_index: destination_index.clone(),
                            destination_array: Some(destination_region),
                        },
                    )
                } else {
                    ExpressionSources::default()
                }
            } else {
                ExpressionSources::whole(self.eval_expr(expression))
            };
            sources.extend_whole(whole);
            sources.normalize();
            for (_, relation) in &mut sources.sources {
                *relation = destination_offset
                    .map(|base| relation.compose(base))
                    .unwrap_or_else(PositionRelation::whole);
            }
            let version = self
                .ssa
                .related_definition_guarded(sources.sources, &self.path_condition);
            if let Some(token) = self.active_assignment {
                self.ssa.record_site(version, token, controls);
            }
            self.bind_destination(key, version, dynamic);
        }
    }

    fn eval_function_body(
        &mut self,
        statements: &[Statement],
        return_id: Option<VarId>,
        controls: &[VersionId],
    ) {
        let caller_condition = self.path_condition.clone();
        let checkpoint = self.ssa.checkpoint();
        self.function_flows.push(FunctionFlow {
            return_id,
            checkpoint,
            returns: Vec::new(),
        });
        let flow = self.eval_block(statements, controls);
        let mut function = self
            .function_flows
            .pop()
            .expect("function flow was pushed above");
        let fallthrough = self.ssa.capture_and_rollback(checkpoint);
        if flow.flow != ProcedureFlow::Return {
            function.returns.push(fallthrough);
        }
        self.ssa.merge(&function.returns);
        self.path_condition = caller_condition;
    }

    fn record_return(&mut self) {
        let Some(function) = self.function_flows.last() else {
            return;
        };
        let state = self.ssa.snapshot_since(function.checkpoint);
        self.function_flows
            .last_mut()
            .expect("checked above")
            .returns
            .push(state);
    }

    fn record_break(&mut self) {
        let Some(r#loop) = self.loop_flows.last() else {
            return;
        };
        let state = FlowState {
            state: self.ssa.snapshot_since(r#loop.checkpoint),
            condition: self.path_condition.clone(),
        };
        self.loop_flows
            .last_mut()
            .expect("checked above")
            .breaks
            .push(state);
    }

    fn next_branch_id(&mut self, arms: usize) -> BranchId {
        let branch = BranchId::new(self.branch_namespace, self.next_branch, arms);
        self.next_branch += 1;
        branch
    }

    fn use_expression_namespace(&mut self, expression: &Expression) {
        if self.branch_namespace == 0 && self.next_branch == 0 {
            self.branch_namespace = std::ptr::from_ref(expression).addr();
        }
    }

    fn expression_branch_id(&mut self, expression: &Expression) -> BranchId {
        let key = std::ptr::from_ref(expression);
        if let Some(branch) = self
            .call_caches
            .last()
            .and_then(Option::as_ref)
            .and_then(|cache| cache.expression_branches.get(&key))
        {
            return *branch;
        }
        let branch = self.next_branch_id(2);
        if let Some(Some(cache)) = self.call_caches.last_mut() {
            cache.expression_branches.insert(key, branch);
        }
        branch
    }

    fn merge_flow_states(&mut self, states: &[FlowState]) {
        self.ssa.merge(states.iter().map(|state| &state.state));
        self.path_condition =
            PathCondition::disjoin_all(states.iter().map(|state| &state.condition));
    }

    fn is_return_assignment(&self, destinations: &[AssignDestination]) -> bool {
        self.function_flows
            .last()
            .and_then(|function| function.return_id)
            .is_some_and(|return_id| {
                destinations
                    .iter()
                    .any(|destination| destination.id == return_id)
            })
    }

    fn eval_block(&mut self, statements: &[Statement], controls: &[VersionId]) -> FlowResult {
        let mut active_controls = controls.to_vec();
        let mut continuation_controls = Vec::new();
        for statement in statements {
            let result = self.eval_statement(statement, &active_controls);
            if result.flow != ProcedureFlow::Continue {
                return result;
            }
            for control in result.continuation_controls {
                if !active_controls.contains(&control) {
                    active_controls.push(control);
                }
                if !continuation_controls.contains(&control) {
                    continuation_controls.push(control);
                }
            }
        }
        FlowResult {
            flow: ProcedureFlow::Continue,
            continuation_controls,
        }
    }

    fn eval_statement(&mut self, statement: &Statement, controls: &[VersionId]) -> FlowResult {
        match statement {
            Statement::Assign(assign) => {
                let previous_assignment = self.active_assignment;
                self.active_assignment = self.tracing.then_some(assign.token);
                self.call_caches.push(Some(EvaluationCache::default()));
                let widths: Vec<_> = assign
                    .dst
                    .iter()
                    .map(|destination| self.destination_width(destination))
                    .collect();
                if widths.iter().all(Option::is_some) {
                    let total_width = widths.iter().flatten().sum();
                    let mut offset = total_width;
                    for (destination, width) in assign.dst.iter().zip(widths) {
                        let width = width.expect("checked above");
                        offset -= width;
                        self.write_assignment_destination(
                            destination,
                            &assign.expr,
                            offset,
                            total_width,
                            controls,
                        );
                    }
                } else {
                    let sources = self.eval_expr(&assign.expr);
                    for destination in &assign.dst {
                        self.write_destination(destination, &sources, controls);
                    }
                }
                self.call_caches.pop();
                self.active_assignment = previous_assignment;
                if self.is_return_assignment(&assign.dst) {
                    self.record_return();
                    FlowResult::new(ProcedureFlow::Return)
                } else {
                    FlowResult::new(ProcedureFlow::Continue)
                }
            }
            Statement::If(statement) => self.eval_if(statement, controls),
            Statement::Case(statement) => self.eval_case(statement, controls),
            Statement::For(statement) => self.eval_for(statement, controls),
            Statement::FunctionCall(call) => {
                self.eval_call(call, controls);
                FlowResult::new(ProcedureFlow::Continue)
            }
            Statement::SystemFunctionCall(call) => {
                self.eval_system_call(call, controls, false);
                FlowResult::new(ProcedureFlow::Continue)
            }
            Statement::Break => {
                self.record_break();
                FlowResult::new(ProcedureFlow::Break)
            }
            Statement::IfReset(_) | Statement::TbMethodCall(_) | Statement::Null => {
                FlowResult::new(ProcedureFlow::Continue)
            }
            Statement::Unsupported(_) => {
                self.status = AnalysisStatus::Barrier;
                FlowResult::new(ProcedureFlow::Continue)
            }
        }
    }

    fn merge_branches(
        &mut self,
        branches: Vec<(FlowResult, BranchState<SsaKey>, PathCondition)>,
        branch_controls: &[VersionId],
    ) -> FlowResult {
        let mut continuation = Vec::new();
        let mut continuation_conditions = Vec::new();
        let mut continuation_controls = Vec::new();
        let mut has_continue = false;
        let mut has_break = false;
        let all_continue = branches
            .iter()
            .all(|(result, _, _)| result.flow == ProcedureFlow::Continue);
        for (result, state, condition) in branches {
            match result.flow {
                ProcedureFlow::Continue => {
                    has_continue = true;
                    continuation.push(state);
                    let mut controls = result.continuation_controls;
                    if !all_continue {
                        controls.extend_from_slice(branch_controls);
                    }
                    if !controls.is_empty() {
                        continuation_controls
                            .push(self.ssa.definition_guarded(controls, &condition));
                    }
                    continuation_conditions.push(condition);
                }
                ProcedureFlow::Break => {
                    has_break = true;
                }
                ProcedureFlow::Return => {}
            }
        }
        self.ssa.merge(&continuation);
        if has_continue {
            self.path_condition = PathCondition::disjoin_all(&continuation_conditions);
            FlowResult {
                flow: ProcedureFlow::Continue,
                continuation_controls,
            }
        } else if has_break {
            FlowResult::new(ProcedureFlow::Break)
        } else {
            FlowResult::new(ProcedureFlow::Return)
        }
    }

    fn eval_if(&mut self, statement: &IfStatement, controls: &[VersionId]) -> FlowResult {
        let condition = self.eval_expr(&statement.cond);
        let mut nested_controls = controls.to_vec();
        nested_controls.extend_from_slice(&condition);
        match self.constant_truth(&statement.cond) {
            Some(true) => return self.eval_block(&statement.true_side, &nested_controls),
            Some(false) => return self.eval_block(&statement.false_side, &nested_controls),
            None => {}
        }

        let branch = self.next_branch_id(2);
        let parent_condition = self.path_condition.clone();
        self.path_condition = parent_condition.with_choice(branch, 0);
        let checkpoint = self.ssa.checkpoint();
        let true_flow = self.eval_block(&statement.true_side, &nested_controls);
        let true_state = self.ssa.capture_and_rollback(checkpoint);
        let true_condition = self.path_condition.clone();

        self.path_condition = parent_condition.with_choice(branch, 1);
        let checkpoint = self.ssa.checkpoint();
        let false_flow = self.eval_block(&statement.false_side, &nested_controls);
        let false_state = self.ssa.capture_and_rollback(checkpoint);
        let false_condition = self.path_condition.clone();

        self.path_condition = parent_condition;
        self.merge_branches(
            vec![
                (true_flow, true_state, true_condition),
                (false_flow, false_state, false_condition),
            ],
            &condition,
        )
    }

    fn eval_case(&mut self, statement: &CaseStatement, controls: &[VersionId]) -> FlowResult {
        let mut condition = self.eval_expr(&statement.case_target);
        for arm in &statement.arms {
            for pattern in &arm.patterns {
                match pattern {
                    crate::ir::CasePattern::Eq(expression) => {
                        condition.extend(self.eval_expr(expression));
                    }
                    crate::ir::CasePattern::Range { lo, hi, .. } => {
                        condition.extend(self.eval_expr(lo));
                        condition.extend(self.eval_expr(hi));
                    }
                }
            }
        }
        let mut nested_controls = controls.to_vec();
        nested_controls.extend_from_slice(&condition);

        if let Some(target) = statement.case_target.eval_value(&mut self.ctx) {
            let mut possible = Vec::new();
            let mut has_definite_match = false;
            for (index, arm) in statement.arms.iter().enumerate() {
                let mut uncertain = false;
                let mut matched = false;
                for pattern in &arm.patterns {
                    match pattern.matches(&target, &mut self.ctx) {
                        Some(true) => {
                            matched = true;
                            break;
                        }
                        Some(false) => {}
                        None => uncertain = true,
                    }
                }
                if matched {
                    possible.push(index);
                    has_definite_match = true;
                    break;
                }
                if uncertain {
                    possible.push(index);
                }
            }

            if possible.is_empty() {
                return self.eval_block(&statement.default, &nested_controls);
            }
            if possible.len() == 1 && has_definite_match {
                return self.eval_block(&statement.arms[possible[0]].body, &nested_controls);
            }

            let branch = self.next_branch_id(statement.arms.len() + 1);
            let parent_condition = self.path_condition.clone();
            let mut states = Vec::with_capacity(possible.len() + usize::from(!has_definite_match));
            for index in possible {
                self.path_condition = parent_condition.with_choice(branch, index);
                let checkpoint = self.ssa.checkpoint();
                let flow = self.eval_block(&statement.arms[index].body, &nested_controls);
                let state = self.ssa.capture_and_rollback(checkpoint);
                states.push((flow, state, self.path_condition.clone()));
            }
            if !has_definite_match {
                self.path_condition = parent_condition.with_choice(branch, statement.arms.len());
                let checkpoint = self.ssa.checkpoint();
                let flow = self.eval_block(&statement.default, &nested_controls);
                let state = self.ssa.capture_and_rollback(checkpoint);
                states.push((flow, state, self.path_condition.clone()));
            }
            self.path_condition = parent_condition;
            return self.merge_branches(states, &condition);
        }

        let branch = self.next_branch_id(statement.arms.len() + 1);
        let parent_condition = self.path_condition.clone();
        let mut states = Vec::with_capacity(statement.arms.len() + 1);
        for (index, arm) in statement.arms.iter().enumerate() {
            self.path_condition = parent_condition.with_choice(branch, index);
            let checkpoint = self.ssa.checkpoint();
            let flow = self.eval_block(&arm.body, &nested_controls);
            let state = self.ssa.capture_and_rollback(checkpoint);
            states.push((flow, state, self.path_condition.clone()));
        }
        self.path_condition = parent_condition.with_choice(branch, statement.arms.len());
        let checkpoint = self.ssa.checkpoint();
        let flow = self.eval_block(&statement.default, &nested_controls);
        let state = self.ssa.capture_and_rollback(checkpoint);
        states.push((flow, state, self.path_condition.clone()));
        self.path_condition = parent_condition;
        self.merge_branches(states, &condition)
    }

    fn eval_for(&mut self, statement: &ForStatement, controls: &[VersionId]) -> FlowResult {
        let range_controls = self.eval_for_range_controls(&statement.range, controls);
        if self.for_range_is_proven_empty(&statement.range) {
            return FlowResult::new(ProcedureFlow::Continue);
        }

        if let Some(iterations) = statement.range.eval_iter(&mut self.ctx) {
            self.eval_known_for_iterations(statement, &range_controls, iterations)
        } else if statement.range.is_over_size_limit(&mut self.ctx) {
            // A resource limit must not turn a finite, statically known loop
            // into the more conservative runtime-loop semantics. Without an
            // exact finite summary, suppress dependencies from this procedure
            // and report the analysis as incomplete.
            self.status = AnalysisStatus::Barrier;
            FlowResult::new(ProcedureFlow::Continue)
        } else {
            self.eval_runtime_for(statement, &range_controls)
        }
    }

    fn eval_for_range_controls(
        &mut self,
        range: &ForRange,
        controls: &[VersionId],
    ) -> Vec<VersionId> {
        let mut range_controls = controls.to_vec();
        let (start, end, _) = for_range_bounds(range);
        for bound in [start, end] {
            if let ForBound::Expression(expression) = bound {
                range_controls.extend(self.eval_expr(expression));
            }
        }
        range_controls
    }

    fn for_range_is_proven_empty(&mut self, range: &ForRange) -> bool {
        let (start, end, inclusive) = for_range_bounds(range);
        !inclusive
            && affine_bound(start, &mut self.ctx)
                .zip(affine_bound(end, &mut self.ctx))
                .is_some_and(|(start, end)| start == end)
    }

    fn set_known_iterator_value(&mut self, statement: &ForStatement, value: usize) {
        let Some(variable) = self.ctx.variables.get_mut(&statement.var_id) else {
            return;
        };
        let Some(width) = statement.var_type.total_width() else {
            return;
        };
        variable.set_value(
            &[],
            Value::new(value as u64, width, statement.var_type.signed),
            None,
        );
    }

    fn forget_runtime_iterator_value(&mut self, iterator: VarId) {
        if let Some(variable) = self.ctx.variables.get_mut(&iterator) {
            // Conversion may leave the range's initial value in the shared
            // compile-time store. It is not a constant in a runtime loop and
            // must not prune iterator-dependent branches.
            variable.value.clear();
        }
    }

    fn eval_known_for_iterations(
        &mut self,
        statement: &ForStatement,
        range_controls: &[VersionId],
        iterations: Vec<usize>,
    ) -> FlowResult {
        let parent_condition = self.path_condition.clone();
        let checkpoint = self.ssa.checkpoint();
        self.loop_flows.push(LoopFlow {
            checkpoint,
            breaks: Vec::new(),
        });
        let mut flow = ProcedureFlow::Continue;
        let mut iteration_controls = range_controls.to_vec();
        for value in iterations {
            self.set_known_iterator_value(statement, value);
            let result = self.eval_block(&statement.body, &iteration_controls);
            flow = result.flow;
            if flow != ProcedureFlow::Continue {
                break;
            }
            for control in result.continuation_controls {
                if !iteration_controls.contains(&control) {
                    iteration_controls.push(control);
                }
            }
        }
        let mut loop_flow = self.loop_flows.pop().expect("loop flow was pushed above");
        let fallthrough = self.ssa.capture_and_rollback(checkpoint);
        if flow == ProcedureFlow::Continue {
            loop_flow.breaks.push(FlowState {
                state: fallthrough,
                condition: self.path_condition.clone(),
            });
        }
        if loop_flow.breaks.is_empty() {
            self.path_condition = parent_condition;
            return FlowResult::new(ProcedureFlow::Return);
        }
        self.merge_flow_states(&loop_flow.breaks);
        FlowResult::new(ProcedureFlow::Continue)
    }

    fn eval_runtime_for(
        &mut self,
        statement: &ForStatement,
        range_controls: &[VersionId],
    ) -> FlowResult {
        // A runtime iterator is not part of a static prefix. Consequently
        // accesses such as x[i], x[i + 1], and x[j] all may address the same
        // LSP region. Evaluate the body once without binding the iterator so
        // the ordinary dynamic-access rules conservatively retain that alias.
        self.forget_runtime_iterator_value(statement.var_id);
        let parent_condition = self.path_condition.clone();
        let may_execute_zero_times = for_range_has_dynamic_bounds(&statement.range);
        let checkpoint = self.ssa.checkpoint();
        self.loop_flows.push(LoopFlow {
            checkpoint,
            breaks: Vec::new(),
        });
        let flow = self.eval_block(&statement.body, range_controls);
        let mut loop_flow = self.loop_flows.pop().expect("loop flow was pushed above");
        let body_state = self.ssa.capture_and_rollback(checkpoint);
        if flow.flow == ProcedureFlow::Continue {
            loop_flow.breaks.push(FlowState {
                state: body_state,
                condition: self.path_condition.clone(),
            });
        }
        self.path_condition = parent_condition;
        // Break and fallthrough paths jointly describe one abstract
        // iteration. Close that finite transfer instead of re-evaluating the
        // body or enumerating runtime iterator values.
        let transfer = self.merge_flow_state_bindings(&loop_flow.breaks);
        self.ssa
            .close_repeated_transfer(&transfer, checkpoint, may_execute_zero_times);
        FlowResult::new(ProcedureFlow::Continue)
    }

    fn merge_flow_state_bindings(&mut self, states: &[FlowState]) -> BranchState<SsaKey> {
        let checkpoint = self.ssa.checkpoint();
        self.ssa.merge(states.iter().map(|state| &state.state));
        self.ssa.capture_and_rollback(checkpoint)
    }

    fn eval_expr_requested(
        &mut self,
        expression: &Expression,
        requested_array: ArraySpan,
        requested: PackedSpan,
        context_width: usize,
    ) -> ExpressionSources {
        self.eval_expr_requested_in(
            expression,
            requested_array,
            requested,
            context_width,
            &ProjectionContext::default(),
        )
    }

    fn eval_expr_requested_in(
        &mut self,
        expression: &Expression,
        requested_array: ArraySpan,
        requested: PackedSpan,
        context_width: usize,
        projection: &ProjectionContext,
    ) -> ExpressionSources {
        let requested_array = if matches!(expression, Expression::ArrayLiteral(_, _)) {
            requested_array
        } else {
            let expression_array = expression.comptime().r#type.array.total().unwrap_or(1);
            let Some(requested_array) = requested_array.intersection(ArraySpan {
                start: 0,
                length: expression_array,
            }) else {
                return ExpressionSources::default();
            };
            requested_array
        };
        let expression_width = expression
            .comptime()
            .r#type
            .total_width()
            .unwrap_or(context_width);
        let mut reads = PackedSpan::whole(expression_width)
            .and_then(|width| requested.intersection(width))
            .map(|span| self.eval_expr_bits_in(expression, requested_array, span, projection))
            .unwrap_or_default();
        if context_width > expression_width
            && expression.comptime().r#type.signed
            && requested.end() > expression_width
            && expression_width != 0
        {
            let mut sign = self.eval_expr_bits_in(
                expression,
                requested_array,
                PackedSpan {
                    start: expression_width - 1,
                    length: 1,
                },
                projection,
            );
            sign.widen_all();
            reads.extend(sign);
        }
        reads.normalize();
        reads
    }

    fn eval_expr_bits(
        &mut self,
        expression: &Expression,
        requested_array: ArraySpan,
        requested: PackedSpan,
    ) -> ExpressionSources {
        self.eval_expr_bits_in(
            expression,
            requested_array,
            requested,
            &ProjectionContext::default(),
        )
    }

    fn eval_expr_bits_in(
        &mut self,
        expression: &Expression,
        requested_array: ArraySpan,
        requested: PackedSpan,
        projection: &ProjectionContext,
    ) -> ExpressionSources {
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::Variable(id, index, select, _) => {
                    let mut selector_sources = Vec::new();
                    for expression in index.0.iter().chain(select.0.iter()) {
                        selector_sources.extend(self.eval_expr(expression));
                    }
                    if let Some((_, expression)) = &select.1 {
                        selector_sources.extend(self.eval_expr(expression));
                    }
                    let variable = self.ctx.variables.get(id).cloned();
                    let selected = if select.is_const_with_range() {
                        variable.as_ref().and_then(|variable| {
                            select.eval_value(&mut self.ctx, &variable.r#type, false)
                        })
                    } else {
                        None
                    };
                    if let Some((_, low)) = selected {
                        let mut reads = Vec::new();
                        let receiver = self.receiver_index(*id, index);
                        let accesses = var_reads(*id, &receiver, select, &mut self.ctx);
                        let dynamic_array_offset = projection
                            .destination_index
                            .clone()
                            .filter(|destination| !destination.terms.is_empty())
                            .and_then(|destination| {
                                let source = self.flattened_affine_index(*id, index)?;
                                destination.destination_offset_from(&source)
                            });
                        let position_preserving =
                            receiver.0.iter().all(|index| index.comptime().is_const)
                                && accesses.len() == 1;
                        if let Some(source_span) = requested.translated(0, low) {
                            for (idx, access) in &accesses {
                                let source_array = if let Some(offset) = dynamic_array_offset {
                                    offset
                                        .checked_neg()
                                        .and_then(|offset| {
                                            translate_array_span(
                                                projection
                                                    .destination_array
                                                    .unwrap_or(requested_array),
                                                offset,
                                            )
                                        })
                                        .and_then(|requested| requested.intersection(*idx))
                                } else if position_preserving {
                                    requested_array
                                        .translated(0, idx.start)
                                        .and_then(|requested| requested.intersection(*idx))
                                } else {
                                    Some(*idx)
                                };
                                if let (Some(source_array), Some(source_span)) =
                                    (source_array, source_span.intersection(*access))
                                {
                                    for key in self.bit_part.overlapping_access(
                                        *id,
                                        source_array,
                                        source_span,
                                    ) {
                                        reads.push(self.read_key(key));
                                    }
                                }
                            }
                        }
                        let offset = dynamic_array_offset
                            .and_then(|array| {
                                Some(PositionRelation {
                                    array: Some(array),
                                    packed: Some(isize::try_from(low).ok()?.checked_neg()?),
                                })
                            })
                            .or_else(|| {
                                position_preserving
                                    .then(|| {
                                        Some(PositionRelation {
                                            array: Some(
                                                isize::try_from(accesses[0].0.start)
                                                    .ok()?
                                                    .checked_neg()?,
                                            ),
                                            packed: Some(isize::try_from(low).ok()?.checked_neg()?),
                                        })
                                    })
                                    .flatten()
                            });
                        if let Some(offset) = offset {
                            let mut sources = ExpressionSources {
                                sources: reads
                                    .into_iter()
                                    .map(|version| (version, offset))
                                    .collect(),
                            };
                            sources.extend_whole(selector_sources);
                            sources
                        } else {
                            selector_sources.extend(reads);
                            ExpressionSources::whole(selector_sources)
                        }
                    } else {
                        selector_sources.extend(self.read_variable(*id, index, select));
                        ExpressionSources::whole(selector_sources)
                    }
                }
                Factor::SystemFunctionCall(call) => match &call.kind {
                    SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                        self.eval_expr_bits_in(&input.0, requested_array, requested, projection)
                    }
                    _ => ExpressionSources::whole(self.eval_system_call(call, &[], true)),
                },
                Factor::FunctionCall(call) => ExpressionSources {
                    sources: self
                        .eval_call_requested(call, &[], Some((requested_array, requested)))
                        .into_iter()
                        .map(|version| (version, PositionRelation::default()))
                        .collect(),
                },
                Factor::Unknown(_) => {
                    if self.function_flows.is_empty() {
                        self.status = AnalysisStatus::Barrier;
                    }
                    ExpressionSources::default()
                }
                Factor::HierVariable(_) | Factor::Value(_) | Factor::Anonymous(_) => {
                    ExpressionSources::default()
                }
            },
            Expression::Unary(op, operand, _) => match op {
                Op::BitNot | Op::Add => {
                    self.eval_expr_bits_in(operand, requested_array, requested, projection)
                }
                _ => ExpressionSources::whole(self.eval_expr(operand)),
            },
            Expression::Binary(left, op, right, comptime) => match op {
                Op::As => {
                    let context_width = comptime.r#type.total_width().unwrap_or(requested.end());
                    self.eval_expr_requested_in(
                        left,
                        requested_array,
                        requested,
                        context_width,
                        projection,
                    )
                }
                Op::LogicShiftL | Op::ArithShiftL => {
                    let shift = right
                        .eval_value(&mut self.ctx)
                        .and_then(|value| value.to_usize());
                    let mut reads = ExpressionSources::whole(self.eval_expr(right));
                    if let Some(shift) = shift {
                        let start = requested.start.max(shift);
                        if let Some(length) = requested.end().checked_sub(start)
                            && let Some(input) = PackedSpan::new(start - shift, length)
                        {
                            let mut input =
                                self.eval_expr_bits_in(left, requested_array, input, projection);
                            if let Ok(shift) = isize::try_from(shift) {
                                input.translate(PositionRelation {
                                    array: Some(0),
                                    packed: Some(shift),
                                });
                            } else {
                                input.widen_all();
                            }
                            reads.extend(input);
                        }
                    } else {
                        reads.extend_whole(self.eval_expr(left));
                    }
                    reads
                }
                Op::LogicShiftR | Op::ArithShiftR => {
                    let shift = right
                        .eval_value(&mut self.ctx)
                        .and_then(|value| value.to_usize());
                    let mut reads = ExpressionSources::whole(self.eval_expr(right));
                    if let Some(shift) = shift {
                        let width = left.comptime().r#type.total_width().unwrap_or(0);
                        let shifted = requested.translated(0, shift);
                        if let Some(input) = shifted
                            .and_then(|shifted| PackedSpan::whole(width)?.intersection(shifted))
                        {
                            let mut input =
                                self.eval_expr_bits_in(left, requested_array, input, projection);
                            if let Ok(shift) = isize::try_from(shift) {
                                input.translate(PositionRelation {
                                    array: Some(0),
                                    packed: Some(-shift),
                                });
                            } else {
                                input.widen_all();
                            }
                            reads.extend(input);
                        }
                        if *op == Op::ArithShiftR
                            && left.comptime().r#type.signed
                            && width != 0
                            && shifted.is_some_and(|shifted| shifted.end() > width)
                        {
                            let mut sign = self.eval_expr_bits_in(
                                left,
                                requested_array,
                                PackedSpan {
                                    start: width - 1,
                                    length: 1,
                                },
                                projection,
                            );
                            sign.widen_all();
                            reads.extend(sign);
                        }
                    } else {
                        reads.extend_whole(self.eval_expr(left));
                    }
                    reads
                }
                Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
                    let context_width = comptime.r#type.total_width().unwrap_or(requested.end());
                    let mut reads = self.eval_expr_requested_in(
                        left,
                        requested_array,
                        requested,
                        context_width,
                        projection,
                    );
                    reads.extend(self.eval_expr_requested_in(
                        right,
                        requested_array,
                        requested,
                        context_width,
                        projection,
                    ));
                    reads
                }
                Op::LogicAnd | Op::LogicOr => {
                    let mut reads = ExpressionSources::whole(self.eval_expr(left));
                    let execute_right = match (op, self.constant_truth(left)) {
                        (Op::LogicAnd, Some(false)) | (Op::LogicOr, Some(true)) => Some(false),
                        (Op::LogicAnd, Some(true)) | (Op::LogicOr, Some(false)) => Some(true),
                        _ => None,
                    };
                    match execute_right {
                        Some(false) => {}
                        Some(true) => reads.extend_whole(self.eval_expr(right)),
                        None => {
                            let branch = self.expression_branch_id(expression);
                            let parent_condition = self.path_condition.clone();

                            let checkpoint = self.ssa.checkpoint();
                            self.path_condition = parent_condition.with_choice(branch, 0);
                            let right = self.eval_expr(right);
                            let right = self.ssa.definition_guarded(right, &self.path_condition);
                            let evaluated_state = self.ssa.capture_and_rollback(checkpoint);

                            let checkpoint = self.ssa.checkpoint();
                            self.path_condition = parent_condition.with_choice(branch, 1);
                            let skipped_state = self.ssa.capture_and_rollback(checkpoint);

                            self.ssa.merge([&evaluated_state, &skipped_state]);
                            self.path_condition = parent_condition;
                            reads.push(right, PositionRelation::whole());
                        }
                    }
                    reads
                }
                _ => ExpressionSources::whole(self.eval_expr(expression)),
            },
            Expression::Ternary(condition, left, right, comptime) => {
                let context_width = comptime.r#type.total_width().unwrap_or(requested.end());
                let mut reads = ExpressionSources::whole(self.eval_expr(condition));
                match self.constant_truth(condition) {
                    Some(true) => reads.extend(self.eval_expr_requested_in(
                        left,
                        requested_array,
                        requested,
                        context_width,
                        projection,
                    )),
                    Some(false) => reads.extend(self.eval_expr_requested_in(
                        right,
                        requested_array,
                        requested,
                        context_width,
                        projection,
                    )),
                    None => {
                        let branch = self.expression_branch_id(expression);
                        let parent_condition = self.path_condition.clone();

                        let checkpoint = self.ssa.checkpoint();
                        self.path_condition = parent_condition.with_choice(branch, 0);
                        let left = self.eval_expr_requested_in(
                            left,
                            requested_array,
                            requested,
                            context_width,
                            projection,
                        );
                        let left = self.guard_expression_sources(left);
                        let left_state = self.ssa.capture_and_rollback(checkpoint);

                        let checkpoint = self.ssa.checkpoint();
                        self.path_condition = parent_condition.with_choice(branch, 1);
                        let right = self.eval_expr_requested_in(
                            right,
                            requested_array,
                            requested,
                            context_width,
                            projection,
                        );
                        let right = self.guard_expression_sources(right);
                        let right_state = self.ssa.capture_and_rollback(checkpoint);

                        self.ssa.merge([&left_state, &right_state]);
                        self.path_condition = parent_condition;
                        reads.extend(left);
                        reads.extend(right);
                    }
                }
                reads
            }
            Expression::Concatenation(parts, _) => {
                let mut low = 0usize;
                let mut reads = ExpressionSources::default();
                for (part, repeat) in parts.iter().rev() {
                    let Some(width) = part.comptime().r#type.total_width() else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    let count = if let Some(repeat) = repeat {
                        let Some(count) = repeat
                            .eval_value(&mut self.ctx)
                            .and_then(|value| value.to_usize())
                        else {
                            return ExpressionSources::whole(self.eval_expr(expression));
                        };
                        reads.extend_whole(self.eval_expr_inner(repeat, false));
                        count
                    } else {
                        1
                    };
                    match project_repeated_span(
                        requested.start,
                        requested.length,
                        low,
                        width,
                        count,
                    ) {
                        RepeatedProjection::Empty => {}
                        RepeatedProjection::Single {
                            local_start,
                            length,
                            output_start,
                        } => {
                            if let Some(local) = PackedSpan::new(local_start, length) {
                                let mut part = self.eval_expr_bits_in(
                                    part,
                                    requested_array,
                                    local,
                                    projection,
                                );
                                if let Ok(output_start) = isize::try_from(output_start) {
                                    part.translate(PositionRelation {
                                        array: Some(0),
                                        packed: Some(output_start),
                                    });
                                } else {
                                    part.widen_all();
                                }
                                reads.extend(part);
                            }
                        }
                        RepeatedProjection::Multiple => {
                            if let Some(local) = PackedSpan::whole(width) {
                                let mut part = self.eval_expr_bits_in(
                                    part,
                                    requested_array,
                                    local,
                                    projection,
                                );
                                part.forget_packed_position();
                                reads.extend(part);
                            } else {
                                reads.extend_whole(self.eval_expr(part));
                            }
                        }
                    }
                    let Some(part_width) = width.checked_mul(count) else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    let Some(next) = low.checked_add(part_width) else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    low = next;
                }
                reads
            }
            Expression::ArrayLiteral(items, _) => {
                let Some(requested_end) = requested_array.end() else {
                    return ExpressionSources::whole(self.eval_expr(expression));
                };
                let total = self
                    .expression_array_extent(expression)
                    .unwrap_or(1)
                    .max(requested_end);
                let mut cursor = 0usize;
                let mut default = None;
                let mut reads = ExpressionSources::default();
                for item in items {
                    let ArrayLiteralItem::Value(value, repeat) = item else {
                        let ArrayLiteralItem::Defaul(value) = item else {
                            unreachable!();
                        };
                        default = Some(value.as_ref());
                        continue;
                    };
                    let item_length = self.expression_array_extent(value).unwrap_or(1);
                    let count = if let Some(repeat) = repeat {
                        let Some(count) = repeat
                            .eval_value(&mut self.ctx)
                            .and_then(|value| value.to_usize())
                        else {
                            return ExpressionSources::whole(self.eval_expr(expression));
                        };
                        reads.extend_whole(self.eval_expr_inner(repeat, false));
                        count
                    } else {
                        1
                    };
                    match project_repeated_span(
                        requested_array.start,
                        requested_array.length,
                        cursor,
                        item_length,
                        count,
                    ) {
                        RepeatedProjection::Empty => {}
                        RepeatedProjection::Single {
                            local_start,
                            length,
                            output_start,
                        } => {
                            let mut item = self.eval_expr_requested_in(
                                value,
                                ArraySpan {
                                    start: local_start,
                                    length,
                                },
                                requested,
                                value
                                    .comptime()
                                    .r#type
                                    .total_width()
                                    .unwrap_or(requested.length),
                                projection,
                            );
                            if let Ok(output_start) = isize::try_from(output_start) {
                                item.translate(PositionRelation {
                                    array: Some(output_start),
                                    packed: Some(0),
                                });
                            } else {
                                item.widen_all();
                            }
                            reads.extend(item);
                        }
                        RepeatedProjection::Multiple => {
                            let mut item = self.eval_expr_requested_in(
                                value,
                                ArraySpan {
                                    start: 0,
                                    length: item_length,
                                },
                                requested,
                                value
                                    .comptime()
                                    .r#type
                                    .total_width()
                                    .unwrap_or(requested.length),
                                projection,
                            );
                            item.forget_array_position();
                            reads.extend(item);
                        }
                    }
                    let Some(item_extent) = item_length.checked_mul(count) else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    let Some(next) = cursor.checked_add(item_extent) else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    cursor = next;
                }
                if let Some(default) = default
                    && cursor < total
                {
                    let item_length = self.expression_array_extent(default).unwrap_or(1);
                    let remaining = total - cursor;
                    let count = remaining.div_ceil(item_length);
                    match project_repeated_span(
                        requested_array.start,
                        requested_array.length,
                        cursor,
                        item_length,
                        count,
                    ) {
                        RepeatedProjection::Empty => {}
                        RepeatedProjection::Single {
                            local_start,
                            length,
                            output_start,
                        } => {
                            let mut item = self.eval_expr_requested_in(
                                default,
                                ArraySpan {
                                    start: local_start,
                                    length,
                                },
                                requested,
                                default
                                    .comptime()
                                    .r#type
                                    .total_width()
                                    .unwrap_or(requested.length),
                                projection,
                            );
                            if let Ok(output_start) = isize::try_from(output_start) {
                                item.translate(PositionRelation {
                                    array: Some(output_start),
                                    packed: Some(0),
                                });
                            } else {
                                item.widen_all();
                            }
                            reads.extend(item);
                        }
                        RepeatedProjection::Multiple => {
                            let mut item = self.eval_expr_requested_in(
                                default,
                                ArraySpan {
                                    start: 0,
                                    length: item_length,
                                },
                                requested,
                                default
                                    .comptime()
                                    .r#type
                                    .total_width()
                                    .unwrap_or(requested.length),
                                projection,
                            );
                            item.forget_array_position();
                            reads.extend(item);
                        }
                    }
                }
                reads
            }
            Expression::StructConstructor(r#type, fields, _) => {
                let mut low = 0usize;
                let mut reads = ExpressionSources::default();
                for (name, value) in fields.iter().rev() {
                    let Some(member) = r#type.get_member_type(*name) else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    let Some(width) = member.total_width() else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    if let Some(window) = PackedSpan::new(low, width)
                        && let Some(local) = requested
                            .intersection(window)
                            .and_then(|span| span.translated(low, 0))
                    {
                        let mut field = self.eval_expr_requested_in(
                            value,
                            requested_array,
                            local,
                            width,
                            projection,
                        );
                        if let Ok(low) = isize::try_from(low) {
                            field.translate(PositionRelation {
                                array: Some(0),
                                packed: Some(low),
                            });
                        } else {
                            field.widen_all();
                        }
                        reads.extend(field);
                    }
                    let Some(next) = low.checked_add(width) else {
                        return ExpressionSources::whole(self.eval_expr(expression));
                    };
                    low = next;
                }
                reads
            }
        }
    }

    fn expression_array_extent(&mut self, expression: &Expression) -> Option<usize> {
        match expression {
            Expression::ArrayLiteral(items, _) if !items.is_empty() => {
                let mut total = 0usize;
                for item in items {
                    let ArrayLiteralItem::Value(value, repeat) = item else {
                        return expression.comptime().r#type.array.total();
                    };
                    let value_extent = self.expression_array_extent(value)?;
                    let repeat = repeat
                        .as_ref()
                        .map(|repeat| {
                            repeat
                                .eval_value(&mut self.ctx)
                                .and_then(|value| value.to_usize())
                        })
                        .unwrap_or(Some(1))?;
                    total = total.checked_add(value_extent.checked_mul(repeat)?)?;
                }
                Some(total)
            }
            _ => expression.comptime().r#type.array.total().or(Some(1)),
        }
    }

    fn eval_expr(&mut self, expression: &Expression) -> Vec<VersionId> {
        self.eval_expr_inner(expression, true)
    }

    fn guard_expression_sources(&mut self, mut sources: ExpressionSources) -> ExpressionSources {
        sources.normalize();
        if sources.is_empty() {
            return sources;
        }
        let version = self
            .ssa
            .related_definition_guarded(sources.sources, &self.path_condition);
        ExpressionSources {
            sources: vec![(version, PositionRelation::default())],
        }
    }

    fn eval_reachable_expr(&mut self, expression: &Expression) -> Vec<VersionId> {
        self.eval_expr_inner(expression, true)
    }

    fn eval_expr_inner(
        &mut self,
        expression: &Expression,
        prune_constant_branches: bool,
    ) -> Vec<VersionId> {
        let mut reads = Vec::new();
        match expression {
            Expression::Term(factor) => self.eval_factor(factor, &mut reads),
            Expression::Unary(_, expression, _) => {
                reads.extend(self.eval_expr_inner(expression, prune_constant_branches));
            }
            Expression::Binary(left, op, right, _) => {
                reads.extend(self.eval_expr_inner(left, prune_constant_branches));
                let evaluate_right = match (prune_constant_branches, op) {
                    (true, Op::LogicAnd) => self.constant_truth(left) != Some(false),
                    (true, Op::LogicOr) => self.constant_truth(left) != Some(true),
                    _ => true,
                };
                if evaluate_right {
                    reads.extend(self.eval_expr_inner(right, prune_constant_branches));
                }
            }
            Expression::Ternary(condition, left, right, _) => {
                reads.extend(self.eval_expr_inner(condition, prune_constant_branches));
                match prune_constant_branches
                    .then(|| self.constant_truth(condition))
                    .flatten()
                {
                    Some(true) => {
                        reads.extend(self.eval_expr_inner(left, prune_constant_branches));
                    }
                    Some(false) => {
                        reads.extend(self.eval_expr_inner(right, prune_constant_branches));
                    }
                    None => {
                        let branch = self.expression_branch_id(expression);
                        let parent_condition = self.path_condition.clone();

                        let checkpoint = self.ssa.checkpoint();
                        self.path_condition = parent_condition.with_choice(branch, 0);
                        let left = self.eval_expr_inner(left, prune_constant_branches);
                        let left = self.ssa.definition_guarded(left, &self.path_condition);
                        let left_state = self.ssa.capture_and_rollback(checkpoint);

                        let checkpoint = self.ssa.checkpoint();
                        self.path_condition = parent_condition.with_choice(branch, 1);
                        let right = self.eval_expr_inner(right, prune_constant_branches);
                        let right = self.ssa.definition_guarded(right, &self.path_condition);
                        let right_state = self.ssa.capture_and_rollback(checkpoint);

                        self.ssa.merge([&left_state, &right_state]);
                        self.path_condition = parent_condition;
                        reads.push(left);
                        reads.push(right);
                    }
                }
            }
            Expression::Concatenation(parts, _) => {
                for (part, repeat) in parts {
                    reads.extend(self.eval_expr_inner(part, prune_constant_branches));
                    if let Some(repeat) = repeat {
                        reads.extend(self.eval_expr_inner(repeat, prune_constant_branches));
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(value, repeat) => {
                            reads.extend(self.eval_expr_inner(value, prune_constant_branches));
                            if let Some(repeat) = repeat {
                                reads.extend(self.eval_expr_inner(repeat, prune_constant_branches));
                            }
                        }
                        ArrayLiteralItem::Defaul(value) => {
                            reads.extend(self.eval_expr_inner(value, prune_constant_branches));
                        }
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, value) in fields {
                    reads.extend(self.eval_expr_inner(value, prune_constant_branches));
                }
            }
        }
        reads.sort_unstable();
        reads.dedup();
        reads
    }

    fn constant_truth(&mut self, expression: &Expression) -> Option<bool> {
        expression
            .eval_value(&mut self.ctx)
            .and_then(|value| value.to_usize())
            .map(|value| value != 0)
    }

    fn eval_factor(&mut self, factor: &Factor, reads: &mut Vec<VersionId>) {
        match factor {
            Factor::Variable(id, index, select, _) => {
                for expression in index.0.iter().chain(select.0.iter()) {
                    reads.extend(self.eval_expr(expression));
                }
                if let Some((_, expression)) = &select.1 {
                    reads.extend(self.eval_expr(expression));
                }
                reads.extend(self.read_variable(*id, index, select));
            }
            Factor::FunctionCall(call) => reads.extend(self.eval_call(call, &[])),
            Factor::SystemFunctionCall(call) => {
                reads.extend(self.eval_system_call(call, &[], true));
            }
            Factor::Unknown(_) => {
                if self.function_flows.is_empty() {
                    self.status = AnalysisStatus::Barrier;
                }
            }
            Factor::HierVariable(_) | Factor::Value(_) | Factor::Anonymous(_) => {}
        }
    }

    fn eval_call(&mut self, call: &FunctionCall, controls: &[VersionId]) -> Vec<VersionId> {
        self.eval_call_requested(call, controls, None)
    }

    fn eval_call_requested(
        &mut self,
        call: &FunctionCall,
        controls: &[VersionId],
        requested: Option<(ArraySpan, PackedSpan)>,
    ) -> Vec<VersionId> {
        #[cfg(test)]
        if matches!(self.call_caches.last(), Some(None)) {
            FUNCTION_BARRIER_EVALUATIONS.set(FUNCTION_BARRIER_EVALUATIONS.get() + 1);
        }
        let cache_key = std::ptr::from_ref(call);
        if let Some(cached) = self
            .call_caches
            .last()
            .and_then(Option::as_ref)
            .and_then(|cache| cache.calls.get(&cache_key))
            .cloned()
        {
            let result = self.select_call_result(&cached, requested);
            #[cfg(test)]
            FUNCTION_RESULT_VERSIONS.set(FUNCTION_RESULT_VERSIONS.get() + result.len());
            return result;
        }

        let evaluated = self.eval_call_uncached(call, controls);
        let result = self.select_call_result(&evaluated, requested);
        if let Some(Some(cache)) = self.call_caches.last_mut() {
            cache.calls.insert(cache_key, evaluated);
        }
        #[cfg(test)]
        FUNCTION_RESULT_VERSIONS.set(FUNCTION_RESULT_VERSIONS.get() + result.len());
        result
    }

    fn select_call_result(
        &mut self,
        evaluated: &CallResult,
        requested: Option<(ArraySpan, PackedSpan)>,
    ) -> Vec<VersionId> {
        let mut result = Vec::new();
        for (array, regions) in &evaluated.region_groups {
            if requested.is_some_and(|(requested_array, _)| !array.overlaps(requested_array)) {
                continue;
            }
            let requested_packed = requested.map(|(_, packed)| packed);
            let first = requested_packed.map_or(0, |requested| {
                regions.partition_point(|(span, _)| {
                    #[cfg(test)]
                    FUNCTION_RESULT_REGION_PROBES.set(FUNCTION_RESULT_REGION_PROBES.get() + 1);
                    span.end() <= requested.start
                })
            });
            for (span, version) in &regions[first..] {
                #[cfg(test)]
                FUNCTION_RESULT_REGION_PROBES.set(FUNCTION_RESULT_REGION_PROBES.get() + 1);
                if requested_packed.is_some_and(|requested| span.start >= requested.end()) {
                    break;
                }
                if let Some((requested_array, requested_packed)) = requested {
                    if requested_array.intersection(*array) == Some(*array)
                        && requested_packed.intersection(*span) == Some(*span)
                    {
                        result.push(*version);
                    } else {
                        result.extend(self.project_version_sources(
                            *version,
                            requested_array,
                            requested_packed,
                        ));
                    }
                } else {
                    result.push(*version);
                }
            }
        }
        result.extend_from_slice(&evaluated.opaque_sources);
        result.sort_unstable();
        result.dedup();
        result
    }

    fn eval_call_uncached(&mut self, call: &FunctionCall, controls: &[VersionId]) -> CallResult {
        #[cfg(test)]
        FUNCTION_EVALUATIONS.set(FUNCTION_EVALUATIONS.get() + 1);

        let summary = self
            .summaries
            .as_deref_mut()
            .map(|summaries| summaries.get(call, &mut self.ctx));
        match summary {
            Some(FunctionSummaryLookup::Ready(summary)) => {
                return self.apply_function_summary(call, controls, summary.as_ref());
            }
            Some(FunctionSummaryLookup::Recursive) => {
                self.status = AnalysisStatus::Barrier;
                let mut sources = Vec::new();
                for input in call.inputs.values() {
                    sources.extend(self.eval_expr(input));
                }
                return CallResult {
                    region_groups: Vec::new(),
                    opaque_sources: sources,
                };
            }
            Some(FunctionSummaryLookup::Missing) | None => {}
        }

        let receiver_index = self.ctx.functions.get(&call.id).and_then(|function| {
            (!function.path.path.0.is_empty())
                .then(|| call.index.as_deref().map(concrete_var_index))
                .flatten()
        });
        let body = self.ctx.functions.get(&call.id).and_then(|function| {
            if let Some(index) = &call.index {
                function.get_function(index)
            } else {
                function.get_function(&[])
            }
        });
        let Some(body) = body else {
            let mut sources = Vec::new();
            for input in call.inputs.values() {
                sources.extend(self.eval_expr(input));
            }
            for outputs in call.outputs.values() {
                for destination in outputs {
                    self.write_destination(destination, &sources, controls);
                }
            }
            return CallResult {
                region_groups: Vec::new(),
                opaque_sources: sources,
            };
        };
        let mut actual_sources = Vec::new();
        let mut input_bindings = Vec::new();

        for (path, actual) in &call.inputs {
            actual_sources.extend(self.eval_expr(actual));
            let Some(&formal) = body.arg_map.get(path) else {
                continue;
            };
            for key in self.keys_for_id(formal) {
                let mut sources = self.eval_actual_for_formal_key(actual, key);
                sources.normalize();
                input_bindings.push((key, sources));
            }
        }

        let call_frame = self.next_call_frame;
        self.next_call_frame += 1;
        self.call_frames.push(call_frame);

        let mut formal_ids = body.arg_map.values().copied().collect::<Vec<_>>();
        formal_ids.extend(body.ret);
        formal_ids.sort_unstable();
        formal_ids.dedup();
        for formal in formal_ids {
            for key in self.keys_for_id(formal) {
                let version = self.ssa.definition(Vec::new());
                self.bind_key(key, version);
            }
        }
        for (key, sources) in input_bindings {
            let version = self.ssa.related_definition(sources.sources);
            self.bind_key(key, version);
        }

        self.call_caches.push(None);
        self.receiver_indices.push(receiver_index);
        self.eval_function_body(&body.statements, body.ret, controls);
        self.receiver_indices.pop();
        self.call_caches.pop();

        let mut formal_outputs = HashMap::default();
        for (path, _) in &call.outputs {
            let Some(&formal) = body.arg_map.get(path) else {
                continue;
            };
            formal_outputs
                .entry(formal)
                .or_insert_with(|| self.current_key_versions_for_id(formal));
        }
        let region_groups = body
            .ret
            .map(|ret| self.current_region_groups_for_id(ret))
            .unwrap_or_default();

        assert_eq!(self.call_frames.pop(), Some(call_frame));

        for (path, destinations) in &call.outputs {
            let Some(&formal) = body.arg_map.get(path) else {
                continue;
            };
            let formal_versions = formal_outputs
                .get(&formal)
                .map(Vec::as_slice)
                .unwrap_or_default();
            self.write_formal_outputs(destinations, formal_versions, controls);
        }

        let opaque_sources = if statements_have_unknown(&body.statements) {
            actual_sources
        } else {
            Vec::new()
        };
        CallResult {
            region_groups,
            opaque_sources,
        }
    }

    fn apply_function_summary(
        &mut self,
        call: &FunctionCall,
        controls: &[VersionId],
        summary: &FunctionSummary,
    ) -> CallResult {
        self.status = self.status.max(summary.status);
        self.call_caches.push(Some(EvaluationCache::default()));
        for actual in call.inputs.values() {
            self.eval_expr(actual);
        }
        let branch_map = self.instantiate_summary_branches(summary);
        let mut bindings = HashMap::default();
        for node in &summary.graph.nodes {
            let DependencyDagNode::External(key) = node else {
                continue;
            };
            if !bindings.contains_key(key) {
                bindings.insert(
                    *key,
                    self.map_summary_node_source(call, summary, key.node)
                        .sources,
                );
            }
        }

        for (destination, root) in &summary.writes {
            let imported = self.ssa.imported(
                summary.graph.clone(),
                *root,
                bindings.clone(),
                branch_map.clone(),
            );
            let mut sources = ExpressionSources {
                sources: vec![(imported, PositionRelation::default())],
            };
            sources.extend_whole(controls.iter().copied());
            let version = self
                .ssa
                .related_definition_guarded(sources.sources, &self.path_condition);
            self.bind_key(*destination, version);
            self.written.insert(*destination);
        }

        let formal_outputs = call
            .outputs
            .iter()
            .filter_map(|(path, _)| {
                let formal = *summary.arg_map.get(path)?;
                Some((formal, self.current_key_versions_for_id(formal)))
            })
            .collect::<HashMap<_, _>>();

        for (path, destinations) in &call.outputs {
            let Some(&formal) = summary.arg_map.get(path) else {
                continue;
            };
            self.write_formal_outputs(destinations, &formal_outputs[&formal], controls);
        }

        let region_groups = summary
            .result
            .iter()
            .map(|(array, regions)| {
                let regions = regions
                    .iter()
                    .map(|(span, root)| {
                        let imported = self.ssa.imported(
                            summary.graph.clone(),
                            *root,
                            bindings.clone(),
                            branch_map.clone(),
                        );
                        let version = if self.path_condition.is_unconditional() {
                            imported
                        } else {
                            self.ssa.related_definition_guarded(
                                vec![(imported, PositionRelation::default())],
                                &self.path_condition,
                            )
                        };
                        (*span, version)
                    })
                    .collect();
                (*array, regions)
            })
            .collect();
        let opaque_sources = summary
            .opaque_sources
            .iter()
            .flat_map(|source| {
                self.map_summary_node_source(call, summary, *source)
                    .sources
                    .into_iter()
                    .map(|(version, _)| version)
            })
            .collect();
        self.call_caches.pop();
        CallResult {
            region_groups,
            opaque_sources,
        }
    }

    fn map_summary_node_source(
        &mut self,
        call: &FunctionCall,
        summary: &FunctionSummary,
        source: NodeKey,
    ) -> ExpressionSources {
        if self.is_module_scope_key(source) {
            return ExpressionSources {
                sources: vec![(self.read_key(source), PositionRelation::default())],
            };
        }
        let actual = summary.arg_map.iter().find_map(|(path, formal)| {
            (*formal == source.0)
                .then(|| {
                    call.inputs
                        .iter()
                        .find_map(|(actual_path, actual)| (actual_path == path).then_some(actual))
                })
                .flatten()
        });
        let Some(actual) = actual else {
            return ExpressionSources::default();
        };
        self.eval_actual_for_formal_key(actual, source)
    }

    fn instantiate_summary_branches(
        &mut self,
        summary: &FunctionSummary,
    ) -> HashMap<BranchId, BranchId> {
        let mut branches = summary
            .graph
            .edges
            .iter()
            .flat_map(|edge| edge.condition.branches())
            .collect::<Vec<_>>();
        branches.sort_unstable();
        branches.dedup();
        branches
            .into_iter()
            .map(|branch| (branch, self.next_branch_id(branch.arms())))
            .collect()
    }

    fn keys_for_id(&self, id: VarId) -> Vec<NodeKey> {
        let mut keys = self
            .bit_part
            .array_spans(id)
            .iter()
            .flat_map(|index| {
                let ranges = self.bit_part.ranges_of((id, *index));
                (0..ranges.len()).map(move |range| (id, *index, range))
            })
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    fn current_key_versions_for_id(&mut self, id: VarId) -> Vec<(NodeKey, VersionId)> {
        self.keys_for_id(id)
            .into_iter()
            .map(|key| (key, self.read_key(key)))
            .collect()
    }

    fn project_version_sources(
        &mut self,
        version: VersionId,
        requested_array: ArraySpan,
        requested_packed: PackedSpan,
    ) -> Vec<VersionId> {
        if self.ssa.has_structural_dependency(version) {
            return vec![
                self.ssa
                    .projected(version, position_domain(requested_array, requested_packed)),
            ];
        }
        let sources = self.ssa.root_source_relations_guarded(version);
        if sources.is_empty() {
            return vec![version];
        }
        let mut projected = Vec::new();
        for (key, relation, condition) in sources {
            let array_matches = relation.array.is_none_or(|offset| {
                translate_array_span(key.node.1, offset)
                    .is_some_and(|span| span.overlaps(requested_array))
            });
            let packed_matches = relation.packed.is_none_or(|offset| {
                self.key_span(key.node)
                    .and_then(|span| translate_packed_span(span, offset))
                    .is_some_and(|span| span.overlaps(requested_packed))
            });
            if array_matches && packed_matches {
                let source = self.ssa.read(key);
                projected.push(
                    self.ssa
                        .related_definition_guarded(vec![(source, relation)], &condition),
                );
            }
        }
        projected.sort_unstable();
        projected.dedup();
        projected
    }

    fn current_region_groups_for_id(
        &mut self,
        id: VarId,
    ) -> Vec<(ArraySpan, Vec<(PackedSpan, VersionId)>)> {
        let mut groups = Vec::<(ArraySpan, Vec<(PackedSpan, VersionId)>)>::new();
        let mut previous_array_span = None;
        for key in self.keys_for_id(id) {
            let Some(span) = self.key_span(key) else {
                continue;
            };
            if previous_array_span != Some(key.1) {
                groups.push((key.1, Vec::new()));
                previous_array_span = Some(key.1);
            }
            let group = &mut groups.last_mut().expect("pushed above").1;
            debug_assert!(group.last().is_none_or(|(previous, _)| {
                previous.start <= span.start && previous.end() <= span.start
            }));
            group.push((span, self.read_key(key)));
        }
        groups
    }

    fn eval_actual_for_formal_key(
        &mut self,
        actual: &Expression,
        formal_key: NodeKey,
    ) -> ExpressionSources {
        let Some(span) = self.key_span(formal_key) else {
            return ExpressionSources::whole(self.eval_expr(actual));
        };
        self.eval_expr_bits(actual, formal_key.1, span)
    }

    fn write_formal_outputs(
        &mut self,
        destinations: &[AssignDestination],
        formal_versions: &[(NodeKey, VersionId)],
        controls: &[VersionId],
    ) {
        // A concatenated actual is one lvalue. Sample all its selectors after
        // the callee returns, before writing any piece of the concatenation.
        let selectors = destinations
            .iter()
            .map(|destination| {
                let mut sources = controls.to_vec();
                sources.extend(self.eval_destination_selectors(destination));
                sources
            })
            .collect::<Vec<_>>();
        let widths = destinations
            .iter()
            .map(|destination| self.destination_width(destination))
            .collect::<Option<Vec<_>>>();
        if let Some(widths) = widths {
            let total_width = widths.iter().sum();
            let mut offset = total_width;
            for ((destination, width), selectors) in destinations.iter().zip(widths).zip(selectors)
            {
                offset -= width;
                self.write_formal_output(
                    destination,
                    formal_versions,
                    offset,
                    total_width,
                    &selectors,
                );
            }
        } else {
            for (destination, mut sources) in destinations.iter().zip(selectors) {
                sources.extend(formal_versions.iter().map(|(_, version)| *version));
                self.bind_whole_destination(destination, sources, controls);
            }
        }
    }

    fn write_formal_output(
        &mut self,
        destination: &AssignDestination,
        formal_versions: &[(NodeKey, VersionId)],
        formal_offset: usize,
        context_width: usize,
        selectors: &[VersionId],
    ) {
        let formal_type = formal_versions.first().and_then(|(key, _)| {
            self.ctx
                .variables
                .get(&key.0)
                .map(|variable| &variable.r#type)
        });
        let formal_width = formal_type
            .and_then(|ty| ty.total_width())
            .unwrap_or(context_width);
        let signed = formal_type.is_some_and(|ty| ty.signed);
        let variable = self.ctx.variables.get(&destination.id).cloned();
        let selected = if destination.select.is_const_with_range() {
            variable.as_ref().and_then(|variable| {
                destination
                    .select
                    .eval_value(&mut self.ctx, &variable.r#type, false)
            })
        } else {
            None
        };
        let mut selected_destination = destination.clone();
        selected_destination.index = self.receiver_index(destination.id, &destination.index);
        let destination_array = selected_destination
            .index
            .is_const()
            .then(|| {
                dst_writes(&selected_destination, &mut self.ctx)
                    .into_iter()
                    .map(|(array, _)| array)
                    .next()
            })
            .flatten();
        let position_offset = destination_array
            .zip(selected)
            .and_then(|(array, (_, low))| {
                Some(PositionRelation {
                    array: Some(isize::try_from(array.start).ok()?),
                    packed: Some(signed_difference(low, formal_offset)?),
                })
            });
        let dynamic = self.destination_is_dynamic(destination);
        for key in self.write_keys(destination) {
            let mut positional = Vec::new();
            let mut whole = selectors.to_vec();
            if let (Some(destination_array), Some((_, low)), Some(span), Some(position_offset)) = (
                destination_array,
                selected,
                self.key_span(key),
                position_offset,
            ) {
                if let (Some(requested_array), Some(requested)) = (
                    key.1
                        .intersection(destination_array)
                        .and_then(|array| array.translated(destination_array.start, 0)),
                    span.translated(low, formal_offset)
                        .and_then(|span| PackedSpan::whole(context_width)?.intersection(span)),
                ) {
                    for (formal_key, version) in formal_versions {
                        if !formal_key.1.overlaps(requested_array) {
                            continue;
                        }
                        let Some(formal_span) = self.key_span(*formal_key) else {
                            continue;
                        };
                        if formal_span.overlaps(requested) {
                            positional.extend(
                                self.project_version_sources(*version, requested_array, requested)
                                    .into_iter()
                                    .map(|version| (version, position_offset)),
                            );
                        }
                        if signed && context_width > formal_width && formal_width != 0 {
                            let sign = PackedSpan {
                                start: formal_width - 1,
                                length: 1,
                            };
                            let extension = PackedSpan {
                                start: formal_width,
                                length: context_width - formal_width,
                            };
                            if formal_span.overlaps(sign)
                                && let Some(extension) = requested.intersection(extension)
                            {
                                // Replicate only the sign bit, and constrain
                                // the result to the widened portion. Keeping
                                // both projections avoids inventing a path
                                // from the sign to an unchanged lower bit.
                                let sign = self
                                    .ssa
                                    .projected(*version, position_domain(requested_array, sign));
                                let extended = self.ssa.related_definition(vec![(
                                    sign,
                                    PositionRelation {
                                        array: Some(0),
                                        packed: None,
                                    },
                                )]);
                                let extended = self.ssa.projected(
                                    extended,
                                    position_domain(requested_array, extension),
                                );
                                positional.push((extended, position_offset));
                            }
                        }
                    }
                }
            } else {
                whole.extend(formal_versions.iter().map(|(_, version)| *version));
            }
            positional.extend(
                whole
                    .into_iter()
                    .map(|version| (version, PositionRelation::whole())),
            );
            let version = self
                .ssa
                .related_definition_guarded(positional, &self.path_condition);
            self.bind_destination(key, version, dynamic);
        }
    }

    fn eval_system_call(
        &mut self,
        call: &crate::ir::SystemFunctionCall,
        controls: &[VersionId],
        _value_position: bool,
    ) -> Vec<VersionId> {
        match &call.kind {
            SystemFunctionKind::Bits(_)
            | SystemFunctionKind::Size(_)
            | SystemFunctionKind::Clog2(_)
            | SystemFunctionKind::Finish => Vec::new(),
            SystemFunctionKind::Onehot(input)
            | SystemFunctionKind::Signed(input)
            | SystemFunctionKind::Unsigned(input) => self.eval_expr(&input.0),
            SystemFunctionKind::Readmemh(input, output) => {
                let sources = self.eval_expr(&input.0);
                for destination in &output.0 {
                    self.write_destination(destination, &sources, controls);
                }
                Vec::new()
            }
            SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
                for input in inputs {
                    self.eval_expr(&input.0);
                }
                Vec::new()
            }
            SystemFunctionKind::Assert { cond, args, .. } => {
                self.eval_expr(&cond.0);
                for input in args {
                    self.eval_expr(&input.0);
                }
                Vec::new()
            }
        }
    }
}

fn statements_have_unknown(statements: &[Statement]) -> bool {
    statements.iter().any(|statement| match statement {
        Statement::Assign(assign) => expression_has_unknown(&assign.expr),
        Statement::If(statement) => {
            expression_has_unknown(&statement.cond)
                || statements_have_unknown(&statement.true_side)
                || statements_have_unknown(&statement.false_side)
        }
        Statement::Case(statement) => {
            expression_has_unknown(&statement.case_target)
                || statement
                    .arms
                    .iter()
                    .any(|arm| statements_have_unknown(&arm.body))
                || statements_have_unknown(&statement.default)
        }
        Statement::For(statement) => statements_have_unknown(&statement.body),
        _ => false,
    })
}

fn concrete_var_index(index: &[usize]) -> VarIndex {
    VarIndex(
        index
            .iter()
            .map(|index| {
                Expression::create_value(
                    Value::new(*index as u64, 32, false),
                    veryl_parser::token_range::TokenRange::default(),
                )
            })
            .collect(),
    )
}

fn expression_has_unknown(expression: &Expression) -> bool {
    match expression {
        Expression::Term(factor) => matches!(factor.as_ref(), Factor::Unknown(_)),
        Expression::Unary(_, expression, _) => expression_has_unknown(expression),
        Expression::Binary(left, _, right, _) => {
            expression_has_unknown(left) || expression_has_unknown(right)
        }
        Expression::Ternary(condition, left, right, _) => {
            expression_has_unknown(condition)
                || expression_has_unknown(left)
                || expression_has_unknown(right)
        }
        Expression::Concatenation(parts, _) => parts.iter().any(|(expression, repeat)| {
            expression_has_unknown(expression)
                || repeat.as_ref().is_some_and(expression_has_unknown)
        }),
        Expression::ArrayLiteral(items, _) => items.iter().any(|item| match item {
            ArrayLiteralItem::Value(expression, repeat) => {
                expression_has_unknown(expression)
                    || repeat
                        .as_ref()
                        .is_some_and(|expression| expression_has_unknown(expression.as_ref()))
            }
            ArrayLiteralItem::Defaul(expression) => expression_has_unknown(expression),
        }),
        Expression::StructConstructor(_, fields, _) => fields
            .iter()
            .any(|(_, expression)| expression_has_unknown(expression)),
    }
}
