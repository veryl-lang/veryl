//! Veryl IR adapter for the IR-independent region MemorySSA engine.

use std::collections::{BTreeMap, BTreeSet};

use crate::conv::Context;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, CasePattern, CombDeclaration, Expression, Factor,
    ForBound, ForRange, FunctionCall, Module, Op, Statement, SystemFunctionCall,
    SystemFunctionKind, TypeKind, VarId, VarIndex, VarSelect,
};
use veryl_causal::graph::{EdgeKind, IncompleteReason};
use veryl_causal::procedure::{
    self, AlignedDependency, Event, MustAliasCandidate, Procedure, ProcedureError,
    ProcedureSummary, WriteId,
};
use veryl_causal::region::{Region, Span};
use veryl_parser::token_range::TokenRange;

pub(crate) struct ProcedureAnalysis {
    pub summary: ProcedureSummary<VarId>,
    pub write_tokens: BTreeMap<WriteId, TokenRange>,
}

impl std::ops::Deref for ProcedureAnalysis {
    type Target = ProcedureSummary<VarId>;

    fn deref(&self) -> &Self::Target {
        &self.summary
    }
}

pub(crate) fn analyze(
    module: &Module,
    declaration: &CombDeclaration,
) -> Result<ProcedureAnalysis, ProcedureError> {
    let mut builder = Builder::new(module);
    let exit = builder
        .lower_statements(&declaration.statements, 0, &[], None)
        .unwrap_or(0);
    builder.procedure.exit = exit;
    if let Some(message) = builder.model_error {
        return Err(ProcedureError::Model(message));
    }
    let summary = procedure::analyze(&builder.procedure)?;
    Ok(ProcedureAnalysis {
        summary,
        write_tokens: builder.write_tokens,
    })
}

pub(crate) fn analyze_observer_expression(
    module: &Module,
    expression: &Expression,
) -> Result<ProcedureAnalysis, ProcedureError> {
    let mut builder = Builder::new(module);
    builder.lower_observer_expression(0, expression);
    builder.procedure.exit = 0;
    if let Some(message) = builder.model_error {
        return Err(ProcedureError::Model(message));
    }
    let summary = procedure::analyze(&builder.procedure)?;
    Ok(ProcedureAnalysis {
        summary,
        write_tokens: builder.write_tokens,
    })
}

/// Map a result-bit span to the module regions which structurally supply it.
/// `None` requests the caller's conservative, all-operands fallback.
pub(crate) fn map_expression_span(
    context: &Context,
    expression: &Expression,
    requested: Span,
) -> Option<Vec<Region<VarId>>> {
    let mut nested_context = Context::default();
    nested_context.variables = context.variables.clone();
    nested_context.functions = context.functions.clone();
    Builder::with_context(nested_context).map_expression_span_to_actual(expression, requested)
}

#[derive(Clone, Copy)]
pub(crate) struct PositionedRegion {
    pub region: Region<VarId>,
    /// Bit coordinates in the mapped expression which this region supplies.
    pub expression: Span,
    /// Whether source and result bits correspond one-to-one.
    pub aligned: bool,
    /// Value for data (including replicated fill); Control for selectors.
    pub kind: EdgeKind,
    /// Regular copies of this source-to-expression mapping.
    pub repetitions: usize,
    pub expression_stride: usize,
}

pub(crate) fn map_expression_span_positioned(
    context: &Context,
    expression: &Expression,
    requested: Span,
) -> Option<Vec<PositionedRegion>> {
    let mut nested_context = Context::default();
    nested_context.variables = context.variables.clone();
    nested_context.functions = context.functions.clone();
    Builder::with_context(nested_context)
        .map_expression_span_positioned_to_actual(expression, requested)
}

/// Map an expression after applying the assignment context supplied by a
/// formal port or argument. Array literals are context-determined, so their
/// own IR node does not necessarily retain the aggregate shape needed to map
/// a child summary back into the actual expression.
pub(crate) fn map_expression_span_positioned_as(
    context: &Context,
    expression: &Expression,
    expected: &crate::ir::Type,
    requested: Span,
) -> Option<Vec<PositionedRegion>> {
    let mut expression = expression.clone();
    if matches!(expression, Expression::ArrayLiteral(_, _)) {
        expression.comptime_mut().r#type = expected.clone();
    }
    map_expression_span_positioned(context, &expression, requested)
}

fn exact_regions_have_equal_length(left: Region<VarId>, right: Region<VarId>) -> bool {
    matches!(
        (left, right),
        (
            Region::Exact { span: left, .. },
            Region::Exact { span: right, .. }
        ) if left.length == right.length
    )
}

struct Builder {
    context: Context,
    procedure: Procedure<VarId>,
    next_read: usize,
    next_write: usize,
    unresolved_writes: BTreeMap<String, Vec<(usize, Vec<usize>)>>,
    call_stack: Vec<VarId>,
    write_tokens: BTreeMap<WriteId, TokenRange>,
    model_error: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct MappedFunctionRead {
    read: usize,
    kind: EdgeKind,
    input: Region<VarId>,
    aligned: bool,
}

impl Builder {
    fn new(module: &Module) -> Self {
        let mut context = Context::default();
        context.variables = module.variables.clone();
        context.functions = module.functions.clone();
        Self::with_context(context)
    }

    fn with_context(context: Context) -> Self {
        let object_spans = context
            .variables
            .iter()
            .filter_map(|(&id, variable)| {
                variable
                    .total_width()?
                    .checked_mul(variable.r#type.total_array().unwrap_or(1))
                    .map(|length| (id, Span { start: 0, length }))
            })
            .collect();
        Self {
            context,
            procedure: Procedure {
                entry: 0,
                exit: 0,
                successors: vec![Vec::new()],
                events: vec![Vec::new()],
                object_spans,
                must_alias: BTreeSet::new(),
                incomplete: BTreeSet::new(),
            },
            next_read: 0,
            next_write: 0,
            unresolved_writes: BTreeMap::new(),
            call_stack: Vec::new(),
            write_tokens: BTreeMap::new(),
            model_error: None,
        }
    }

    fn nested(&self, callee: VarId) -> Self {
        let mut context = Context::default();
        context.variables = self.context.variables.clone();
        context.functions = self.context.functions.clone();
        let mut call_stack = self.call_stack.clone();
        call_stack.push(callee);
        Self {
            context,
            procedure: Procedure {
                entry: 0,
                exit: 0,
                successors: vec![Vec::new()],
                events: vec![Vec::new()],
                object_spans: self.procedure.object_spans.clone(),
                must_alias: BTreeSet::new(),
                incomplete: BTreeSet::new(),
            },
            next_read: 0,
            next_write: 0,
            unresolved_writes: BTreeMap::new(),
            call_stack,
            write_tokens: BTreeMap::new(),
            model_error: None,
        }
    }

    fn new_block(&mut self) -> usize {
        let block = self.procedure.events.len();
        self.procedure.events.push(Vec::new());
        self.procedure.successors.push(Vec::new());
        block
    }

    fn edge(&mut self, from: usize, to: usize) {
        self.procedure.successors[from].push(to);
    }

    fn lower_statements(
        &mut self,
        statements: &[Statement],
        mut block: usize,
        controls: &[(usize, EdgeKind)],
        break_target: Option<usize>,
    ) -> Option<usize> {
        let mut index = 0usize;
        while index < statements.len() {
            if let Some(consumed) =
                self.lower_periodic_variable_assignments(&statements[index..], block, controls)
            {
                index += consumed;
                continue;
            }
            block = self.lower_statement(&statements[index], block, controls, break_target)?;
            index += 1;
        }
        Some(block)
    }

    /// Collapse an elaborated run such as `b[i] = a` into one periodic
    /// transfer. The front-end expands array assignment patterns for codegen;
    /// rebuilding their regular structure here prevents causal analysis from
    /// inheriting a declaration-width-sized event list.
    fn lower_periodic_variable_assignments(
        &mut self,
        statements: &[Statement],
        block: usize,
        controls: &[(usize, EdgeKind)],
    ) -> Option<usize> {
        let Statement::Assign(first) = statements.first()? else {
            return None;
        };
        if first.dst.len() != 1 {
            return None;
        }
        let Expression::Term(first_factor) = &first.expr else {
            return None;
        };
        let Factor::Variable(source_id, source_index, source_select, _) = first_factor.as_ref()
        else {
            return None;
        };
        let source = self.variable_region(*source_id, source_index, source_select);
        let Region::Exact {
            span: source_span, ..
        } = source
        else {
            return None;
        };

        let mut destinations = Vec::new();
        for statement in statements {
            let Statement::Assign(assign) = statement else {
                break;
            };
            if assign.dst.len() != 1 {
                break;
            }
            let Expression::Term(factor) = &assign.expr else {
                break;
            };
            let Factor::Variable(id, index, select, _) = factor.as_ref() else {
                break;
            };
            if self.variable_region(*id, index, select) != source {
                break;
            }
            let Region::Exact { object, span } = self.variable_region(
                assign.dst[0].id,
                &assign.dst[0].index,
                &assign.dst[0].select,
            ) else {
                break;
            };
            if span.length != source_span.length {
                break;
            }
            destinations.push((object, span));
        }
        if destinations.len() < 2 {
            return None;
        }
        let object = destinations[0].0;
        if destinations
            .iter()
            .any(|(candidate, _)| *candidate != object)
        {
            return None;
        }
        destinations.sort_unstable_by_key(|(_, span)| span.start);
        if destinations
            .windows(2)
            .any(|pair| pair[0].1.end().is_none_or(|end| end != pair[1].1.start))
        {
            return None;
        }
        let output = Span {
            start: destinations[0].1.start,
            length: source_span.length.checked_mul(destinations.len())?,
        };
        let read = self.push_read(block, source);
        let id = self.next_write;
        self.next_write += 1;
        self.write_tokens.insert(id, first.dst[0].token);
        self.procedure.events[block].push(Event::Write {
            id,
            region: Region::Exact {
                object,
                span: output,
            },
            dependencies: controls.to_vec(),
            aligned_dependencies: vec![AlignedDependency {
                read,
                kind: EdgeKind::Value,
                source: Span {
                    start: 0,
                    length: source_span.length,
                },
                destination: Span {
                    start: 0,
                    length: source_span.length,
                },
                repetitions: destinations.len(),
                destination_stride: source_span.length,
            }],
        });
        Some(destinations.len())
    }

    fn lower_statement(
        &mut self,
        statement: &Statement,
        block: usize,
        controls: &[(usize, EdgeKind)],
        break_target: Option<usize>,
    ) -> Option<usize> {
        match statement {
            Statement::Assign(assign) => {
                let (mut dependencies, aligned_by_destination) =
                    self.assignment_dependencies(block, &assign.expr, &assign.dst);
                dependencies.extend_from_slice(controls);
                for (destination, aligned_dependencies) in
                    assign.dst.iter().zip(aligned_by_destination)
                {
                    self.write_destination(
                        block,
                        destination,
                        dependencies.clone(),
                        aligned_dependencies,
                    );
                }
                Some(block)
            }
            Statement::If(statement) => {
                let condition = self.read_expression(block, &statement.cond, EdgeKind::Control);
                let mut nested_controls = controls.to_vec();
                nested_controls.extend(condition);
                let true_block = self.new_block();
                let false_block = self.new_block();
                self.edge(block, true_block);
                self.edge(block, false_block);
                let true_exit = self.lower_statements(
                    &statement.true_side,
                    true_block,
                    &nested_controls,
                    break_target,
                );
                let false_exit = self.lower_statements(
                    &statement.false_side,
                    false_block,
                    &nested_controls,
                    break_target,
                );
                let exits = true_exit.into_iter().chain(false_exit).collect::<Vec<_>>();
                if exits.is_empty() {
                    None
                } else {
                    let join = self.new_block();
                    for exit in exits {
                        self.edge(exit, join);
                    }
                    Some(join)
                }
            }
            Statement::Case(statement) => {
                let mut condition =
                    self.read_expression(block, &statement.case_target, EdgeKind::Control);
                for arm in &statement.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            CasePattern::Eq(expression) => condition.extend(self.read_expression(
                                block,
                                expression,
                                EdgeKind::Control,
                            )),
                            CasePattern::Range { lo, hi, .. } => {
                                condition.extend(self.read_expression(
                                    block,
                                    lo,
                                    EdgeKind::Control,
                                ));
                                condition.extend(self.read_expression(
                                    block,
                                    hi,
                                    EdgeKind::Control,
                                ));
                            }
                        }
                    }
                }
                let mut nested_controls = controls.to_vec();
                nested_controls.extend(condition);
                let mut exits = Vec::new();
                for arm in &statement.arms {
                    let arm_block = self.new_block();
                    self.edge(block, arm_block);
                    exits.extend(self.lower_statements(
                        &arm.body,
                        arm_block,
                        &nested_controls,
                        break_target,
                    ));
                }
                let default_block = self.new_block();
                self.edge(block, default_block);
                exits.extend(self.lower_statements(
                    &statement.default,
                    default_block,
                    &nested_controls,
                    break_target,
                ));
                if exits.is_empty() {
                    None
                } else {
                    let join = self.new_block();
                    for exit in exits {
                        self.edge(exit, join);
                    }
                    Some(join)
                }
            }
            Statement::For(statement) => {
                self.procedure
                    .incomplete
                    .insert(IncompleteReason::RuntimeLoop);
                let header = self.new_block();
                let body = self.new_block();
                let exit = self.new_block();
                self.edge(block, header);
                let mut condition = Vec::new();
                let bounds = match &statement.range {
                    ForRange::Forward { start, end, .. }
                    | ForRange::Reverse { start, end, .. }
                    | ForRange::Stepped { start, end, .. } => [start, end],
                };
                for bound in bounds {
                    if let ForBound::Expression(expression) = bound {
                        condition.extend(self.read_expression(
                            header,
                            expression,
                            EdgeKind::Control,
                        ));
                    }
                }
                let mut nested_controls = controls.to_vec();
                nested_controls.extend(condition);
                self.edge(header, body);
                self.edge(header, exit);
                if let Some(body_exit) =
                    self.lower_statements(&statement.body, body, &nested_controls, Some(exit))
                {
                    self.edge(body_exit, header);
                }
                Some(exit)
            }
            Statement::FunctionCall(call) => {
                self.lower_function_call(block, call, controls);
                Some(block)
            }
            Statement::SystemFunctionCall(call) => {
                self.lower_system_function(block, call, false);
                Some(block)
            }
            Statement::TbMethodCall(call) => {
                self.procedure
                    .incomplete
                    .insert(IncompleteReason::TimedOrEventEffect);
                if let Some(destination) = &call.ret {
                    let unknown = self.unknown_read(block);
                    self.write_destination(
                        block,
                        destination,
                        vec![(unknown, EdgeKind::Unknown)],
                        Vec::new(),
                    );
                }
                Some(block)
            }
            Statement::IfReset(_) => {
                self.model_error
                    .get_or_insert("always_comb contains an always_ff-only if_reset statement");
                Some(block)
            }
            Statement::Break => {
                self.procedure
                    .incomplete
                    .insert(IncompleteReason::RuntimeLoop);
                if let Some(target) = break_target {
                    self.edge(block, target);
                } else {
                    self.model_error
                        .get_or_insert("break appears outside a lowered loop");
                }
                None
            }
            Statement::Unsupported(_) => {
                self.model_error.get_or_insert(
                    "always_comb contains a statement rejected during IR conversion",
                );
                Some(block)
            }
            Statement::Null => Some(block),
        }
    }

    fn assignment_dependencies(
        &mut self,
        block: usize,
        expression: &Expression,
        destinations: &[AssignDestination],
    ) -> (Vec<(usize, EdgeKind)>, Vec<Vec<AlignedDependency>>) {
        if let Expression::Ternary(condition, left, right, _) = expression {
            let condition_dependencies = self.read_expression(block, condition, EdgeKind::Control);
            let branches = match self.constant_condition(condition) {
                Some(true) => [Some(left.as_ref()), None],
                Some(false) => [Some(right.as_ref()), None],
                None => [Some(left.as_ref()), Some(right.as_ref())],
            };
            let mut all_dependencies = condition_dependencies.clone();
            let mut precise_dependencies = condition_dependencies.clone();
            let mut aligned = vec![Vec::new(); destinations.len()];
            let mut precise = true;
            for branch in branches.into_iter().flatten() {
                let mut branch_dependencies = self.read_expression(block, branch, EdgeKind::Value);
                all_dependencies.extend_from_slice(&branch_dependencies);
                if branch_dependencies.is_empty() {
                    continue;
                }
                let Some(branch_aligned) = self.aligned_assignment_dependencies(
                    block,
                    branch,
                    destinations,
                    &mut branch_dependencies,
                    Some(left.comptime().r#type.signed && right.comptime().r#type.signed),
                ) else {
                    precise = false;
                    continue;
                };
                precise_dependencies.extend(branch_dependencies);
                for (combined, branch) in aligned.iter_mut().zip(branch_aligned) {
                    combined.extend(branch);
                }
            }
            if precise {
                (precise_dependencies, aligned)
            } else {
                (all_dependencies, vec![Vec::new(); destinations.len()])
            }
        } else {
            let mut dependencies = self.read_expression(block, expression, EdgeKind::Value);
            let aligned = self
                .aligned_assignment_dependencies(
                    block,
                    expression,
                    destinations,
                    &mut dependencies,
                    None,
                )
                .unwrap_or_else(|| vec![Vec::new(); destinations.len()]);
            (dependencies, aligned)
        }
    }

    fn read_expression(
        &mut self,
        block: usize,
        expression: &Expression,
        kind: EdgeKind,
    ) -> Vec<(usize, EdgeKind)> {
        let mut reads = Vec::new();
        match expression {
            Expression::Term(factor) => self.read_factor(block, factor, kind, &mut reads),
            Expression::Unary(_, expression, _) => {
                reads.extend(self.read_expression(block, expression, kind));
            }
            Expression::Binary(left, op, right, _) => {
                reads.extend(self.read_expression(block, left, kind));
                if self.binary_rhs_is_reachable(left, *op) {
                    reads.extend(self.read_expression(block, right, kind));
                }
            }
            Expression::Ternary(condition, left, right, _) => {
                reads.extend(self.read_expression(block, condition, EdgeKind::Control));
                match self.constant_condition(condition) {
                    Some(true) => reads.extend(self.read_expression(block, left, kind)),
                    Some(false) => reads.extend(self.read_expression(block, right, kind)),
                    None => {
                        reads.extend(self.read_expression(block, left, kind));
                        reads.extend(self.read_expression(block, right, kind));
                    }
                }
            }
            Expression::Concatenation(parts, _) => {
                for (expression, repeat) in parts {
                    reads.extend(self.read_expression(block, expression, kind));
                    if let Some(repeat) = repeat {
                        reads.extend(self.read_expression(block, repeat, EdgeKind::Address));
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(expression, repeat) => {
                            reads.extend(self.read_expression(block, expression, kind));
                            if let Some(repeat) = repeat {
                                reads.extend(self.read_expression(
                                    block,
                                    repeat,
                                    EdgeKind::Address,
                                ));
                            }
                        }
                        ArrayLiteralItem::Defaul(expression) => {
                            reads.extend(self.read_expression(block, expression, kind));
                        }
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, expression) in fields {
                    reads.extend(self.read_expression(block, expression, kind));
                }
            }
        }
        reads
    }

    fn read_factor(
        &mut self,
        block: usize,
        factor: &Factor,
        kind: EdgeKind,
        reads: &mut Vec<(usize, EdgeKind)>,
    ) {
        match factor {
            Factor::Variable(id, index, select, _) => {
                let mut selector_reads = Vec::new();
                for expression in index.0.iter().chain(select.0.iter()) {
                    selector_reads.extend(self.read_expression(
                        block,
                        expression,
                        EdgeKind::Address,
                    ));
                }
                if let Some((_, expression)) = &select.1 {
                    selector_reads.extend(self.read_expression(
                        block,
                        expression,
                        EdgeKind::Address,
                    ));
                }
                reads.extend(selector_reads.iter().copied());
                for region in self.variable_regions(*id, index, select) {
                    reads.push((
                        self.push_variable_read(block, *id, index, select, region, &selector_reads),
                        kind,
                    ));
                }
            }
            Factor::HierVariable(_) => {
                self.procedure
                    .incomplete
                    .insert(IncompleteReason::HierarchicalReference);
                reads.push((self.unknown_read(block), EdgeKind::Unknown));
            }
            Factor::SystemFunctionCall(call) => {
                reads.extend(self.lower_system_function(block, call, true));
            }
            Factor::FunctionCall(call) => {
                reads.extend(self.lower_function_call(block, call, &[]));
            }
            Factor::Unknown(_) => {
                reads.push((self.unknown_read(block), EdgeKind::Unknown));
            }
            Factor::Value(_) | Factor::Anonymous(_) => {}
        }
    }

    fn lower_function_call(
        &mut self,
        block: usize,
        call: &FunctionCall,
        controls: &[(usize, EdgeKind)],
    ) -> Vec<(usize, EdgeKind)> {
        let mut actual_inputs = BTreeMap::<VarId, (Expression, Vec<(usize, EdgeKind)>)>::new();
        let function_body = self.context.functions.get(&call.id).and_then(|function| {
            if let Some(index) = &call.index {
                function.get_function(index)
            } else {
                function.get_function(&[])
            }
        });

        let Some(function_body) = function_body else {
            self.model_error
                .get_or_insert("resolved function call has no instantiated function body");
            return vec![(self.unknown_read(block), EdgeKind::Unknown)];
        };
        for (path, input) in &call.inputs {
            let reads = self.read_expression(block, input, EdgeKind::Value);
            if let Some(&formal) = function_body.arg_map.get(path) {
                actual_inputs.insert(formal, (input.clone(), reads));
            }
        }

        if self.call_stack.contains(&call.id) {
            self.procedure
                .incomplete
                .insert(IncompleteReason::RecursiveCall);
            return vec![(self.unknown_read(block), EdgeKind::Unknown)];
        }

        let mut nested = self.nested(call.id);
        let exit = nested
            .lower_statements(&function_body.statements, 0, &[], None)
            .unwrap_or(0);
        nested.procedure.exit = exit;
        if self.model_error.is_none() {
            self.model_error = nested.model_error;
        }
        if self.model_error.is_some() {
            return vec![(self.unknown_read(block), EdgeKind::Unknown)];
        }
        let Ok(summary) = procedure::analyze(&nested.procedure) else {
            self.model_error
                .get_or_insert("function body produced an invalid causal model");
            return vec![(self.unknown_read(block), EdgeKind::Unknown)];
        };
        if !summary.incomplete.is_empty() {
            self.procedure
                .incomplete
                .extend(summary.incomplete.iter().copied());
        }

        let formal_ids = function_body
            .arg_map
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut dependencies_by_output = BTreeMap::<Region<VarId>, Vec<MappedFunctionRead>>::new();
        for dependency in &summary.dependencies {
            let mut mapped = Vec::new();
            let input_object = match dependency.input {
                Region::Exact { object, .. }
                | Region::UnknownRegion { object, .. }
                | Region::UnknownObject(object) => Some(object),
                Region::UnknownAll => None,
            };
            if let Some(formal) =
                input_object.and_then(|object| actual_inputs.get_key_value(&object))
            {
                let (_, (actual_expression, fallback_reads)) = formal;
                if let Some(actual_regions) =
                    self.map_formal_region_to_actual(actual_expression, dependency.input)
                {
                    let preserve_alignment = dependency.aligned
                        && actual_regions.len() == 1
                        && actual_regions[0].aligned
                        && exact_regions_have_equal_length(
                            actual_regions[0].region,
                            dependency.output,
                        );
                    mapped.extend(actual_regions.into_iter().map(|actual| {
                        let actual_region = actual.region;
                        MappedFunctionRead {
                            read: self.push_read(block, actual_region),
                            kind: dependency.kind,
                            input: actual_region,
                            aligned: preserve_alignment,
                        }
                    }));
                } else {
                    mapped.extend(fallback_reads.iter().map(|&(read, actual_kind)| {
                        MappedFunctionRead {
                            read,
                            kind: combine_edge_kinds(dependency.kind, actual_kind),
                            input: Region::UnknownAll,
                            aligned: false,
                        }
                    }));
                }
            } else if input_object.is_some_and(|object| {
                self.context.variables.get(&object).is_some_and(|variable| {
                    variable.affiliation == crate::symbol::Affiliation::Module
                })
            }) {
                mapped.push(MappedFunctionRead {
                    read: self.push_read(block, dependency.input),
                    kind: dependency.kind,
                    input: dependency.input,
                    aligned: dependency.aligned
                        && exact_regions_have_equal_length(dependency.input, dependency.output),
                });
            } else if input_object.is_some_and(|object| formal_ids.contains(&object)) {
                self.model_error
                    .get_or_insert("function dependency refers to an unmapped input argument");
                continue;
            } else {
                self.procedure
                    .incomplete
                    .insert(IncompleteReason::MalformedModel);
                mapped.push(MappedFunctionRead {
                    read: self.unknown_read(block),
                    kind: EdgeKind::Unknown,
                    input: Region::UnknownAll,
                    aligned: false,
                });
            }
            mapped.sort_unstable_by_key(|dependency| (dependency.read, dependency.kind));
            mapped.dedup_by_key(|dependency| (dependency.read, dependency.kind));
            dependencies_by_output
                .entry(dependency.output)
                .or_default()
                .extend(mapped);
        }
        for dependencies in dependencies_by_output.values_mut() {
            dependencies.sort_unstable_by_key(|dependency| (dependency.read, dependency.kind));
            dependencies.dedup_by_key(|dependency| (dependency.read, dependency.kind));
        }
        for &output in &summary.outputs {
            dependencies_by_output.entry(output).or_default();
        }

        for (&output, dependencies) in &dependencies_by_output {
            let output_object = match output {
                Region::Exact { object, .. }
                | Region::UnknownRegion { object, .. }
                | Region::UnknownObject(object) => object,
                Region::UnknownAll => continue,
            };
            let is_module_capture = self
                .context
                .variables
                .get(&output_object)
                .is_some_and(|variable| variable.affiliation == crate::symbol::Affiliation::Module);
            if !is_module_capture {
                continue;
            }
            let mut dependencies = dependencies
                .iter()
                .map(|dependency| (dependency.read, dependency.kind))
                .collect::<Vec<_>>();
            dependencies.extend_from_slice(controls);
            let id = self.next_write;
            self.next_write += 1;
            self.write_tokens.insert(id, call.comptime.token);
            self.procedure.events[block].push(Event::Write {
                id,
                region: output,
                dependencies,
                aligned_dependencies: Vec::new(),
            });
        }

        for (path, outputs) in &call.outputs {
            let Some(&formal) = function_body.arg_map.get(path) else {
                self.model_error
                    .get_or_insert("function call output has no formal argument mapping");
                continue;
            };
            let mapped_dependencies = dependencies_by_output
                .iter()
                .filter_map(|(output, dependencies)| match output {
                    Region::Exact { object, .. } | Region::UnknownObject(object)
                        if *object == formal =>
                    {
                        Some((*output, dependencies.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let mut dependencies = mapped_dependencies
                .iter()
                .flat_map(|(_, dependencies)| dependencies.iter().copied())
                .map(|dependency| (dependency.read, dependency.kind))
                .collect::<Vec<_>>();
            dependencies.sort_unstable();
            dependencies.dedup();
            dependencies.extend_from_slice(controls);
            let aligned_by_destination =
                self.function_output_aligned_dependencies(formal, outputs, &mapped_dependencies);
            if aligned_by_destination.is_some() {
                dependencies = controls.to_vec();
            }
            for (index, destination) in outputs.iter().enumerate() {
                self.write_destination(
                    block,
                    destination,
                    dependencies.clone(),
                    aligned_by_destination
                        .as_ref()
                        .map(|aligned| aligned[index].clone())
                        .unwrap_or_default(),
                );
            }
        }

        if let Some(ret) = function_body.ret {
            let mut dependencies = dependencies_by_output
                .into_iter()
                .filter_map(|(output, dependencies)| match output {
                    Region::Exact { object, .. } | Region::UnknownObject(object)
                        if object == ret =>
                    {
                        Some(dependencies)
                    }
                    _ => None,
                })
                .flatten()
                .map(|dependency| (dependency.read, dependency.kind))
                .collect::<Vec<_>>();
            dependencies.sort_unstable();
            dependencies.dedup();
            dependencies
        } else {
            Vec::new()
        }
    }

    fn function_output_aligned_dependencies(
        &mut self,
        formal: VarId,
        outputs: &[AssignDestination],
        mapped_dependencies: &[(Region<VarId>, Vec<MappedFunctionRead>)],
    ) -> Option<Vec<Vec<AlignedDependency>>> {
        let formal_length = self
            .context
            .variables
            .get(&formal)?
            .total_width()?
            .checked_mul(
                self.context
                    .variables
                    .get(&formal)?
                    .r#type
                    .total_array()
                    .unwrap_or(1),
            )?;
        let mut high = formal_length;
        let mut destination_spans = Vec::with_capacity(outputs.len());
        for destination in outputs {
            let Region::Exact {
                span: destination_span,
                ..
            } = self.variable_region(destination.id, &destination.index, &destination.select)
            else {
                return None;
            };
            high = high.checked_sub(destination_span.length)?;
            destination_spans.push(Span {
                start: high,
                length: destination_span.length,
            });
        }
        if high != 0 {
            return None;
        }

        let mut aligned = vec![Vec::new(); outputs.len()];
        for &(output, ref dependencies) in mapped_dependencies {
            let Region::Exact {
                object,
                span: output_span,
            } = output
            else {
                return None;
            };
            if object != formal || output_span.end()? > formal_length {
                return None;
            }
            for dependency in dependencies {
                let Region::Exact {
                    span: input_span, ..
                } = dependency.input
                else {
                    return None;
                };
                if !dependency.aligned || input_span.length != output_span.length {
                    return None;
                }
                for (index, destination_span) in destination_spans.iter().copied().enumerate() {
                    let Some(overlap) = output_span.intersection(destination_span) else {
                        continue;
                    };
                    aligned[index].push(AlignedDependency {
                        read: dependency.read,
                        kind: dependency.kind,
                        source: Span {
                            start: overlap.start.checked_sub(output_span.start)?,
                            length: overlap.length,
                        },
                        destination: Span {
                            start: overlap.start.checked_sub(destination_span.start)?,
                            length: overlap.length,
                        },
                        repetitions: 1,
                        destination_stride: 0,
                    });
                }
            }
        }
        Some(aligned)
    }

    fn map_formal_region_to_actual(
        &mut self,
        expression: &Expression,
        formal_region: Region<VarId>,
    ) -> Option<Vec<PositionedRegion>> {
        let Region::Exact {
            span: formal_span, ..
        } = formal_region
        else {
            return None;
        };
        self.map_expression_span_positioned_to_actual(expression, formal_span)
    }

    /// Maps a low-bit-based span of an expression to the source variable
    /// regions which supply those bits. `None` means that this expression
    /// form is not modeled precisely and the caller must conservatively use
    /// all reads. `Some([])` means the requested bits are constant.
    fn map_expression_span_to_actual(
        &mut self,
        expression: &Expression,
        requested: Span,
    ) -> Option<Vec<Region<VarId>>> {
        self.map_expression_span_positioned_to_actual(expression, requested)
            .map(|mapped| {
                mapped
                    .into_iter()
                    .map(|positioned| positioned.region)
                    .collect()
            })
    }

    fn map_expression_span_positioned_to_actual(
        &mut self,
        expression: &Expression,
        requested: Span,
    ) -> Option<Vec<PositionedRegion>> {
        self.map_expression_span_positioned_with_signed(
            expression,
            requested,
            expression.comptime().r#type.signed,
        )
    }

    fn map_expression_span_positioned_with_signed(
        &mut self,
        expression: &Expression,
        requested: Span,
        extension_signed: bool,
    ) -> Option<Vec<PositionedRegion>> {
        let expression_type = &expression.comptime().r#type;
        let packed_width = expression_type.total_width()?;
        let expression_length =
            packed_width.checked_mul(expression_type.total_array().unwrap_or(1))?;
        if requested.end()? > expression_length {
            // Context sizing extends only a packed value. Unpacked shape
            // coercions are not bit extension and remain a conservative
            // fallback when their flattened lengths disagree.
            if expression_type.total_array().unwrap_or(1) != 1 {
                return None;
            }
            let mut mapped = Vec::new();
            if let Some(value_bits) = requested.intersection(Span {
                start: 0,
                length: packed_width,
            }) {
                mapped
                    .extend(self.map_expression_span_positioned_to_actual(expression, value_bits)?);
            }
            let extension = requested.intersection(Span {
                start: packed_width,
                length: requested.end()?.checked_sub(packed_width)?,
            });
            if extension_signed
                && packed_width != 0
                && let Some(extension) = extension
            {
                mapped.extend(
                    self.map_expression_span_positioned_to_actual(
                        expression,
                        Span {
                            start: packed_width - 1,
                            length: 1,
                        },
                    )?
                    .into_iter()
                    .map(|sign| PositionedRegion {
                        region: sign.region,
                        expression: extension,
                        aligned: false,
                        kind: EdgeKind::Value,
                        repetitions: 1,
                        expression_stride: 0,
                    }),
                );
            }
            return Some(mapped);
        }
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::Variable(id, index, select, _) => {
                    let Region::Exact {
                        object,
                        span: actual_span,
                    } = self.variable_region(*id, index, select)
                    else {
                        return None;
                    };
                    if requested.end()? > actual_span.length {
                        return None;
                    }
                    Some(vec![PositionedRegion {
                        region: Region::Exact {
                            object,
                            span: Span {
                                start: actual_span.start.checked_add(requested.start)?,
                                length: requested.length,
                            },
                        },
                        expression: requested,
                        aligned: true,
                        kind: EdgeKind::Value,
                        repetitions: 1,
                        expression_stride: 0,
                    }])
                }
                Factor::Value(_) => Some(Vec::new()),
                Factor::SystemFunctionCall(call) => match &call.kind {
                    SystemFunctionKind::Bits(_)
                    | SystemFunctionKind::Size(_)
                    | SystemFunctionKind::Clog2(_) => Some(Vec::new()),
                    SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                        self.map_expression_span_positioned_to_actual(&input.0, requested)
                    }
                    _ => None,
                },
                Factor::FunctionCall(call) => self
                    .map_function_return_spans_to_actual(call, requested)
                    .map(|mapped| {
                        mapped
                            .into_iter()
                            .map(|(region, relative, aligned)| PositionedRegion {
                                region,
                                expression: Span {
                                    start: requested.start + relative.start,
                                    length: relative.length,
                                },
                                aligned,
                                kind: EdgeKind::Value,
                                repetitions: 1,
                                expression_stride: 0,
                            })
                            .collect()
                    }),
                _ => None,
            },
            Expression::Ternary(condition, left, right, _) => {
                if let Some(selected) = self.constant_condition(condition) {
                    return self.map_expression_span_positioned_to_actual(
                        if selected { left } else { right },
                        requested,
                    );
                }
                let result_signed = expression.comptime().r#type.signed;
                let mut mapped = self.map_expression_span_positioned_with_signed(
                    left,
                    requested,
                    result_signed,
                )?;
                mapped.extend(self.map_expression_span_positioned_with_signed(
                    right,
                    requested,
                    result_signed,
                )?);
                let condition_width = condition.comptime().r#type.total_width()?;
                mapped.extend(
                    self.map_expression_span_positioned_to_actual(
                        condition,
                        Span {
                            start: 0,
                            length: condition_width,
                        },
                    )?
                    .into_iter()
                    .map(|condition| PositionedRegion {
                        region: condition.region,
                        expression: requested,
                        aligned: false,
                        kind: EdgeKind::Control,
                        repetitions: 1,
                        expression_stride: 0,
                    }),
                );
                Some(mapped)
            }
            Expression::Concatenation(parts, _) => {
                let mut low = 0usize;
                let mut mapped = Vec::new();
                for (part, repeat) in parts.iter().rev() {
                    let width = part.comptime().r#type.total_width()?;
                    let repeat = if let Some(repeat) = repeat {
                        repeat.clone().eval_value(&mut self.context)?.to_usize()?
                    } else {
                        1
                    };
                    for copy in 0..repeat {
                        let copy_start = low.checked_add(copy.checked_mul(width)?)?;
                        let copy_span = Span {
                            start: copy_start,
                            length: width,
                        };
                        let Some(overlap) = requested.intersection(copy_span) else {
                            continue;
                        };
                        let local = Span {
                            start: overlap.start.checked_sub(copy_start)?,
                            length: overlap.length,
                        };
                        mapped.extend(
                            self.map_expression_span_positioned_to_actual(part, local)?
                                .into_iter()
                                .map(|positioned| PositionedRegion {
                                    region: positioned.region,
                                    expression: Span {
                                        start: positioned.expression.start + copy_start,
                                        length: positioned.expression.length,
                                    },
                                    aligned: positioned.aligned,
                                    kind: positioned.kind,
                                    repetitions: positioned.repetitions,
                                    expression_stride: positioned.expression_stride,
                                }),
                        );
                    }
                    low = low.checked_add(width.checked_mul(repeat)?)?;
                }
                Some(mapped)
            }
            Expression::StructConstructor(r#type, fields, _) => {
                let TypeKind::Struct(structure) = &r#type.kind else {
                    return None;
                };
                if structure.members.len() != fields.len() {
                    return None;
                }

                // A packed structure is laid out like a concatenation in
                // declaration order: the first member occupies the most
                // significant bits. The IR constructor has already expanded
                // `default` and normalized named fields into that order.
                let mut high = r#type.total_width()?;
                let mut mapped = Vec::new();
                for (member, (name, value)) in structure.members.iter().zip(fields) {
                    if member.name != *name {
                        return None;
                    }
                    let member_width = member.r#type.total_width()?;
                    high = high.checked_sub(member_width)?;
                    let member_span = Span {
                        start: high,
                        length: member_width,
                    };
                    let Some(overlap) = requested.intersection(member_span) else {
                        continue;
                    };
                    let local = Span {
                        start: overlap.start.checked_sub(member_span.start)?,
                        length: overlap.length,
                    };
                    for positioned in self.map_expression_span_positioned_with_signed(
                        value,
                        local,
                        member.r#type.signed,
                    )? {
                        mapped.push(PositionedRegion {
                            region: positioned.region,
                            expression: Span {
                                start: positioned
                                    .expression
                                    .start
                                    .checked_add(member_span.start)?,
                                length: positioned.expression.length,
                            },
                            aligned: positioned.aligned,
                            kind: positioned.kind,
                            repetitions: positioned.repetitions,
                            expression_stride: positioned.expression_stride,
                        });
                    }
                }
                (high == 0).then_some(mapped)
            }
            Expression::ArrayLiteral(items, _) => {
                let dimensions = expression_type.array.as_slice();
                let element_count = dimensions.first().copied().flatten()?;
                let inner_count = dimensions[1..]
                    .iter()
                    .try_fold(1usize, |count, dimension| count.checked_mul((*dimension)?))?;
                let element_length = packed_width.checked_mul(inner_count)?;
                let mut element_type = expression_type.clone();
                element_type.array = crate::ir::Shape::from(&dimensions[1..]);
                element_type.set_array_expr(Vec::new());
                let mut next_element = 0usize;
                let mut default = None;
                let mut mapped = Vec::new();

                for item in items {
                    let (value, copies) = match item {
                        ArrayLiteralItem::Value(value, repeat) => {
                            let copies = if let Some(repeat) = repeat {
                                repeat.clone().eval_value(&mut self.context)?.to_usize()?
                            } else {
                                1
                            };
                            (value.as_ref(), copies)
                        }
                        ArrayLiteralItem::Defaul(value) => {
                            if default.replace(value.as_ref()).is_some() {
                                return None;
                            }
                            continue;
                        }
                    };
                    let end_element = next_element.checked_add(copies)?;
                    if end_element > element_count {
                        return None;
                    }
                    self.map_repeated_array_item(
                        value,
                        next_element,
                        end_element,
                        element_length,
                        requested,
                        &element_type,
                        &mut mapped,
                    )?;
                    next_element = end_element;
                }

                if next_element < element_count {
                    let default = default?;
                    self.map_repeated_array_item(
                        default,
                        next_element,
                        element_count,
                        element_length,
                        requested,
                        &element_type,
                        &mut mapped,
                    )?;
                    next_element = element_count;
                }
                (next_element == element_count).then_some(mapped)
            }
            Expression::Unary(Op::BitNot | Op::Add, operand, _) => {
                self.map_expression_span_positioned_to_actual(operand, requested)
            }
            Expression::Binary(left, Op::As, _, _) => self
                .map_expression_span_positioned_with_signed(
                    left,
                    requested,
                    expression.comptime().r#type.signed,
                ),
            Expression::Binary(left, op, right, _)
                if matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor) =>
            {
                let result_signed = expression.comptime().r#type.signed;
                let mut mapped = self.map_expression_span_positioned_with_signed(
                    left,
                    requested,
                    result_signed,
                )?;
                mapped.extend(self.map_expression_span_positioned_with_signed(
                    right,
                    requested,
                    result_signed,
                )?);
                Some(mapped)
            }
            Expression::Binary(left, op, right, _)
                if matches!(op, Op::LogicShiftL | Op::ArithShiftL)
                    && left.comptime().r#type.total_width()?
                        == expression.comptime().r#type.total_width()? =>
            {
                let width = expression.comptime().r#type.total_width()?;
                let shift = right
                    .clone()
                    .eval_value(&mut self.context)?
                    .to_usize()?
                    .min(width);
                let overlap = requested.intersection(Span {
                    start: shift,
                    length: width.checked_sub(shift)?,
                });
                let Some(overlap) = overlap else {
                    return Some(Vec::new());
                };
                let source = Span {
                    start: overlap.start.checked_sub(shift)?,
                    length: overlap.length,
                };
                Some(
                    self.map_expression_span_positioned_to_actual(left, source)?
                        .into_iter()
                        .map(|positioned| PositionedRegion {
                            region: positioned.region,
                            expression: Span {
                                start: positioned.expression.start + shift,
                                length: positioned.expression.length,
                            },
                            aligned: positioned.aligned,
                            kind: positioned.kind,
                            repetitions: positioned.repetitions,
                            expression_stride: positioned.expression_stride,
                        })
                        .collect(),
                )
            }
            Expression::Binary(left, Op::LogicShiftR, right, _)
                if left.comptime().r#type.total_width()?
                    == expression.comptime().r#type.total_width()? =>
            {
                let width = expression.comptime().r#type.total_width()?;
                let shift = right
                    .clone()
                    .eval_value(&mut self.context)?
                    .to_usize()?
                    .min(width);
                let Some(overlap) = requested.intersection(Span {
                    start: 0,
                    length: width.checked_sub(shift)?,
                }) else {
                    return Some(Vec::new());
                };
                let source = Span {
                    start: overlap.start.checked_add(shift)?,
                    length: overlap.length,
                };
                Some(
                    self.map_expression_span_positioned_to_actual(left, source)?
                        .into_iter()
                        .map(|positioned| PositionedRegion {
                            region: positioned.region,
                            expression: Span {
                                start: positioned.expression.start - shift,
                                length: positioned.expression.length,
                            },
                            aligned: positioned.aligned,
                            kind: positioned.kind,
                            repetitions: positioned.repetitions,
                            expression_stride: positioned.expression_stride,
                        })
                        .collect(),
                )
            }
            Expression::Binary(left, Op::ArithShiftR, right, _)
                if left.comptime().r#type.signed
                    && left.comptime().r#type.total_width()?
                        == expression.comptime().r#type.total_width()? =>
            {
                let width = expression.comptime().r#type.total_width()?;
                let shift = right
                    .clone()
                    .eval_value(&mut self.context)?
                    .to_usize()?
                    .min(width);
                let live_length = width.checked_sub(shift)?;
                let mut mapped = Vec::new();
                if let Some(overlap) = requested.intersection(Span {
                    start: 0,
                    length: live_length,
                }) {
                    let source = Span {
                        start: overlap.start.checked_add(shift)?,
                        length: overlap.length,
                    };
                    mapped.extend(
                        self.map_expression_span_positioned_to_actual(left, source)?
                            .into_iter()
                            .map(|positioned| PositionedRegion {
                                region: positioned.region,
                                expression: Span {
                                    start: positioned.expression.start - shift,
                                    length: positioned.expression.length,
                                },
                                aligned: positioned.aligned,
                                kind: positioned.kind,
                                repetitions: positioned.repetitions,
                                expression_stride: positioned.expression_stride,
                            }),
                    );
                }
                if let Some(fill) = requested.intersection(Span {
                    start: live_length,
                    length: shift,
                }) {
                    mapped.extend(
                        self.map_expression_span_positioned_to_actual(
                            left,
                            Span {
                                start: width.checked_sub(1)?,
                                length: 1,
                            },
                        )?
                        .into_iter()
                        .map(|sign| PositionedRegion {
                            region: sign.region,
                            expression: fill,
                            aligned: false,
                            kind: EdgeKind::Value,
                            repetitions: 1,
                            expression_stride: 0,
                        }),
                    );
                }
                Some(mapped)
            }
            _ => None,
        }
    }

    /// Project only the repeated array copies intersected by `requested`.
    /// This keeps a sparse element query independent of the declared array
    /// length while preserving bit positions inside every repeated copy.
    #[allow(clippy::too_many_arguments)]
    fn map_repeated_array_item(
        &mut self,
        value: &Expression,
        first_element: usize,
        end_element: usize,
        element_length: usize,
        requested: Span,
        element_type: &crate::ir::Type,
        mapped: &mut Vec<PositionedRegion>,
    ) -> Option<()> {
        if first_element == end_element || element_length == 0 {
            return Some(());
        }
        let covered = Span {
            start: first_element.checked_mul(element_length)?,
            length: end_element
                .checked_sub(first_element)?
                .checked_mul(element_length)?,
        };
        let Some(overlap) = requested.intersection(covered) else {
            return Some(());
        };
        let mut contextual_value = None;
        if matches!(value, Expression::ArrayLiteral(_, _)) {
            let mut value = value.clone();
            value.comptime_mut().r#type = element_type.clone();
            contextual_value = Some(value);
        }
        let value = contextual_value.as_ref().unwrap_or(value);
        let first_copy = overlap.start / element_length;
        let last_copy = overlap.end()?.checked_sub(1)?.checked_div(element_length)?;
        let first_full = overlap
            .start
            .checked_add(element_length - 1)?
            .checked_div(element_length)?
            .max(first_copy);
        let end_full = overlap
            .end()?
            .checked_div(element_length)?
            .min(last_copy.saturating_add(1));

        for copy in [
            Some(first_copy),
            (last_copy != first_copy).then_some(last_copy),
        ]
        .into_iter()
        .flatten()
        {
            let copy_start = copy.checked_mul(element_length)?;
            let copy_span = Span {
                start: copy_start,
                length: element_length,
            };
            let Some(copy_overlap) = requested.intersection(copy_span) else {
                continue;
            };
            if copy_overlap == copy_span {
                continue;
            }
            let local = Span {
                start: copy_overlap.start.checked_sub(copy_start)?,
                length: copy_overlap.length,
            };
            for positioned in
                self.map_expression_span_positioned_with_signed(value, local, element_type.signed)?
            {
                mapped.push(PositionedRegion {
                    region: positioned.region,
                    expression: Span {
                        start: positioned.expression.start.checked_add(copy_start)?,
                        length: positioned.expression.length,
                    },
                    aligned: positioned.aligned,
                    kind: positioned.kind,
                    repetitions: positioned.repetitions,
                    expression_stride: positioned.expression_stride,
                });
            }
        }

        if first_full < end_full {
            let copies = end_full - first_full;
            let copy_start = first_full.checked_mul(element_length)?;
            for mut positioned in self.map_expression_span_positioned_with_signed(
                value,
                Span {
                    start: 0,
                    length: element_length,
                },
                element_type.signed,
            )? {
                positioned.expression.start =
                    positioned.expression.start.checked_add(copy_start)?;
                if copies > 1 {
                    if positioned.repetitions == 1 {
                        positioned.repetitions = copies;
                        positioned.expression_stride = element_length;
                    } else if positioned
                        .expression_stride
                        .checked_mul(positioned.repetitions)
                        == Some(element_length)
                    {
                        positioned.repetitions = positioned.repetitions.checked_mul(copies)?;
                    } else {
                        // A non-flattenable nested pattern retains one compact
                        // inner pattern per outer copy. The number of explicit
                        // items then reflects source structure, not bit width.
                        for copy in 0..copies {
                            let mut positioned = positioned;
                            positioned.expression.start = positioned
                                .expression
                                .start
                                .checked_add(copy.checked_mul(element_length)?)?;
                            mapped.push(positioned);
                        }
                        continue;
                    }
                }
                mapped.push(positioned);
            }
        }
        Some(())
    }

    fn map_function_return_spans_to_actual(
        &mut self,
        call: &FunctionCall,
        requested: Span,
    ) -> Option<Vec<(Region<VarId>, Span, bool)>> {
        if self.call_stack.contains(&call.id) {
            return None;
        }
        let function_body = self.context.functions.get(&call.id).and_then(|function| {
            if let Some(index) = &call.index {
                function.get_function(index)
            } else {
                function.get_function(&[])
            }
        })?;
        let ret = function_body.ret?;

        let mut actual_inputs = BTreeMap::<VarId, &Expression>::new();
        for (path, input) in &call.inputs {
            if let Some(&formal) = function_body.arg_map.get(path) {
                actual_inputs.insert(formal, input);
            }
        }

        let mut nested = self.nested(call.id);
        let exit = nested
            .lower_statements(&function_body.statements, 0, &[], None)
            .unwrap_or(0);
        nested.procedure.exit = exit;
        if nested.model_error.is_some() {
            return None;
        }
        let summary = procedure::analyze(&nested.procedure).ok()?;
        let mut mapped = Vec::new();
        for dependency in summary.dependencies {
            let Region::Exact {
                object: output,
                span: output_span,
            } = dependency.output
            else {
                continue;
            };
            let Some(output_overlap) = output_span.intersection(requested) else {
                continue;
            };
            if output != ret {
                continue;
            }
            if dependency.kind == EdgeKind::Unknown {
                continue;
            }
            let Region::Exact {
                object: input,
                span: input_span,
            } = dependency.input
            else {
                continue;
            };
            let relative_output = Span {
                start: output_overlap.start.checked_sub(requested.start)?,
                length: output_overlap.length,
            };

            let mapped_input = if dependency.aligned {
                if input_span.length != output_span.length {
                    return None;
                }
                let relative_start = output_overlap.start.checked_sub(output_span.start)?;
                Region::Exact {
                    object: input,
                    span: Span {
                        start: input_span.start.checked_add(relative_start)?,
                        length: output_overlap.length,
                    },
                }
            } else {
                dependency.input
            };

            if let Some(actual) = actual_inputs.get(&input) {
                for actual in self.map_formal_region_to_actual(actual, mapped_input)? {
                    let actual_region = actual.region;
                    let Region::Exact {
                        span: actual_span, ..
                    } = actual_region
                    else {
                        return None;
                    };
                    let aligned = dependency.aligned && actual.aligned;
                    if aligned && actual_span.length != output_overlap.length {
                        return None;
                    }
                    mapped.push((actual_region, relative_output, aligned));
                }
            } else if self
                .context
                .variables
                .get(&input)
                .is_some_and(|variable| variable.affiliation == crate::symbol::Affiliation::Module)
            {
                mapped.push((mapped_input, relative_output, dependency.aligned));
            } else {
                return None;
            }
        }
        mapped.sort_unstable();
        mapped.dedup();
        Some(mapped)
    }

    fn lower_system_function(
        &mut self,
        block: usize,
        call: &SystemFunctionCall,
        value_position: bool,
    ) -> Vec<(usize, EdgeKind)> {
        match &call.kind {
            SystemFunctionKind::Bits(_)
            | SystemFunctionKind::Size(_)
            | SystemFunctionKind::Clog2(_) => Vec::new(),
            SystemFunctionKind::Onehot(input)
            | SystemFunctionKind::Signed(input)
            | SystemFunctionKind::Unsigned(input) => {
                self.read_expression(block, &input.0, EdgeKind::Value)
            }
            SystemFunctionKind::Readmemh(input, output) => {
                self.procedure
                    .incomplete
                    .insert(IncompleteReason::TimedOrEventEffect);
                let dependencies = self.read_expression(block, &input.0, EdgeKind::Unknown);
                for destination in &output.0 {
                    self.write_destination(block, destination, dependencies.clone(), Vec::new());
                }
                Vec::new()
            }
            SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
                // Observer reads affect process sensitivity, but never a
                // signal definition. Only preserve nested function output
                // side effects; recording ordinary reads here would let a
                // dynamic observer unnecessarily taint the whole procedure.
                for input in inputs {
                    self.lower_observer_expression(block, &input.0);
                }
                Vec::new()
            }
            SystemFunctionKind::Assert { cond, args, .. } => {
                self.lower_observer_expression(block, &cond.0);
                for input in args {
                    self.lower_observer_expression(block, &input.0);
                }
                Vec::new()
            }
            SystemFunctionKind::Finish => Vec::new(),
        }
        .into_iter()
        .map(|(read, kind)| {
            if value_position {
                (read, kind)
            } else {
                (read, EdgeKind::Value)
            }
        })
        .collect()
    }

    fn lower_observer_expression(&mut self, block: usize, expression: &Expression) {
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::FunctionCall(call) => {
                    self.lower_function_call(block, call, &[]);
                }
                Factor::SystemFunctionCall(call) => {
                    self.lower_system_function(block, call, false);
                }
                _ => {}
            },
            Expression::Unary(_, expression, _) => {
                self.lower_observer_expression(block, expression);
            }
            Expression::Binary(left, op, right, _) => {
                self.lower_observer_expression(block, left);
                if self.binary_rhs_is_reachable(left, *op) {
                    self.lower_observer_expression(block, right);
                }
            }
            Expression::Ternary(condition, left, right, _) => {
                self.lower_observer_expression(block, condition);
                match self.constant_condition(condition) {
                    Some(true) => self.lower_observer_expression(block, left),
                    Some(false) => self.lower_observer_expression(block, right),
                    None => {
                        self.lower_observer_expression(block, left);
                        self.lower_observer_expression(block, right);
                    }
                }
            }
            Expression::Concatenation(parts, _) => {
                for (expression, repeat) in parts {
                    self.lower_observer_expression(block, expression);
                    if let Some(repeat) = repeat {
                        self.lower_observer_expression(block, repeat);
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(expression, repeat) => {
                            self.lower_observer_expression(block, expression);
                            if let Some(repeat) = repeat {
                                self.lower_observer_expression(block, repeat);
                            }
                        }
                        ArrayLiteralItem::Defaul(expression) => {
                            self.lower_observer_expression(block, expression);
                        }
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, expression) in fields {
                    self.lower_observer_expression(block, expression);
                }
            }
        }
    }

    fn constant_condition(&mut self, expression: &Expression) -> Option<bool> {
        let value = expression.clone().eval_value(&mut self.context)?;
        (!value.is_xz()).then(|| value.is_positive())
    }

    fn binary_rhs_is_reachable(&mut self, left: &Expression, op: Op) -> bool {
        match op {
            Op::LogicAnd => self.constant_condition(left) != Some(false),
            Op::LogicOr => self.constant_condition(left) != Some(true),
            _ => true,
        }
    }

    fn write_destination(
        &mut self,
        block: usize,
        destination: &AssignDestination,
        mut dependencies: Vec<(usize, EdgeKind)>,
        aligned_dependencies: Vec<AlignedDependency>,
    ) {
        let mut selector_reads = Vec::new();
        for expression in destination
            .index
            .0
            .iter()
            .chain(destination.select.0.iter())
        {
            selector_reads.extend(self.read_expression(block, expression, EdgeKind::Address));
        }
        if let Some((_, expression)) = &destination.select.1 {
            selector_reads.extend(self.read_expression(block, expression, EdgeKind::Address));
        }
        dependencies.extend(selector_reads.iter().copied());
        for region in self.variable_regions(destination.id, &destination.index, &destination.select)
        {
            let id = self.next_write;
            self.next_write += 1;
            self.write_tokens.insert(id, destination.token);
            if let Some(key) = Self::unresolved_access_key(
                destination.id,
                &destination.index,
                &destination.select,
                region,
            ) {
                self.unresolved_writes
                    .entry(key)
                    .or_default()
                    .push((id, selector_reads.iter().map(|&(read, _)| read).collect()));
            }
            self.procedure.events[block].push(Event::Write {
                id,
                region,
                dependencies: dependencies.clone(),
                aligned_dependencies: aligned_dependencies.clone(),
            });
        }
    }

    fn aligned_assignment_dependencies(
        &mut self,
        block: usize,
        expression: &Expression,
        destinations: &[AssignDestination],
        dependencies: &mut Vec<(usize, EdgeKind)>,
        extension_signed: Option<bool>,
    ) -> Option<Vec<Vec<AlignedDependency>>> {
        let mut expression_width = 0usize;
        let mut destination_lengths = Vec::with_capacity(destinations.len());
        for destination in destinations {
            let Region::Exact {
                span: destination_span,
                ..
            } = self.variable_region(destination.id, &destination.index, &destination.select)
            else {
                return None;
            };
            expression_width = expression_width.checked_add(destination_span.length)?;
            destination_lengths.push(destination_span.length);
        }
        let mut high = expression_width;
        let mut destination_spans = Vec::with_capacity(destinations.len());
        for length in destination_lengths {
            high = high.checked_sub(length)?;
            destination_spans.push(Span {
                start: high,
                length,
            });
        }

        let mut aligned = Vec::new();
        let positioned = self.map_expression_span_positioned_with_signed(
            expression,
            Span {
                start: 0,
                length: expression_width,
            },
            extension_signed.unwrap_or(expression.comptime().r#type.signed),
        )?;
        let mut retained = Vec::new();
        for positioned in positioned {
            let Region::Exact { span, .. } = positioned.region else {
                return None;
            };
            let read = self.push_read(block, positioned.region);
            if positioned.aligned {
                if span.length != positioned.expression.length {
                    return None;
                }
                aligned.push(AlignedDependency {
                    read,
                    kind: positioned.kind,
                    source: Span {
                        start: 0,
                        length: span.length,
                    },
                    destination: positioned.expression,
                    repetitions: positioned.repetitions,
                    destination_stride: positioned.expression_stride,
                });
            } else {
                retained.push((read, positioned.kind));
            }
        }
        *dependencies = retained;
        let mut ret = vec![Vec::new(); destinations.len()];
        for dependency in aligned {
            let pattern_end = dependency
                .destination_stride
                .checked_mul(dependency.repetitions.saturating_sub(1))?
                .checked_add(dependency.destination.end()?)?;
            if dependency.repetitions > 1
                && let Some((index, destination)) = destination_spans
                    .iter()
                    .copied()
                    .enumerate()
                    .find(|(_, destination)| {
                        dependency.destination.start >= destination.start
                            && pattern_end <= destination.end().unwrap_or(0)
                    })
            {
                ret[index].push(AlignedDependency {
                    read: dependency.read,
                    kind: dependency.kind,
                    source: dependency.source,
                    destination: Span {
                        start: dependency
                            .destination
                            .start
                            .checked_sub(destination.start)?,
                        length: dependency.destination.length,
                    },
                    repetitions: dependency.repetitions,
                    destination_stride: dependency.destination_stride,
                });
                continue;
            }
            for (index, destination) in destination_spans.iter().copied().enumerate() {
                for copy in 0..dependency.repetitions {
                    let copy_start = dependency
                        .destination
                        .start
                        .checked_add(copy.checked_mul(dependency.destination_stride)?)?;
                    let copy_span = Span {
                        start: copy_start,
                        length: dependency.destination.length,
                    };
                    let Some(overlap) = copy_span.intersection(destination) else {
                        continue;
                    };
                    let source_offset = overlap.start.checked_sub(copy_span.start)?;
                    let destination_offset = overlap.start.checked_sub(destination.start)?;
                    ret[index].push(AlignedDependency {
                        read: dependency.read,
                        kind: dependency.kind,
                        source: Span {
                            start: dependency.source.start.checked_add(source_offset)?,
                            length: overlap.length,
                        },
                        destination: Span {
                            start: destination_offset,
                            length: overlap.length,
                        },
                        repetitions: 1,
                        destination_stride: 0,
                    });
                }
            }
        }
        Some(ret)
    }

    fn push_read(&mut self, block: usize, region: Region<VarId>) -> usize {
        let id = self.next_read;
        self.next_read += 1;
        self.procedure.events[block].push(Event::Read { id, region });
        id
    }

    fn push_variable_read(
        &mut self,
        block: usize,
        object: VarId,
        index: &VarIndex,
        select: &VarSelect,
        region: Region<VarId>,
        selector_reads: &[(usize, EdgeKind)],
    ) -> usize {
        let read = self.push_read(block, region);
        if let Some(key) = Self::unresolved_access_key(object, index, select, region) {
            let read_selectors = selector_reads
                .iter()
                .map(|&(selector, _)| selector)
                .collect::<Vec<_>>();
            if let Some(writes) = self.unresolved_writes.get(&key) {
                for (write, write_selectors) in writes {
                    if write_selectors.len() == read_selectors.len() {
                        self.procedure.must_alias.insert(MustAliasCandidate {
                            read,
                            write: *write,
                            selector_reads: write_selectors
                                .iter()
                                .copied()
                                .zip(read_selectors.iter().copied())
                                .collect(),
                        });
                    }
                }
            }
        }
        read
    }

    fn unresolved_access_key(
        object: VarId,
        index: &VarIndex,
        select: &VarSelect,
        region: Region<VarId>,
    ) -> Option<String> {
        (!region.is_exact()).then(|| format!("{object:?}{index}{select}"))
    }

    fn unknown_read(&mut self, block: usize) -> usize {
        self.push_read(block, Region::UnknownAll)
    }

    fn variable_region(
        &mut self,
        id: VarId,
        index: &VarIndex,
        select: &VarSelect,
    ) -> Region<VarId> {
        let regions = self.variable_regions(id, index, select);
        if regions.len() == 1 {
            return regions[0];
        }
        let mut start = usize::MAX;
        let mut end = 0usize;
        for region in regions {
            let Region::UnknownRegion { object, span } = region else {
                return Region::UnknownObject(id);
            };
            if object != id {
                return Region::UnknownObject(id);
            }
            start = start.min(span.start);
            let Some(span_end) = span.end() else {
                return Region::UnknownObject(id);
            };
            end = end.max(span_end);
        }
        let Some(length) = end.checked_sub(start) else {
            return Region::UnknownObject(id);
        };
        Region::UnknownRegion {
            object: id,
            span: Span { start, length },
        }
    }

    fn variable_regions(
        &mut self,
        id: VarId,
        index: &VarIndex,
        select: &VarSelect,
    ) -> Vec<Region<VarId>> {
        let Some(variable) = self.context.variables.get(&id).cloned() else {
            return vec![Region::UnknownObject(id)];
        };
        let Some(width) = variable.total_width() else {
            return vec![Region::UnknownObject(id)];
        };
        vec![self.variable_region_fallback(id, &variable, index, select, width)]
    }

    fn variable_region_fallback(
        &mut self,
        id: VarId,
        variable: &crate::ir::Variable,
        index: &VarIndex,
        select: &VarSelect,
        width: usize,
    ) -> Region<VarId> {
        if index.0.is_empty() && select.0.is_empty() && select.1.is_none() {
            let Some(length) = variable
                .r#type
                .total_array()
                .and_then(|elements| elements.checked_mul(width))
            else {
                return Region::UnknownObject(id);
            };
            return Region::Exact {
                object: id,
                span: Span { start: 0, length },
            };
        }
        // Preserve the longest statically known prefix. `eval_value` maps an
        // X/Z numeric index to zero for simulation convenience, so alias
        // analysis must inspect every constant before accepting its value.
        let mut index_path = Vec::new();
        for expression in &index.0 {
            if !expression.comptime().is_const {
                break;
            }
            let Some(value) = expression.eval_value(&mut self.context) else {
                break;
            };
            let Some(value) = (!value.is_xz()).then(|| value.to_usize()).flatten() else {
                break;
            };
            index_path.push(value);
        }
        if index_path.len() != index.0.len() {
            let Some((first, last)) = variable.r#type.array.calc_range(&index_path) else {
                return Region::UnknownObject(id);
            };
            let Some(start) = first.checked_mul(width) else {
                return Region::UnknownObject(id);
            };
            let Some(length) = last
                .checked_sub(first)
                .and_then(|count| count.checked_add(1))
                .and_then(|count| count.checked_mul(width))
            else {
                return Region::UnknownObject(id);
            };
            return Region::UnknownRegion {
                object: id,
                span: Span { start, length },
            };
        }
        let Some(array_index) = variable.r#type.array.calc_index(&index_path) else {
            return Region::UnknownObject(id);
        };
        let Some(array_base) = array_index.checked_mul(width) else {
            return Region::UnknownObject(id);
        };

        let mut select_prefix = 0;
        for expression in &select.0 {
            if !expression.comptime().is_const {
                break;
            }
            let Some(value) = expression.eval_value(&mut self.context) else {
                break;
            };
            if value.is_xz() || value.to_usize().is_none() {
                break;
            }
            select_prefix += 1;
        }
        let range_is_known = select.1.as_ref().is_none_or(|(_, expression)| {
            expression.comptime().is_const
                && expression
                    .eval_value(&mut self.context)
                    .is_some_and(|value| !value.is_xz() && value.to_usize().is_some())
        });
        if select_prefix == select.0.len() && range_is_known {
            let Some((high, low)) = select.eval_value(&mut self.context, &variable.r#type, false)
            else {
                return Region::UnknownObject(id);
            };
            let Some(start) = array_base.checked_add(low) else {
                return Region::UnknownObject(id);
            };
            let Some(length) = high.checked_sub(low).and_then(|delta| delta.checked_add(1)) else {
                return Region::UnknownObject(id);
            };
            return Region::Exact {
                object: id,
                span: Span { start, length },
            };
        }

        // An unresolved range bound invalidates its base dimension too: the
        // selected interval may extend beyond that one element.
        if select_prefix == select.0.len() && !range_is_known {
            select_prefix = select_prefix.saturating_sub(1);
        }
        let (high, low) = if select_prefix == 0 {
            (width.saturating_sub(1), 0)
        } else {
            let prefix = VarSelect(select.0[..select_prefix].to_vec(), None);
            let Some(span) = prefix.eval_value(&mut self.context, &variable.r#type, false) else {
                return Region::UnknownObject(id);
            };
            span
        };
        let start = match array_index
            .checked_mul(width)
            .and_then(|base| base.checked_add(low))
        {
            Some(start) => start,
            None => return Region::UnknownObject(id),
        };
        let Some(length) = high.checked_sub(low).and_then(|delta| delta.checked_add(1)) else {
            return Region::UnknownObject(id);
        };
        Region::UnknownRegion {
            object: id,
            span: Span { start, length },
        }
    }
}

fn combine_edge_kinds(left: EdgeKind, right: EdgeKind) -> EdgeKind {
    if left == EdgeKind::Unknown || right == EdgeKind::Unknown {
        EdgeKind::Unknown
    } else if left == EdgeKind::Control || right == EdgeKind::Control {
        EdgeKind::Control
    } else if left == EdgeKind::Address || right == EdgeKind::Address {
        EdgeKind::Address
    } else {
        EdgeKind::Value
    }
}
