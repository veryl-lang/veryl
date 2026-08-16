//! Analyzer-IR procedure evaluation for combinational dependency extraction.

use super::region::{BitPartition, NodeKey, PackedSpan, dst_writes, var_reads};
use super::ssa::{BranchState, SsaStore, VersionId};
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, CaseStatement, Expression, Factor, ForBound, ForRange,
    ForStatement, FunctionCall, IfStatement, Module, Op, Statement, SystemFunctionKind, VarIndex,
    VarSelect,
};
use crate::value::Value;
use crate::{HashMap, HashSet};

#[cfg(test)]
use std::cell::Cell;

#[derive(Clone)]
struct CallResult {
    region_groups: Vec<Vec<(PackedSpan, VersionId)>>,
    opaque_sources: Vec<VersionId>,
}

// Region-split writes query one RHS several times, but a function call in that
// RHS is one procedural evaluation. `None` is an invocation barrier: temporary
// call nodes in a cloned callee body must never enter the caller's cache.
type CallCache = Option<HashMap<*const FunctionCall, CallResult>>;

// Module and interface storage is shared by every call. Function-owned
// storage is automatic, so its SSA identity also includes the invocation.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SsaKey {
    node: NodeKey,
    call_frame: Option<usize>,
}

#[cfg(test)]
thread_local! {
    static FUNCTION_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_RESULT_VERSIONS: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_RESULT_REGION_PROBES: Cell<usize> = const { Cell::new(0) };
    static FUNCTION_BARRIER_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
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

pub(super) fn analyze(
    module: &Module,
    bit_part: &BitPartition,
    statements: &[Statement],
) -> Vec<(NodeKey, NodeKey)> {
    ProcedureAnalysis::analyze(module, bit_part, statements)
}

struct ProcedureAnalysis<'a> {
    bit_part: &'a BitPartition,
    ctx: Context,
    ssa: SsaStore<SsaKey>,
    written: HashSet<NodeKey>,
    call_caches: Vec<CallCache>,
    call_frames: Vec<usize>,
    next_call_frame: usize,
}

impl<'a> ProcedureAnalysis<'a> {
    fn analyze(
        module: &'a Module,
        bit_part: &'a BitPartition,
        statements: &[Statement],
    ) -> Vec<(NodeKey, NodeKey)> {
        let mut ctx = Context::default();
        ctx.variables = module.variables.clone();
        ctx.functions = module.functions.clone();
        let mut this = Self {
            bit_part,
            ctx,
            ssa: SsaStore::default(),
            written: HashSet::default(),
            call_caches: Vec::new(),
            call_frames: Vec::new(),
            next_call_frame: 0,
        };
        this.eval_block(statements, &[]);

        let mut dependencies = Vec::new();
        let destinations: Vec<_> = this
            .written
            .iter()
            .copied()
            .filter(|key| this.is_module_scope_key(*key))
            .collect();
        for destination in destinations {
            let version = this.read_key(destination);
            let sources = this.ssa.root_sources(version);
            dependencies.extend(
                sources
                    .into_iter()
                    .filter_map(|source| source.call_frame.is_none().then_some(source.node))
                    .filter(|source| this.is_module_scope_key(*source))
                    .map(|source| (source, destination)),
            );
        }
        dependencies
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

    fn read_keys(&mut self, id: VarId, index: &VarIndex, select: &VarSelect) -> Vec<NodeKey> {
        let mut keys = Vec::new();
        for (idx, span) in var_reads(id, index, select, &mut self.ctx) {
            keys.extend(self.bit_part.overlapping_access(id, idx, span));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn write_keys(&mut self, destination: &AssignDestination) -> Vec<NodeKey> {
        let mut keys = Vec::new();
        for (idx, span) in dst_writes(destination, &mut self.ctx) {
            keys.extend(self.bit_part.overlapping_access(destination.id, idx, span));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
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
        let version = self.ssa.definition(dependencies);
        for key in keys {
            self.bind_key(key, version);
            self.written.insert(key);
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
        let keys = self.write_keys(destination);
        for key in keys {
            let mut dependencies = controls.to_vec();
            for selector in destination
                .index
                .0
                .iter()
                .chain(destination.select.0.iter())
            {
                dependencies.extend(self.eval_expr(selector));
            }
            if let Some((_, selector)) = &destination.select.1 {
                dependencies.extend(self.eval_expr(selector));
            }
            if let (Some((_, low)), Some(key_span)) = (selected, self.key_span(key)) {
                if let Some(requested) = key_span.translated(low, expression_offset) {
                    dependencies.extend(self.eval_expr_requested(
                        expression,
                        requested,
                        expression_context_width,
                    ));
                }
            } else {
                dependencies.extend(self.eval_expr(expression));
            }
            let version = self.ssa.definition(dependencies);
            self.bind_key(key, version);
            self.written.insert(key);
        }
    }

    /// Returns true when control leaves the current block through `break`.
    fn eval_block(&mut self, statements: &[Statement], controls: &[VersionId]) -> bool {
        for statement in statements {
            if self.eval_statement(statement, controls) {
                return true;
            }
        }
        false
    }

    fn eval_statement(&mut self, statement: &Statement, controls: &[VersionId]) -> bool {
        match statement {
            Statement::Assign(assign) => {
                self.call_caches.push(Some(HashMap::default()));
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
                false
            }
            Statement::If(statement) => {
                self.eval_if(statement, controls);
                false
            }
            Statement::Case(statement) => {
                self.eval_case(statement, controls);
                false
            }
            Statement::For(statement) => {
                self.eval_for(statement, controls);
                false
            }
            Statement::FunctionCall(call) => {
                self.eval_call(call, controls);
                false
            }
            Statement::SystemFunctionCall(call) => {
                self.eval_system_call(call, controls, false);
                false
            }
            Statement::Break => true,
            Statement::IfReset(_)
            | Statement::TbMethodCall(_)
            | Statement::Unsupported(_)
            | Statement::Null => false,
        }
    }

    fn eval_if(&mut self, statement: &IfStatement, controls: &[VersionId]) {
        let condition = self.eval_expr(&statement.cond);
        let mut nested_controls = controls.to_vec();
        nested_controls.extend_from_slice(&condition);
        let checkpoint = self.ssa.checkpoint();
        self.eval_block(&statement.true_side, &nested_controls);
        let true_state = self.ssa.capture_and_rollback(checkpoint);

        let checkpoint = self.ssa.checkpoint();
        self.eval_block(&statement.false_side, &nested_controls);
        let false_state = self.ssa.capture_and_rollback(checkpoint);

        self.ssa.merge(&[true_state, false_state]);
    }

    fn eval_case(&mut self, statement: &CaseStatement, controls: &[VersionId]) {
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
        let mut states = Vec::with_capacity(statement.arms.len() + 1);
        for arm in &statement.arms {
            let checkpoint = self.ssa.checkpoint();
            self.eval_block(&arm.body, &nested_controls);
            states.push(self.ssa.capture_and_rollback(checkpoint));
        }
        let checkpoint = self.ssa.checkpoint();
        self.eval_block(&statement.default, &nested_controls);
        states.push(self.ssa.capture_and_rollback(checkpoint));
        self.ssa.merge(&states);
    }

    fn eval_for(&mut self, statement: &ForStatement, controls: &[VersionId]) {
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
            for value in iterations {
                if let Some(variable) = self.ctx.variables.get_mut(&statement.var_id)
                    && let Some(width) = statement.var_type.total_width()
                {
                    variable.set_value(
                        &[],
                        Value::new(value as u64, width, statement.var_type.signed),
                        None,
                    );
                }
                if self.eval_block(&statement.body, &range_controls) {
                    break;
                }
            }
            return;
        }

        // Runtime loops have a zero-trip path. One symbolic body traversal is
        // enough to expose all explicit reads; the exit phi keeps LiveOnEntry
        // separate so retained state does not become a loop edge.
        let checkpoint = self.ssa.checkpoint();
        self.eval_block(&statement.body, &range_controls);
        let body_state = self.ssa.capture_and_rollback(checkpoint);
        self.ssa.merge(&[BranchState::unchanged(), body_state]);
    }

    fn eval_expr_requested(
        &mut self,
        expression: &Expression,
        requested: PackedSpan,
        context_width: usize,
    ) -> Vec<VersionId> {
        let expression_width = expression
            .comptime()
            .r#type
            .total_width()
            .unwrap_or(context_width);
        let mut reads = PackedSpan::whole(expression_width)
            .and_then(|width| requested.intersection(width))
            .map(|span| self.eval_expr_bits(expression, span))
            .unwrap_or_default();
        if context_width > expression_width
            && expression.comptime().r#type.signed
            && requested.end() > expression_width
            && expression_width != 0
        {
            reads.extend(self.eval_expr_bits(
                expression,
                PackedSpan {
                    start: expression_width - 1,
                    length: 1,
                },
            ));
        }
        reads.sort_unstable();
        reads.dedup();
        reads
    }

    fn eval_expr_bits(&mut self, expression: &Expression, requested: PackedSpan) -> Vec<VersionId> {
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::Variable(id, index, select, _) => {
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
                        if let Some(source_span) = requested.translated(0, low) {
                            for (idx, access) in var_reads(*id, index, select, &mut self.ctx) {
                                if let Some(source_span) = source_span.intersection(access) {
                                    for key in
                                        self.bit_part.overlapping_access(*id, idx, source_span)
                                    {
                                        reads.push(self.read_key(key));
                                    }
                                }
                            }
                        }
                        reads
                    } else {
                        self.read_variable(*id, index, select)
                    }
                }
                Factor::SystemFunctionCall(call) => match &call.kind {
                    SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                        self.eval_expr_bits(&input.0, requested)
                    }
                    _ => self.eval_system_call(call, &[], true),
                },
                Factor::FunctionCall(call) => self.eval_call_requested(call, &[], Some(requested)),
                Factor::HierVariable(_)
                | Factor::Value(_)
                | Factor::Anonymous(_)
                | Factor::Unknown(_) => Vec::new(),
            },
            Expression::Unary(op, operand, _) => match op {
                Op::BitNot | Op::Add | Op::Sub => self.eval_expr_bits(operand, requested),
                _ => self.eval_expr(operand),
            },
            Expression::Binary(left, op, right, _) => match op {
                Op::LogicShiftL | Op::ArithShiftL => {
                    let shift = right
                        .eval_value(&mut self.ctx)
                        .and_then(|value| value.to_usize());
                    let mut reads = self.eval_expr(right);
                    if let Some(shift) = shift {
                        let start = requested.start.max(shift);
                        if let Some(length) = requested.end().checked_sub(start)
                            && let Some(input) = PackedSpan::new(start - shift, length)
                        {
                            reads.extend(self.eval_expr_bits(left, input));
                        }
                    } else {
                        reads.extend(self.eval_expr(left));
                    }
                    reads
                }
                Op::LogicShiftR | Op::ArithShiftR => {
                    let shift = right
                        .eval_value(&mut self.ctx)
                        .and_then(|value| value.to_usize());
                    let mut reads = self.eval_expr(right);
                    if let Some(shift) = shift {
                        let width = left.comptime().r#type.total_width().unwrap_or(0);
                        let shifted = requested.translated(0, shift);
                        if let Some(input) = shifted
                            .and_then(|shifted| PackedSpan::whole(width)?.intersection(shifted))
                        {
                            reads.extend(self.eval_expr_bits(left, input));
                        }
                        if *op == Op::ArithShiftR
                            && left.comptime().r#type.signed
                            && width != 0
                            && shifted.is_some_and(|shifted| shifted.end() > width)
                        {
                            reads.extend(self.eval_expr_bits(
                                left,
                                PackedSpan {
                                    start: width - 1,
                                    length: 1,
                                },
                            ));
                        }
                    } else {
                        reads.extend(self.eval_expr(left));
                    }
                    reads
                }
                Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
                    let mut reads = self.eval_expr_bits(left, requested);
                    reads.extend(self.eval_expr_bits(right, requested));
                    reads
                }
                _ => self.eval_expr(expression),
            },
            Expression::Ternary(condition, left, right, _) => {
                let mut reads = self.eval_expr(condition);
                reads.extend(self.eval_expr_bits(left, requested));
                reads.extend(self.eval_expr_bits(right, requested));
                reads
            }
            Expression::Concatenation(parts, _) => {
                if parts.iter().any(|(_, repeat)| repeat.is_some()) {
                    return self.eval_expr(expression);
                }
                let mut low = 0usize;
                let mut reads = Vec::new();
                for (part, _) in parts.iter().rev() {
                    let Some(width) = part.comptime().r#type.total_width() else {
                        return self.eval_expr(expression);
                    };
                    if let Some(window) = PackedSpan::new(low, width)
                        && let Some(local) = requested
                            .intersection(window)
                            .and_then(|span| span.translated(low, 0))
                    {
                        reads.extend(self.eval_expr_bits(part, local));
                    }
                    low = low.saturating_add(width);
                }
                reads
            }
            Expression::ArrayLiteral(_, _) | Expression::StructConstructor(_, _, _) => {
                self.eval_expr(expression)
            }
        }
    }

    fn eval_expr(&mut self, expression: &Expression) -> Vec<VersionId> {
        let mut reads = Vec::new();
        match expression {
            Expression::Term(factor) => self.eval_factor(factor, &mut reads),
            Expression::Unary(_, expression, _) => reads.extend(self.eval_expr(expression)),
            Expression::Binary(left, _, right, _) => {
                reads.extend(self.eval_expr(left));
                reads.extend(self.eval_expr(right));
            }
            Expression::Ternary(condition, left, right, _) => {
                reads.extend(self.eval_expr(condition));
                reads.extend(self.eval_expr(left));
                reads.extend(self.eval_expr(right));
            }
            Expression::Concatenation(parts, _) => {
                for (part, repeat) in parts {
                    reads.extend(self.eval_expr(part));
                    if let Some(repeat) = repeat {
                        reads.extend(self.eval_expr(repeat));
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(value, repeat) => {
                            reads.extend(self.eval_expr(value));
                            if let Some(repeat) = repeat {
                                reads.extend(self.eval_expr(repeat));
                            }
                        }
                        ArrayLiteralItem::Defaul(value) => reads.extend(self.eval_expr(value)),
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, value) in fields {
                    reads.extend(self.eval_expr(value));
                }
            }
        }
        reads.sort_unstable();
        reads.dedup();
        reads
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
            Factor::HierVariable(_)
            | Factor::Value(_)
            | Factor::Anonymous(_)
            | Factor::Unknown(_) => {}
        }
    }

    fn eval_call(&mut self, call: &FunctionCall, controls: &[VersionId]) -> Vec<VersionId> {
        self.eval_call_requested(call, controls, None)
    }

    fn eval_call_requested(
        &mut self,
        call: &FunctionCall,
        controls: &[VersionId],
        requested: Option<PackedSpan>,
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
            .and_then(|cache| cache.get(&cache_key))
        {
            let result = self.select_call_result(cached, requested);
            #[cfg(test)]
            FUNCTION_RESULT_VERSIONS.set(FUNCTION_RESULT_VERSIONS.get() + result.len());
            return result;
        }

        let evaluated = self.eval_call_uncached(call, controls);
        let result = self.select_call_result(&evaluated, requested);
        if let Some(Some(cache)) = self.call_caches.last_mut() {
            cache.insert(cache_key, evaluated);
        }
        #[cfg(test)]
        FUNCTION_RESULT_VERSIONS.set(FUNCTION_RESULT_VERSIONS.get() + result.len());
        result
    }

    fn select_call_result(
        &self,
        evaluated: &CallResult,
        requested: Option<PackedSpan>,
    ) -> Vec<VersionId> {
        let mut result = Vec::new();
        for regions in &evaluated.region_groups {
            let first = requested.map_or(0, |requested| {
                regions.partition_point(|(span, _)| {
                    #[cfg(test)]
                    FUNCTION_RESULT_REGION_PROBES.set(FUNCTION_RESULT_REGION_PROBES.get() + 1);
                    span.end() <= requested.start
                })
            });
            for (span, version) in &regions[first..] {
                #[cfg(test)]
                FUNCTION_RESULT_REGION_PROBES.set(FUNCTION_RESULT_REGION_PROBES.get() + 1);
                if requested.is_some_and(|requested| span.start >= requested.end()) {
                    break;
                }
                result.push(*version);
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
                let sources = self.eval_actual_for_formal_key(actual, key);
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
            let version = self.ssa.definition(sources);
            self.bind_key(key, version);
        }

        self.call_caches.push(None);
        self.eval_block(&body.statements, controls);
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

    fn current_region_groups_for_id(&mut self, id: VarId) -> Vec<Vec<(PackedSpan, VersionId)>> {
        let mut groups = Vec::<Vec<(PackedSpan, VersionId)>>::new();
        let mut previous_array_span = None;
        for key in self.keys_for_id(id) {
            let Some(span) = self.key_span(key) else {
                continue;
            };
            if previous_array_span != Some(key.1) {
                groups.push(Vec::new());
                previous_array_span = Some(key.1);
            }
            let group = groups.last_mut().expect("pushed above");
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
    ) -> Vec<VersionId> {
        let Some(span) = self.key_span(formal_key) else {
            return self.eval_expr(actual);
        };
        if let Expression::Term(factor) = actual
            && let Factor::Variable(id, index, select, _) = factor.as_ref()
            && index.0.is_empty()
            && select.is_empty()
        {
            return self
                .bit_part
                .overlapping((*id, formal_key.1), span)
                .into_iter()
                .map(|range| self.read_key((*id, formal_key.1, range)))
                .collect();
        }
        self.eval_expr_bits(actual, span)
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
        for key in self.write_keys(destination) {
            let mut sources = controls.to_vec();
            if let (Some((_, low)), Some(span)) = (selected, self.key_span(key)) {
                if let Some(requested) = span
                    .translated(low, formal_offset)
                    .and_then(|span| PackedSpan::whole(formal_width)?.intersection(span))
                {
                    for (formal_key, version) in formal_versions {
                        if formal_key.1.start != 0 {
                            continue;
                        }
                        let Some(formal_span) = self.key_span(*formal_key) else {
                            continue;
                        };
                        if formal_span.overlaps(requested) {
                            sources.push(*version);
                        }
                    }
                }
            } else {
                sources.extend(formal_versions.iter().map(|(_, version)| *version));
            }
            let version = self.ssa.definition(sources);
            self.bind_key(key, version);
            self.written.insert(key);
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
            SystemFunctionKind::Display(_) | SystemFunctionKind::Write(_) => Vec::new(),
            SystemFunctionKind::Assert { .. } => Vec::new(),
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
