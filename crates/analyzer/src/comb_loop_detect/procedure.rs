//! Analyzer-IR procedure evaluation for combinational dependency extraction.

use super::BitDependency;
use super::region::{ArraySpan, BitPartition, NodeKey, PackedSpan, dst_writes, var_reads};
use super::ssa::{
    BranchId, BranchState, Checkpoint, PathCondition, PositionRelation, SourceCache, SsaStore,
    VersionId,
};
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, CaseStatement, Expression, Factor, ForBound, ForRange,
    ForStatement, FunctionCall, IfStatement, Module, Op, Statement, SystemFunctionKind, VarIndex,
    VarPath, VarSelect,
};
use crate::value::Value;
use crate::{HashMap, HashSet};
use std::rc::Rc;

fn signed_difference(destination: usize, source: usize) -> Option<isize> {
    isize::try_from(destination)
        .ok()?
        .checked_sub(isize::try_from(source).ok()?)
}

fn translated_start(start: usize, offset: isize) -> Option<usize> {
    if offset >= 0 {
        start.checked_add(usize::try_from(offset).ok()?)
    } else {
        start.checked_sub(offset.unsigned_abs())
    }
}

fn translate_array_span(span: ArraySpan, offset: isize) -> Option<ArraySpan> {
    Some(ArraySpan {
        start: translated_start(span.start, offset)?,
        length: span.length,
    })
}

fn translate_packed_span(span: PackedSpan, offset: isize) -> Option<PackedSpan> {
    PackedSpan::new(translated_start(span.start, offset)?, span.length)
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

    fn evaluate(&self, values: &[(VarId, isize)]) -> Option<usize> {
        let mut result = self.constant;
        for &(id, coefficient) in &self.terms {
            let value = values
                .iter()
                .rev()
                .find_map(|(candidate, value)| (*candidate == id).then_some(*value))?;
            result = result.checked_add(coefficient.checked_mul(value)?)?;
        }
        usize::try_from(result).ok()
    }

    fn solve_for(
        &self,
        variable: VarId,
        target: usize,
        values: &[(VarId, isize)],
    ) -> Option<usize> {
        let mut remainder = isize::try_from(target).ok()?.checked_sub(self.constant)?;
        let mut variable_coefficient = None;
        for &(id, coefficient) in &self.terms {
            if id == variable {
                variable_coefficient = Some(coefficient);
            } else {
                let value = values
                    .iter()
                    .rev()
                    .find_map(|(candidate, value)| (*candidate == id).then_some(*value))?;
                remainder = remainder.checked_sub(coefficient.checked_mul(value)?)?;
            }
        }
        let coefficient = variable_coefficient?;
        if coefficient == 0 || remainder % coefficient != 0 {
            return None;
        }
        usize::try_from(remainder / coefficient).ok()
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

fn runtime_bound_end(expression: &Expression, inclusive: bool) -> Option<usize> {
    let r#type = &expression.comptime().r#type;
    let width = r#type.total_width()?;
    let value_bits = width.checked_sub(usize::from(r#type.signed))?;
    if value_bits >= usize::BITS as usize {
        return None;
    }
    let maximum = (1usize << value_bits).checked_sub(1)?;
    maximum.checked_add(usize::from(inclusive))
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
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
    result: FunctionResultSummary,
    writes: Vec<(NodeKey, Vec<SummarySource>)>,
    opaque_sources: Vec<NodeKey>,
    complete: bool,
}

type FunctionResultSummary = Vec<(ArraySpan, Vec<(PackedSpan, Vec<SummarySource>)>)>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SummarySource {
    key: NodeKey,
    relation: PositionRelation,
    condition: PathCondition,
}

pub(super) struct FunctionSummaries<'a> {
    module: &'a Module,
    bit_part: &'a BitPartition,
    summaries: HashMap<FunctionSummaryKey, Option<Rc<FunctionSummary>>>,
    context: Option<ProcedureContext>,
}

/// Reusable module-local evaluation context for independent procedural
/// declarations. Its large variable and function maps are built once per
/// module; every analysis still gets fresh SSA and control-flow state.
pub(super) struct ProcedureContext {
    ctx: Option<Context>,
}

impl ProcedureContext {
    pub(super) fn new(module: &Module) -> Self {
        let mut ctx = Context::default();
        ctx.variables = module.variables.clone();
        ctx.functions = module.functions.clone();
        #[cfg(test)]
        MODULE_CONTEXT_ENTRIES
            .set(MODULE_CONTEXT_ENTRIES.get() + ctx.variables.len() + ctx.functions.len());
        Self { ctx: Some(ctx) }
    }

    fn take(&mut self) -> Context {
        let mut ctx = self.ctx.take().expect("procedure context is not reentrant");
        ctx.begin_analysis_transaction();
        ctx
    }

    fn restore(&mut self, mut ctx: Context) {
        ctx.rollback_analysis_transaction();
        debug_assert!(self.ctx.is_none());
        self.ctx = Some(ctx);
    }
}

impl<'a> FunctionSummaries<'a> {
    pub(super) fn new(module: &'a Module, bit_part: &'a BitPartition) -> Self {
        Self {
            module,
            bit_part,
            summaries: HashMap::default(),
            context: None,
        }
    }

    fn get(&mut self, call: &FunctionCall) -> Option<Rc<FunctionSummary>> {
        let key = FunctionSummaryKey {
            id: call.id,
            index: call.index.clone(),
        };
        if let Some(summary) = self.summaries.get(&key) {
            return summary.clone();
        }
        let context = self
            .context
            .get_or_insert_with(|| ProcedureContext::new(self.module));
        let summary = ProcedureAnalysis::summarize_function(
            self.module,
            self.bit_part,
            call.id,
            call.index.as_deref(),
            context,
        )
        .map(Rc::new);
        self.summaries.insert(key, summary.clone());
        summary
    }
}

#[cfg(test)]
thread_local! {
    static FUNCTION_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_RESULT_VERSIONS: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_RESULT_REGION_PROBES: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_BARRIER_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    static MODULE_CONTEXT_ENTRIES: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_function_evaluation_count() {
    FUNCTION_EVALUATIONS.set(0);
    FUNCTION_RESULT_VERSIONS.set(0);
    FUNCTION_RESULT_REGION_PROBES.set(0);
    FUNCTION_BARRIER_EVALUATIONS.set(0);
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
pub(crate) fn reset_module_context_entries() {
    MODULE_CONTEXT_ENTRIES.set(0);
}

#[cfg(test)]
pub(crate) fn module_context_entries() -> usize {
    MODULE_CONTEXT_ENTRIES.get()
}

pub(super) fn analyze(
    bit_part: &BitPartition,
    statements: &[Statement],
    branch_namespace: usize,
    context: &mut ProcedureContext,
) -> ProcedureResult {
    ProcedureAnalysis::analyze(bit_part, statements, branch_namespace, context)
}

pub(super) struct ProcedureResult {
    pub(super) dependencies: Vec<Dependency>,
    pub(super) complete: bool,
}

pub(super) struct Dependency {
    pub(super) source: NodeKey,
    pub(super) destination: NodeKey,
    pub(super) kind: BitDependency,
    pub(super) condition: PathCondition,
}

#[derive(Default)]
struct ExpressionSources {
    positional: Vec<(VersionId, PositionRelation)>,
    whole: Vec<VersionId>,
}

impl ExpressionSources {
    fn whole(versions: Vec<VersionId>) -> Self {
        Self {
            positional: Vec::new(),
            whole: versions,
        }
    }

    fn extend(&mut self, other: Self) {
        self.positional.extend(other.positional);
        self.whole.extend(other.whole);
    }

    fn translate(&mut self, offset: PositionRelation) {
        for (_, current) in &mut self.positional {
            *current = current.compose(offset);
        }
    }

    fn forget_array_position(&mut self) {
        for (_, relation) in &mut self.positional {
            relation.array = None;
        }
    }

    fn forget_packed_position(&mut self) {
        for (_, relation) in &mut self.positional {
            relation.packed = None;
        }
    }

    fn normalize(&mut self) {
        self.positional.sort_unstable();
        let mut merged: Vec<(VersionId, PositionRelation)> =
            Vec::with_capacity(self.positional.len());
        for (source, relation) in self.positional.drain(..) {
            if let Some((previous_source, previous_relation)) = merged.last_mut()
                && *previous_source == source
            {
                *previous_relation = previous_relation.union(relation);
            } else {
                merged.push((source, relation));
            }
        }
        self.positional = merged;
        self.whole.sort_unstable();
        self.whole.dedup();
        self.positional
            .retain(|(source, _)| self.whole.binary_search(source).is_err());
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
        let mut inner = ProcedureAnalysis::from_context(bit_part, context.take());
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
        array: super::region::ArraySpan,
        packed: PackedSpan,
        context_width: usize,
    ) -> Vec<RegionSource> {
        let inner = self.inner.as_mut().expect("expression analysis is active");
        inner.use_expression_namespace(expression);
        let mut sources = inner.eval_expr_requested(expression, array, packed, context_width);
        sources.normalize();
        let mut mapped = Vec::new();
        for (version, expression_offset) in sources.positional {
            let wrapper = inner
                .ssa
                .positional_definition(vec![(version, expression_offset)], Vec::new());
            for (source, relation, condition) in inner.ssa.root_source_relations_guarded(wrapper) {
                if source.call_frame.is_some() {
                    continue;
                }
                let source = source.node;
                let relation = relation.reversed();
                let offset = relation.array.zip(relation.packed);
                mapped.push(RegionSource {
                    key: source,
                    offset,
                    condition,
                });
            }
        }
        for version in sources.whole {
            let wrapper = inner.ssa.definition(vec![version]);
            mapped.extend(
                inner
                    .ssa
                    .root_source_relations_guarded(wrapper)
                    .into_iter()
                    .filter_map(|(source, _, condition)| {
                        source.call_frame.is_none().then_some(RegionSource {
                            key: source.node,
                            offset: None,
                            condition,
                        })
                    }),
            );
        }
        mapped.sort_unstable_by_key(|source| (source.key, source.offset, source.condition.clone()));
        mapped.dedup_by(|left, right| {
            left.key == right.key
                && left.offset == right.offset
                && left.condition == right.condition
        });
        mapped
    }

    pub(super) fn dependencies(&mut self) -> Vec<Dependency> {
        self.inner().dependencies()
    }

    pub(super) fn restore(mut self, context: &mut ProcedureContext) {
        let inner = self.inner.take().expect("expression analysis is active");
        context.restore(inner.ctx);
    }

    pub(super) fn is_complete(&self) -> bool {
        self.inner
            .as_ref()
            .expect("expression analysis is active")
            .complete
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
    ctx: Context,
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
    complete: bool,
    requested_array_index: Option<AffineIndex>,
    requested_destination_array: Option<ArraySpan>,
    runtime_loop_values: Vec<(VarId, isize)>,
    summaries: Option<&'s mut FunctionSummaries<'a>>,
}

impl<'a, 's> ProcedureAnalysis<'a, 's> {
    fn from_context(bit_part: &'a BitPartition, ctx: Context) -> Self {
        Self {
            bit_part,
            ctx,
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
            complete: true,
            requested_array_index: None,
            requested_destination_array: None,
            runtime_loop_values: Vec::new(),
            summaries: None,
        }
    }

    fn analyze(
        bit_part: &'a BitPartition,
        statements: &[Statement],
        branch_namespace: usize,
        context: &mut ProcedureContext,
    ) -> ProcedureResult {
        let mut this = Self::from_context(bit_part, context.take());
        this.branch_namespace = branch_namespace;
        this.eval_block(statements, &[]);
        let result = ProcedureResult {
            dependencies: this.dependencies(),
            complete: this.complete,
        };
        context.restore(this.ctx);
        result
    }

    fn eval_expression_sources(&mut self, expression: &Expression) -> Vec<RegionSource> {
        let versions = self.eval_reachable_expr(expression);
        let value = self.ssa.definition(versions);
        let mut sources = self
            .ssa
            .root_source_relations_guarded(value)
            .into_iter()
            .filter_map(|(source, _, condition)| {
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
    ) -> Option<FunctionSummary> {
        let function = module.functions.get(&id)?;
        let body = function.get_function(index.unwrap_or_default())?;
        let formal_ids = body.arg_map.values().copied().collect::<HashSet<_>>();
        let mut this = Self::from_context(bit_part, context.take());
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
        let mut source_cache = SourceCache::restricted(
            visible_keys
                .into_iter()
                .map(|node| SsaKey { node, call_frame: None }),
        );
        let mut visible_sources = |this: &Self, version| {
            let mut sources = this
                .ssa
                .root_source_relations_guarded_cached(version, &mut source_cache)
                .into_iter()
                .filter_map(|(source, relation, condition)| {
                    source.call_frame.is_none().then_some(SummarySource {
                        key: source.node,
                        relation,
                        condition,
                    })
                })
                .filter(|source| {
                    formal_ids.contains(&source.key.0)
                        || this.is_module_scope_key(source.key)
                })
                .collect::<Vec<_>>();
            sources.sort_unstable();
            sources.dedup();
            sources
        };

        let result = body
            .ret
            .map(|ret| {
                this.current_region_groups_for_id(ret)
                    .into_iter()
                    .map(|(array, regions)| {
                        let regions = regions
                            .into_iter()
                            .map(|(span, version)| (span, visible_sources(&this, version)))
                            .collect();
                        (array, regions)
                    })
                    .collect()
            })
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
        let writes = destinations
            .into_iter()
            .map(|destination| {
                let version = this.read_key(destination);
                (destination, visible_sources(&this, version))
            })
            .collect();

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
            result,
            writes,
            opaque_sources,
            complete: this.complete,
        };
        context.restore(this.ctx);
        Some(summary)
    }

    fn dependencies(&mut self) -> Vec<Dependency> {
        let mut dependencies = Vec::new();
        let mut source_cache = SourceCache::restricted(
            self.module_scope_keys()
                .into_iter()
                .map(|node| SsaKey { node, call_frame: None }),
        );
        let destinations: Vec<_> = self
            .written
            .iter()
            .copied()
            .filter(|key| self.is_module_scope_key(*key))
            .collect();
        for destination in destinations {
            let version = self.read_key(destination);
            let sources = self
                .ssa
                .root_source_relations_guarded_cached(version, &mut source_cache);
            dependencies.extend(
                sources
                    .into_iter()
                    .filter_map(|(source, source_kind, condition)| {
                        source
                            .call_frame
                            .is_none()
                            .then_some((source.node, source_kind, condition))
                    })
                    .filter(|(source, _, _)| self.is_module_scope_key(*source))
                    .map(|(source, source_kind, condition)| Dependency {
                        source,
                        destination,
                        kind: Self::dependency_kind(source_kind),
                        condition,
                    }),
            );
        }
        dependencies.sort_unstable_by_key(|dependency| {
            (
                dependency.source,
                dependency.destination,
                dependency.condition.clone(),
            )
        });
        dependencies
    }

    fn dependency_kind(source_kind: PositionRelation) -> BitDependency {
        BitDependency {
            array: source_kind.array,
            packed: source_kind.packed,
        }
    }

    fn is_module_scope_key(&self, key: NodeKey) -> bool {
        self.ctx.variables.get(&key.0).is_none_or(|variable| {
            matches!(
                variable.affiliation,
                crate::symbol::Affiliation::Module | crate::symbol::Affiliation::Interface
            )
        })
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

    fn module_scope_keys(&self) -> Vec<NodeKey> {
        let mut keys = self
            .ctx
            .variables
            .iter()
            .filter(|(_, variable)| {
                matches!(
                    variable.affiliation,
                    crate::symbol::Affiliation::Module | crate::symbol::Affiliation::Interface
                )
            })
            .flat_map(|(&id, _)| self.keys_for_id(id))
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn read_keys(&mut self, id: VarId, index: &VarIndex, select: &VarSelect) -> Vec<NodeKey> {
        let mut keys = Vec::new();
        let index = self.receiver_index(id, index);
        let accesses = var_reads(id, &index, select, &mut self.ctx);
        if accesses.is_empty() {
            self.complete = false;
        }
        for (idx, span) in accesses {
            keys.extend(self.bit_part.overlapping_access(id, idx, span));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn write_keys(&mut self, destination: &AssignDestination) -> Vec<NodeKey> {
        let mut keys = Vec::new();
        let mut destination = destination.clone();
        destination.index = self.receiver_index(destination.id, &destination.index);
        let concrete_array = self
            .flattened_affine_index(destination.id, &destination.index)
            .and_then(|index| index.evaluate(&self.runtime_loop_values))
            .map(|start| ArraySpan { start, length: 1 });
        let accesses = dst_writes(&destination, &mut self.ctx);
        if accesses.is_empty() {
            self.complete = false;
        }
        for (idx, span) in accesses {
            keys.extend(self.bit_part.overlapping_access(
                destination.id,
                concrete_array.unwrap_or(idx),
                span,
            ));
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

    fn write_destination(
        &mut self,
        destination: &AssignDestination,
        sources: &[VersionId],
        controls: &[VersionId],
    ) {
        let mut dependencies = sources.to_vec();
        dependencies.extend_from_slice(controls);
        for expression in destination
            .index
            .0
            .iter()
            .chain(destination.select.0.iter())
        {
            dependencies.extend(self.eval_expr(expression));
        }
        if let Some((_, expression)) = &destination.select.1 {
            dependencies.extend(self.eval_expr(expression));
        }
        let keys = self.write_keys(destination);
        let dynamic = self.destination_is_dynamic(destination);
        let version = self
            .ssa
            .definition_guarded(dependencies, &self.path_condition);
        for key in keys {
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
        let destination_array = dst_writes(destination, &mut self.ctx)
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
        let dynamic_array = !destination.index.is_const()
            && destination_index
                .as_ref()
                .and_then(|index| index.evaluate(&self.runtime_loop_values))
                .is_none();
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
                    let previous_index = std::mem::replace(
                        &mut self.requested_array_index,
                        destination_index.clone(),
                    );
                    let previous_destination =
                        self.requested_destination_array.replace(destination_region);
                    let sources = self.eval_expr_requested(
                        expression,
                        array,
                        packed,
                        expression_context_width,
                    );
                    self.requested_array_index = previous_index;
                    self.requested_destination_array = previous_destination;
                    sources
                } else {
                    ExpressionSources::default()
                }
            } else {
                ExpressionSources::whole(self.eval_expr(expression))
            };
            sources.whole.extend(whole);
            sources.normalize();
            let mut positional = Vec::new();
            for (version, offset) in sources.positional {
                if let Some(offset) = destination_offset.map(|base| offset.compose(base)) {
                    positional.push((version, offset));
                } else {
                    sources.whole.push(version);
                }
            }
            let version = self.ssa.positional_definition_guarded(
                positional,
                sources.whole,
                &self.path_condition,
            );
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
        if flow != ProcedureFlow::Return {
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
            PathCondition::intersection(states.iter().map(|state| &state.condition));
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

    fn eval_block(&mut self, statements: &[Statement], controls: &[VersionId]) -> ProcedureFlow {
        for statement in statements {
            let flow = self.eval_statement(statement, controls);
            if flow != ProcedureFlow::Continue {
                return flow;
            }
        }
        ProcedureFlow::Continue
    }

    fn eval_statement(&mut self, statement: &Statement, controls: &[VersionId]) -> ProcedureFlow {
        match statement {
            Statement::Assign(assign) => {
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
                if self.is_return_assignment(&assign.dst) {
                    self.record_return();
                    ProcedureFlow::Return
                } else {
                    ProcedureFlow::Continue
                }
            }
            Statement::If(statement) => self.eval_if(statement, controls),
            Statement::Case(statement) => self.eval_case(statement, controls),
            Statement::For(statement) => self.eval_for(statement, controls),
            Statement::FunctionCall(call) => {
                self.eval_call(call, controls);
                ProcedureFlow::Continue
            }
            Statement::SystemFunctionCall(call) => {
                self.eval_system_call(call, controls, false);
                ProcedureFlow::Continue
            }
            Statement::Break => {
                self.record_break();
                ProcedureFlow::Break
            }
            Statement::IfReset(_) | Statement::TbMethodCall(_) | Statement::Null => {
                ProcedureFlow::Continue
            }
            Statement::Unsupported(_) => {
                self.complete = false;
                ProcedureFlow::Continue
            }
        }
    }

    fn merge_branches(
        &mut self,
        branches: Vec<(ProcedureFlow, BranchState<SsaKey>, PathCondition)>,
    ) -> ProcedureFlow {
        let mut continuation = Vec::new();
        let mut continuation_conditions = Vec::new();
        let mut has_continue = false;
        let mut has_break = false;
        for (flow, state, condition) in branches {
            match flow {
                ProcedureFlow::Continue => {
                    has_continue = true;
                    continuation.push(state);
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
            self.path_condition = PathCondition::intersection(&continuation_conditions);
            ProcedureFlow::Continue
        } else if has_break {
            ProcedureFlow::Break
        } else {
            ProcedureFlow::Return
        }
    }

    fn eval_if(&mut self, statement: &IfStatement, controls: &[VersionId]) -> ProcedureFlow {
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
        self.merge_branches(vec![
            (true_flow, true_state, true_condition),
            (false_flow, false_state, false_condition),
        ])
    }

    fn eval_case(&mut self, statement: &CaseStatement, controls: &[VersionId]) -> ProcedureFlow {
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
        nested_controls.extend(condition);

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
            return self.merge_branches(states);
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
        self.merge_branches(states)
    }

    fn runtime_loop_candidates(&mut self, statement: &ForStatement) -> Vec<usize> {
        let mut destinations = Vec::new();
        collect_assign_destinations(&statement.body, &mut destinations);
        let mut candidates = HashSet::default();
        for destination in destinations {
            let Some(index) = self.flattened_affine_index(destination.id, &destination.index)
            else {
                continue;
            };
            for span in self.bit_part.array_spans(destination.id) {
                for target in [
                    Some(span.start),
                    span.end().and_then(|end| end.checked_sub(1)),
                ]
                .into_iter()
                .flatten()
                {
                    if let Some(candidate) =
                        index.solve_for(statement.var_id, target, &self.runtime_loop_values)
                    {
                        candidates.insert(candidate);
                    }
                }
            }
        }

        let (start, end, step, reverse) = match &statement.range {
            ForRange::Forward {
                start,
                end,
                inclusive,
                step,
            }
            | ForRange::Reverse {
                start,
                end,
                inclusive,
                step,
            } => (
                match start {
                    ForBound::Const(value) => Some(*value),
                    ForBound::Expression(_) => None,
                },
                match end {
                    ForBound::Const(value) => Some(value.saturating_add(usize::from(*inclusive))),
                    ForBound::Expression(expression) => runtime_bound_end(expression, *inclusive),
                },
                Some(*step),
                matches!(statement.range, ForRange::Reverse { .. }),
            ),
            ForRange::Stepped {
                start,
                end,
                inclusive,
                ..
            } => (
                match start {
                    ForBound::Const(value) => Some(*value),
                    ForBound::Expression(_) => None,
                },
                match end {
                    ForBound::Const(value) => Some(value.saturating_add(usize::from(*inclusive))),
                    ForBound::Expression(expression) => runtime_bound_end(expression, *inclusive),
                },
                None,
                false,
            ),
        };
        candidates.retain(|candidate| {
            start.is_none_or(|start| *candidate >= start)
                && end.is_none_or(|end| *candidate < end)
                && step.is_none_or(|step| {
                    step != 0
                        && start.is_none_or(|start| candidate.saturating_sub(start) % step == 0)
                })
        });
        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        candidates.sort_unstable();
        if reverse {
            candidates.reverse();
        }
        candidates
    }

    fn eval_for(&mut self, statement: &ForStatement, controls: &[VersionId]) -> ProcedureFlow {
        let mut range_controls = controls.to_vec();
        let bounds = match &statement.range {
            ForRange::Forward { start, end, .. }
            | ForRange::Reverse { start, end, .. }
            | ForRange::Stepped { start, end, .. } => [start, end],
        };
        for bound in bounds {
            if let ForBound::Expression(expression) = bound {
                range_controls.extend(self.eval_expr(expression));
            }
        }

        if let Some(iterations) = statement.range.eval_iter(&mut self.ctx) {
            let parent_condition = self.path_condition.clone();
            let checkpoint = self.ssa.checkpoint();
            self.loop_flows.push(LoopFlow {
                checkpoint,
                breaks: Vec::new(),
            });
            let mut flow = ProcedureFlow::Continue;
            for value in iterations {
                if let Some(variable) = self.ctx.variable_mut(&statement.var_id)
                    && let Some(width) = statement.var_type.total_width()
                {
                    variable.set_value(
                        &[],
                        Value::new(value as u64, width, statement.var_type.signed),
                        None,
                    );
                }
                flow = self.eval_block(&statement.body, &range_controls);
                if flow != ProcedureFlow::Continue {
                    break;
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
                return ProcedureFlow::Return;
            }
            self.merge_flow_states(&loop_flow.breaks);
            return ProcedureFlow::Continue;
        }

        // Runtime loops have a zero-trip path. Split only at sparse partition
        // boundaries touched by affine iterator expressions. This preserves
        // iteration and statement order without scaling with the declared
        // array shape. Non-affine bodies retain one conservative traversal.
        let parent_condition = self.path_condition.clone();
        let checkpoint = self.ssa.checkpoint();
        self.loop_flows.push(LoopFlow {
            checkpoint,
            breaks: Vec::new(),
        });
        let candidates = self.runtime_loop_candidates(statement);
        let mut flow = ProcedureFlow::Continue;
        if candidates.is_empty() {
            flow = self.eval_block(&statement.body, &range_controls);
        } else {
            for candidate in candidates {
                if let Some(variable) = self.ctx.variables.get_mut(&statement.var_id)
                    && let Some(width) = statement.var_type.total_width()
                {
                    variable.set_value(
                        &[],
                        Value::new(candidate as u64, width, statement.var_type.signed),
                        None,
                    );
                }
                let Some(candidate) = isize::try_from(candidate).ok() else {
                    continue;
                };
                self.runtime_loop_values.push((statement.var_id, candidate));
                flow = self.eval_block(&statement.body, &range_controls);
                self.runtime_loop_values.pop();
                if flow != ProcedureFlow::Continue {
                    break;
                }
            }
        }
        let mut loop_flow = self.loop_flows.pop().expect("loop flow was pushed above");
        let body_state = self.ssa.capture_and_rollback(checkpoint);
        loop_flow.breaks.push(FlowState {
            state: BranchState::unchanged(),
            condition: parent_condition,
        });
        if flow == ProcedureFlow::Continue {
            loop_flow.breaks.push(FlowState {
                state: body_state,
                condition: self.path_condition.clone(),
            });
        }
        self.merge_flow_states(&loop_flow.breaks);
        ProcedureFlow::Continue
    }

    fn eval_expr_requested(
        &mut self,
        expression: &Expression,
        requested_array: ArraySpan,
        requested: PackedSpan,
        context_width: usize,
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
            .map(|span| self.eval_expr_bits(expression, requested_array, span))
            .unwrap_or_default();
        if context_width > expression_width
            && expression.comptime().r#type.signed
            && requested.end() > expression_width
            && expression_width != 0
        {
            let mut sign = self.eval_expr_bits(
                expression,
                requested_array,
                PackedSpan {
                    start: expression_width - 1,
                    length: 1,
                },
            );
            sign.whole
                .extend(sign.positional.drain(..).map(|(version, _)| version));
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
                        let accesses = var_reads(*id, index, select, &mut self.ctx);
                        let dynamic_array_offset = self
                            .requested_array_index
                            .clone()
                            .filter(|destination| !destination.terms.is_empty())
                            .and_then(|destination| {
                                let source = self.flattened_affine_index(*id, index)?;
                                destination.destination_offset_from(&source)
                            });
                        let position_preserving =
                            index.0.iter().all(|index| index.comptime().is_const)
                                && accesses.len() == 1;
                        if let Some(source_span) = requested.translated(0, low) {
                            for (idx, access) in &accesses {
                                let source_array = if let Some(offset) = dynamic_array_offset {
                                    offset
                                        .checked_neg()
                                        .and_then(|offset| {
                                            translate_array_span(
                                                self.requested_destination_array
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
                            ExpressionSources {
                                positional: reads
                                    .into_iter()
                                    .map(|version| (version, offset))
                                    .collect(),
                                whole: selector_sources,
                            }
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
                        self.eval_expr_bits(&input.0, requested_array, requested)
                    }
                    _ => ExpressionSources::whole(self.eval_system_call(call, &[], true)),
                },
                Factor::FunctionCall(call) => ExpressionSources {
                    positional: self
                        .eval_call_requested(call, &[], Some((requested_array, requested)))
                        .into_iter()
                        .map(|version| (version, PositionRelation::default()))
                        .collect(),
                    whole: Vec::new(),
                },
                Factor::Unknown(_) => {
                    if self.function_flows.is_empty() {
                        self.complete = false;
                    }
                    ExpressionSources::default()
                }
                Factor::HierVariable(_) | Factor::Value(_) | Factor::Anonymous(_) => {
                    ExpressionSources::default()
                }
            },
            Expression::Unary(op, operand, _) => match op {
                Op::BitNot | Op::Add => self.eval_expr_bits(operand, requested_array, requested),
                _ => ExpressionSources::whole(self.eval_expr(operand)),
            },
            Expression::Binary(left, op, right, comptime) => match op {
                Op::As => {
                    let context_width = comptime.r#type.total_width().unwrap_or(requested.end());
                    self.eval_expr_requested(left, requested_array, requested, context_width)
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
                            let mut input = self.eval_expr_bits(left, requested_array, input);
                            if let Ok(shift) = isize::try_from(shift) {
                                input.translate(PositionRelation {
                                    array: Some(0),
                                    packed: Some(shift),
                                });
                            } else {
                                input
                                    .whole
                                    .extend(input.positional.drain(..).map(|(version, _)| version));
                            }
                            reads.extend(input);
                        }
                    } else {
                        reads.whole.extend(self.eval_expr(left));
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
                            let mut input = self.eval_expr_bits(left, requested_array, input);
                            if let Ok(shift) = isize::try_from(shift) {
                                input.translate(PositionRelation {
                                    array: Some(0),
                                    packed: Some(-shift),
                                });
                            } else {
                                input
                                    .whole
                                    .extend(input.positional.drain(..).map(|(version, _)| version));
                            }
                            reads.extend(input);
                        }
                        if *op == Op::ArithShiftR
                            && left.comptime().r#type.signed
                            && width != 0
                            && shifted.is_some_and(|shifted| shifted.end() > width)
                        {
                            let mut sign = self.eval_expr_bits(
                                left,
                                requested_array,
                                PackedSpan {
                                    start: width - 1,
                                    length: 1,
                                },
                            );
                            sign.whole
                                .extend(sign.positional.drain(..).map(|(version, _)| version));
                            reads.extend(sign);
                        }
                    } else {
                        reads.whole.extend(self.eval_expr(left));
                    }
                    reads
                }
                Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
                    let context_width = comptime.r#type.total_width().unwrap_or(requested.end());
                    let mut reads =
                        self.eval_expr_requested(left, requested_array, requested, context_width);
                    reads.extend(self.eval_expr_requested(
                        right,
                        requested_array,
                        requested,
                        context_width,
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
                        Some(true) => reads.whole.extend(self.eval_expr(right)),
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
                            reads.whole.push(right);
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
                    Some(true) => reads.extend(self.eval_expr_requested(
                        left,
                        requested_array,
                        requested,
                        context_width,
                    )),
                    Some(false) => reads.extend(self.eval_expr_requested(
                        right,
                        requested_array,
                        requested,
                        context_width,
                    )),
                    None => {
                        let branch = self.expression_branch_id(expression);
                        let parent_condition = self.path_condition.clone();

                        let checkpoint = self.ssa.checkpoint();
                        self.path_condition = parent_condition.with_choice(branch, 0);
                        let left = self.eval_expr_requested(
                            left,
                            requested_array,
                            requested,
                            context_width,
                        );
                        let left = self.guard_expression_sources(left);
                        let left_state = self.ssa.capture_and_rollback(checkpoint);

                        let checkpoint = self.ssa.checkpoint();
                        self.path_condition = parent_condition.with_choice(branch, 1);
                        let right = self.eval_expr_requested(
                            right,
                            requested_array,
                            requested,
                            context_width,
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
                        reads.whole.extend(self.eval_expr_inner(repeat, false));
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
                            let Some(local) = PackedSpan::new(local_start, length) else {
                                continue;
                            };
                            let mut part = self.eval_expr_bits(part, requested_array, local);
                            if let Ok(output_start) = isize::try_from(output_start) {
                                part.translate(PositionRelation {
                                    array: Some(0),
                                    packed: Some(output_start),
                                });
                            } else {
                                part.whole
                                    .extend(part.positional.drain(..).map(|(version, _)| version));
                            }
                            reads.extend(part);
                        }
                        RepeatedProjection::Multiple => {
                            let Some(local) = PackedSpan::whole(width) else {
                                reads.whole.extend(self.eval_expr(part));
                                continue;
                            };
                            let mut part = self.eval_expr_bits(part, requested_array, local);
                            part.forget_packed_position();
                            reads.extend(part);
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
                        reads.whole.extend(self.eval_expr_inner(repeat, false));
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
                            let mut item = self.eval_expr_requested(
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
                            );
                            if let Ok(output_start) = isize::try_from(output_start) {
                                item.translate(PositionRelation {
                                    array: Some(output_start),
                                    packed: Some(0),
                                });
                            } else {
                                item.whole
                                    .extend(item.positional.drain(..).map(|(version, _)| version));
                            }
                            reads.extend(item);
                        }
                        RepeatedProjection::Multiple => {
                            let mut item = self.eval_expr_requested(
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
                            let mut item = self.eval_expr_requested(
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
                            );
                            if let Ok(output_start) = isize::try_from(output_start) {
                                item.translate(PositionRelation {
                                    array: Some(output_start),
                                    packed: Some(0),
                                });
                            } else {
                                item.whole
                                    .extend(item.positional.drain(..).map(|(version, _)| version));
                            }
                            reads.extend(item);
                        }
                        RepeatedProjection::Multiple => {
                            let mut item = self.eval_expr_requested(
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
                        let mut field =
                            self.eval_expr_requested(value, requested_array, local, width);
                        if let Ok(low) = isize::try_from(low) {
                            field.translate(PositionRelation {
                                array: Some(0),
                                packed: Some(low),
                            });
                        } else {
                            field
                                .whole
                                .extend(field.positional.drain(..).map(|(version, _)| version));
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
        self.eval_expr_inner(expression, false)
    }

    fn guard_expression_sources(&mut self, mut sources: ExpressionSources) -> ExpressionSources {
        sources.normalize();
        if sources.positional.is_empty() && sources.whole.is_empty() {
            return sources;
        }
        let version = self.ssa.positional_definition_guarded(
            sources.positional,
            sources.whole,
            &self.path_condition,
        );
        ExpressionSources {
            positional: vec![(version, PositionRelation::default())],
            whole: Vec::new(),
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
                    self.complete = false;
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
                    result.extend(self.project_version_sources(
                        *version,
                        requested_array,
                        requested_packed,
                    ));
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
            .and_then(|summaries| summaries.get(call));
        if let Some(summary) = summary {
            return self.apply_function_summary(call, controls, summary.as_ref());
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
            let version = self
                .ssa
                .positional_definition(sources.positional, sources.whole);
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
            let widths: Vec<_> = destinations
                .iter()
                .map(|destination| self.destination_width(destination))
                .collect();
            if widths.iter().all(Option::is_some) {
                let total_width = widths.iter().flatten().sum();
                let mut offset = total_width;
                for (destination, width) in destinations.iter().zip(widths) {
                    let width = width.expect("checked above");
                    offset -= width;
                    self.write_formal_output(
                        destination,
                        formal_versions,
                        offset,
                        total_width,
                        controls,
                    );
                }
            } else {
                let sources = formal_versions
                    .iter()
                    .map(|(_, version)| *version)
                    .collect::<Vec<_>>();
                for destination in destinations {
                    self.write_destination(destination, &sources, controls);
                }
            }
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
        self.complete &= summary.complete;
        self.call_caches.push(Some(EvaluationCache::default()));
        for actual in call.inputs.values() {
            self.eval_expr(actual);
        }
        let branch_map = self.instantiate_summary_branches(summary);

        for (destination, sources) in &summary.writes {
            let mut sources = self.map_summary_sources(call, summary, sources, &branch_map);
            sources.whole.extend_from_slice(controls);
            let version = self.ssa.positional_definition_guarded(
                sources.positional,
                sources.whole,
                &self.path_condition,
            );
            self.bind_key(*destination, version);
            self.written.insert(*destination);
        }

        for (path, destinations) in &call.outputs {
            let Some(&formal) = summary.arg_map.get(path) else {
                continue;
            };
            let widths: Vec<_> = destinations
                .iter()
                .map(|destination| self.destination_width(destination))
                .collect();
            if widths.iter().all(Option::is_some) {
                let formal_versions = self.current_key_versions_for_id(formal);
                let total_width = widths.iter().flatten().sum();
                let mut offset = total_width;
                for (destination, width) in destinations.iter().zip(widths) {
                    let width = width.expect("checked above");
                    offset -= width;
                    self.write_formal_output(
                        destination,
                        &formal_versions,
                        offset,
                        total_width,
                        controls,
                    );
                }
            } else {
                let sources = self
                    .current_key_versions_for_id(formal)
                    .into_iter()
                    .map(|(_, version)| version)
                    .collect::<Vec<_>>();
                for destination in destinations {
                    self.write_destination(destination, &sources, controls);
                }
            }
        }

        let region_groups = summary
            .result
            .iter()
            .map(|(array, regions)| {
                let regions = regions
                    .iter()
                    .map(|(span, sources)| {
                        let sources = self.map_summary_sources(call, summary, sources, &branch_map);
                        (
                            *span,
                            self.ssa.positional_definition_guarded(
                                sources.positional,
                                sources.whole,
                                &self.path_condition,
                            ),
                        )
                    })
                    .collect();
                (*array, regions)
            })
            .collect();
        let opaque_sources = summary
            .opaque_sources
            .iter()
            .flat_map(|source| {
                let sources = self.map_summary_node_source(call, summary, *source);
                sources
                    .positional
                    .into_iter()
                    .map(|(version, _)| version)
                    .chain(sources.whole)
            })
            .collect();
        self.call_caches.pop();
        CallResult {
            region_groups,
            opaque_sources,
        }
    }

    fn map_summary_sources(
        &mut self,
        call: &FunctionCall,
        summary: &FunctionSummary,
        sources: &[SummarySource],
        branch_map: &HashMap<BranchId, BranchId>,
    ) -> ExpressionSources {
        let mut result = ExpressionSources::default();
        for source in sources {
            let mut mapped = self.map_summary_node_source(call, summary, source.key);
            if !mapped.positional.is_empty() || !mapped.whole.is_empty() {
                mapped.translate(source.relation);
                let condition = source.condition.remapped(branch_map);
                let version = self.ssa.positional_definition_guarded(
                    mapped.positional,
                    mapped.whole,
                    &condition,
                );
                result
                    .positional
                    .push((version, PositionRelation::default()));
            }
        }
        result.normalize();
        result
    }

    fn map_summary_node_source(
        &mut self,
        call: &FunctionCall,
        summary: &FunctionSummary,
        source: NodeKey,
    ) -> ExpressionSources {
        if self.is_module_scope_key(source) {
            return ExpressionSources {
                positional: vec![(self.read_key(source), PositionRelation::default())],
                whole: Vec::new(),
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
            .result
            .iter()
            .flat_map(|(_, regions)| regions.iter())
            .flat_map(|(_, sources)| sources)
            .chain(summary.writes.iter().flat_map(|(_, sources)| sources))
            .flat_map(|source| source.condition.branches())
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
                projected.push(self.ssa.positional_definition_guarded(
                    vec![(source, relation)],
                    Vec::new(),
                    &condition,
                ));
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

    fn write_formal_output(
        &mut self,
        destination: &AssignDestination,
        formal_versions: &[(NodeKey, VersionId)],
        formal_offset: usize,
        formal_width: usize,
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
        let destination_array = dst_writes(destination, &mut self.ctx)
            .into_iter()
            .map(|(array, _)| array)
            .next();
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
            let mut whole = controls.to_vec();
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
                        .and_then(|span| PackedSpan::whole(formal_width)?.intersection(span)),
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
                    }
                }
            } else {
                whole.extend(formal_versions.iter().map(|(_, version)| *version));
            }
            let version =
                self.ssa
                    .positional_definition_guarded(positional, whole, &self.path_condition);
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

fn collect_assign_destinations<'a>(
    statements: &'a [Statement],
    destinations: &mut Vec<&'a AssignDestination>,
) {
    for statement in statements {
        match statement {
            Statement::Assign(assign) => destinations.extend(assign.dst.iter()),
            Statement::If(statement) => {
                collect_assign_destinations(&statement.true_side, destinations);
                collect_assign_destinations(&statement.false_side, destinations);
            }
            Statement::Case(statement) => {
                for arm in &statement.arms {
                    collect_assign_destinations(&arm.body, destinations);
                }
                collect_assign_destinations(&statement.default, destinations);
            }
            Statement::For(statement) => {
                collect_assign_destinations(&statement.body, destinations);
            }
            Statement::FunctionCall(call) => {
                destinations.extend(call.outputs.values().flatten());
            }
            Statement::IfReset(_)
            | Statement::SystemFunctionCall(_)
            | Statement::TbMethodCall(_)
            | Statement::Break
            | Statement::Unsupported(_)
            | Statement::Null => {}
        }
    }
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
