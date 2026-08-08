use super::{BitPartition, NodeKey, PackedSpan, dst_writes, var_reads};
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, CaseStatement, Expression, Factor, ForBound, ForRange,
    ForStatement, FunctionCall, IfStatement, Module, Op, Statement, SystemFunctionKind, VarIndex,
    VarSelect,
};
use crate::value::Value;
use crate::{HashMap, HashSet};

pub(super) fn analyze(
    module: &Module,
    bit_part: &BitPartition,
    statements: &[Statement],
) -> Vec<(NodeKey, NodeKey)> {
    SsaProcedure::analyze(module, bit_part, statements)
}

// Minimal statement-ordered SSA used by the loop detector.

type VersionId = usize;

#[derive(Clone)]
enum SsaVersion {
    Entry(NodeKey),
    Definition(Vec<VersionId>),
    Phi(Vec<VersionId>),
}

struct SsaProcedure<'a> {
    bit_part: &'a BitPartition,
    ctx: Context,
    versions: Vec<SsaVersion>,
    entries: HashMap<NodeKey, VersionId>,
    state: HashMap<NodeKey, VersionId>,
    written: HashSet<NodeKey>,
}

impl<'a> SsaProcedure<'a> {
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
            versions: Vec::new(),
            entries: HashMap::default(),
            state: HashMap::default(),
            written: HashSet::default(),
        };
        this.eval_block(statements, &[]);

        let mut dependencies = Vec::new();
        let destinations: Vec<_> = this.written.iter().copied().collect();
        for destination in destinations {
            let version = this.current_version(destination);
            let mut sources = HashSet::default();
            let mut visited = HashSet::default();
            this.collect_root_sources(version, &mut sources, &mut visited);
            dependencies.extend(sources.into_iter().map(|source| (source, destination)));
        }
        dependencies
    }

    fn entry_version(&mut self, key: NodeKey) -> VersionId {
        if let Some(version) = self.entries.get(&key) {
            return *version;
        }
        let version = self.versions.len();
        self.versions.push(SsaVersion::Entry(key));
        self.entries.insert(key, version);
        version
    }

    fn current_version(&mut self, key: NodeKey) -> VersionId {
        if let Some(version) = self.state.get(&key) {
            *version
        } else {
            let version = self.entry_version(key);
            self.state.insert(key, version);
            version
        }
    }

    fn definition(&mut self, mut sources: Vec<VersionId>) -> VersionId {
        sources.sort_unstable();
        sources.dedup();
        let version = self.versions.len();
        self.versions.push(SsaVersion::Definition(sources));
        version
    }

    fn phi(&mut self, mut inputs: Vec<VersionId>) -> VersionId {
        inputs.sort_unstable();
        inputs.dedup();
        if inputs.len() == 1 {
            return inputs[0];
        }
        let version = self.versions.len();
        self.versions.push(SsaVersion::Phi(inputs));
        version
    }

    fn collect_root_sources(
        &self,
        version: VersionId,
        sources: &mut HashSet<NodeKey>,
        visited: &mut HashSet<(VersionId, bool)>,
    ) {
        match &self.versions[version] {
            // A final LiveOnEntry value is retained state, not a combinational
            // read. Entry versions reached through an explicit definition are.
            SsaVersion::Entry(_) => {}
            SsaVersion::Definition(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, true, sources, visited);
                }
            }
            SsaVersion::Phi(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, false, sources, visited);
                }
            }
        }
    }

    fn collect_sources(
        &self,
        version: VersionId,
        include_entry: bool,
        sources: &mut HashSet<NodeKey>,
        visited: &mut HashSet<(VersionId, bool)>,
    ) {
        if !visited.insert((version, include_entry)) {
            return;
        }
        match &self.versions[version] {
            SsaVersion::Entry(key) => {
                if include_entry {
                    sources.insert(*key);
                }
            }
            SsaVersion::Definition(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, true, sources, visited);
                }
            }
            SsaVersion::Phi(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, include_entry, sources, visited);
                }
            }
        }
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
            .map(|key| self.current_version(key))
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
        for key in self.write_keys(destination) {
            let version = self.definition(dependencies.clone());
            self.state.insert(key, version);
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
            let version = self.definition(dependencies);
            self.state.insert(key, version);
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
        let saved_state = self.state.clone();
        let saved_written = self.written.clone();

        self.eval_block(&statement.true_side, &nested_controls);
        let true_state = self.state.clone();
        let true_written = self.written.clone();

        self.state = saved_state.clone();
        self.written = saved_written;
        self.eval_block(&statement.false_side, &nested_controls);
        let false_state = self.state.clone();
        let false_written = self.written.clone();

        self.merge_states(&saved_state, &[true_state, false_state]);
        self.written = true_written;
        self.written.extend(false_written);
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
        let saved_state = self.state.clone();
        let saved_written = self.written.clone();
        let mut states = Vec::with_capacity(statement.arms.len() + 1);
        let mut written = saved_written.clone();
        for arm in &statement.arms {
            self.state = saved_state.clone();
            self.written = saved_written.clone();
            self.eval_block(&arm.body, &nested_controls);
            states.push(self.state.clone());
            written.extend(self.written.iter().copied());
        }
        self.state = saved_state.clone();
        self.written = saved_written;
        self.eval_block(&statement.default, &nested_controls);
        states.push(self.state.clone());
        written.extend(self.written.iter().copied());
        self.merge_states(&saved_state, &states);
        self.written = written;
    }

    fn merge_states(
        &mut self,
        base: &HashMap<NodeKey, VersionId>,
        states: &[HashMap<NodeKey, VersionId>],
    ) {
        let mut keys: HashSet<NodeKey> = base.keys().copied().collect();
        for state in states {
            keys.extend(state.keys().copied());
        }
        let mut merged = HashMap::default();
        for key in keys {
            let fallback = base
                .get(&key)
                .copied()
                .unwrap_or_else(|| self.entry_version(key));
            let inputs = states
                .iter()
                .map(|state| state.get(&key).copied().unwrap_or(fallback))
                .collect();
            merged.insert(key, self.phi(inputs));
        }
        self.state = merged;
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
        let saved_state = self.state.clone();
        let saved_written = self.written.clone();
        self.eval_block(&statement.body, &range_controls);
        let body_state = self.state.clone();
        let body_written = self.written.clone();
        self.merge_states(&saved_state, &[saved_state.clone(), body_state]);
        self.written = saved_written;
        self.written.extend(body_written);
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
                                        reads.push(self.current_version(key));
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
                Factor::FunctionCall(call) => self.eval_call(call, &[]),
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
            return sources;
        };
        let mut actual_sources = Vec::new();

        for (path, actual) in &call.inputs {
            actual_sources.extend(self.eval_expr(actual));
            let Some(&formal) = body.arg_map.get(path) else {
                continue;
            };
            for key in self.keys_for_id(formal) {
                let sources = self.eval_actual_for_formal_key(actual, key);
                let version = self.definition(sources);
                self.state.insert(key, version);
            }
        }

        self.eval_block(&body.statements, controls);

        for (path, destinations) in &call.outputs {
            let Some(&formal) = body.arg_map.get(path) else {
                continue;
            };
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
                    self.write_formal_output(destination, formal, offset, total_width, controls);
                }
            } else {
                let sources = self.current_versions_for_id(formal);
                for destination in destinations {
                    self.write_destination(destination, &sources, controls);
                }
            }
        }

        let mut result = body
            .ret
            .map(|ret| self.current_versions_for_id(ret))
            .unwrap_or_default();
        if statements_have_unknown(&body.statements) {
            result.extend(actual_sources);
        }
        result.sort_unstable();
        result.dedup();
        result
    }

    fn keys_for_id(&self, id: VarId) -> Vec<NodeKey> {
        let mut keys = self
            .bit_part
            .ranges
            .iter()
            .filter(|((object, _), _)| *object == id)
            .flat_map(|((_, index), ranges)| {
                (0..ranges.len()).map(move |range| (id, *index, range))
            })
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    fn current_versions_for_id(&mut self, id: VarId) -> Vec<VersionId> {
        self.keys_for_id(id)
            .into_iter()
            .map(|key| self.current_version(key))
            .collect()
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
                .map(|range| self.current_version((*id, formal_key.1, range)))
                .collect();
        }
        self.eval_expr_bits(actual, span)
    }

    fn write_formal_output(
        &mut self,
        destination: &AssignDestination,
        formal: VarId,
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
                    for formal_key in self.keys_for_id(formal) {
                        if formal_key.1.start != 0 {
                            continue;
                        }
                        let Some(formal_span) = self.key_span(formal_key) else {
                            continue;
                        };
                        if formal_span.overlaps(requested) {
                            sources.push(self.current_version(formal_key));
                        }
                    }
                }
            } else {
                sources.extend(self.current_versions_for_id(formal));
            }
            let version = self.definition(sources);
            self.state.insert(key, version);
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
