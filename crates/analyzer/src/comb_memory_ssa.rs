//! Veryl IR adapter for the IR-independent region MemorySSA engine.

use std::collections::{BTreeMap, BTreeSet};

use crate::conv::Context;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, CasePattern, CombDeclaration, Expression, Factor,
    ForBound, ForRange, FunctionCall, Module, Op, Statement, SystemFunctionCall,
    SystemFunctionKind, VarId, VarIndex, VarSelect,
};
use veryl_causal::graph::{EdgeKind, IncompleteReason};
use veryl_causal::procedure::{
    self, AlignedDependency, Event, Procedure, ProcedureError, ProcedureSummary,
};
use veryl_causal::region::{Region, Span};

pub(crate) fn analyze(
    module: &Module,
    declaration: &CombDeclaration,
) -> Result<ProcedureSummary<VarId>, ProcedureError> {
    let mut builder = Builder::new(module);
    let exit = builder
        .lower_statements(&declaration.statements, 0, &[], None)
        .unwrap_or(0);
    builder.procedure.exit = exit;
    if let Some(message) = builder.model_error {
        return Err(ProcedureError::Model(message));
    }
    procedure::analyze(&builder.procedure)
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

struct Builder {
    context: Context,
    procedure: Procedure<VarId>,
    next_read: usize,
    next_write: usize,
    call_stack: Vec<VarId>,
    model_error: Option<&'static str>,
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
                incomplete: BTreeSet::new(),
            },
            next_read: 0,
            next_write: 0,
            call_stack: Vec::new(),
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
                incomplete: BTreeSet::new(),
            },
            next_read: 0,
            next_write: 0,
            call_stack,
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
        for statement in statements {
            block = self.lower_statement(statement, block, controls, break_target)?;
        }
        Some(block)
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
                let mut dependencies = self.read_expression(block, &assign.expr, EdgeKind::Value);
                let aligned_by_destination = self
                    .aligned_assignment_dependencies(
                        block,
                        &assign.expr,
                        &assign.dst,
                        &mut dependencies,
                    )
                    .unwrap_or_else(|| vec![Vec::new(); assign.dst.len()]);
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
            Expression::Binary(left, _, right, _) => {
                reads.extend(self.read_expression(block, left, kind));
                reads.extend(self.read_expression(block, right, kind));
            }
            Expression::Ternary(condition, left, right, _) => {
                reads.extend(self.read_expression(block, condition, EdgeKind::Control));
                reads.extend(self.read_expression(block, left, kind));
                reads.extend(self.read_expression(block, right, kind));
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
                for expression in index.0.iter().chain(select.0.iter()) {
                    reads.extend(self.read_expression(block, expression, EdgeKind::Address));
                }
                if let Some((_, expression)) = &select.1 {
                    reads.extend(self.read_expression(block, expression, EdgeKind::Address));
                }
                let region = self.variable_region(*id, index, select);
                reads.push((self.push_read(block, region), kind));
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
        let mut dependencies_by_output = BTreeMap::<Region<VarId>, Vec<(usize, EdgeKind)>>::new();
        for dependency in &summary.dependencies {
            let mut mapped = Vec::new();
            let input_object = match dependency.input {
                Region::Exact { object, .. } | Region::UnknownObject(object) => Some(object),
                Region::UnknownAll => None,
            };
            if let Some(formal) =
                input_object.and_then(|object| actual_inputs.get_key_value(&object))
            {
                let (_, (actual_expression, fallback_reads)) = formal;
                if let Some(actual_regions) =
                    self.map_formal_region_to_actual(actual_expression, dependency.input)
                {
                    mapped.extend(actual_regions.into_iter().map(|actual_region| {
                        (self.push_read(block, actual_region), dependency.kind)
                    }));
                } else {
                    mapped.extend(fallback_reads.iter().map(|&(read, actual_kind)| {
                        (read, combine_edge_kinds(dependency.kind, actual_kind))
                    }));
                }
            } else if input_object.is_some_and(|object| {
                self.context.variables.get(&object).is_some_and(|variable| {
                    variable.affiliation == crate::symbol::Affiliation::Module
                })
            }) {
                mapped.push((self.push_read(block, dependency.input), dependency.kind));
            } else if input_object.is_some_and(|object| formal_ids.contains(&object)) {
                self.model_error
                    .get_or_insert("function dependency refers to an unmapped input argument");
                continue;
            } else {
                self.procedure
                    .incomplete
                    .insert(IncompleteReason::MalformedModel);
                mapped.push((self.unknown_read(block), EdgeKind::Unknown));
            }
            mapped.sort_unstable();
            mapped.dedup();
            dependencies_by_output
                .entry(dependency.output)
                .or_default()
                .extend(mapped);
        }
        for dependencies in dependencies_by_output.values_mut() {
            dependencies.sort_unstable();
            dependencies.dedup();
        }

        for (&output, dependencies) in &dependencies_by_output {
            let output_object = match output {
                Region::Exact { object, .. } | Region::UnknownObject(object) => object,
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
            let mut dependencies = dependencies.clone();
            dependencies.extend_from_slice(controls);
            let id = self.next_write;
            self.next_write += 1;
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
            let mut dependencies = dependencies_by_output
                .iter()
                .filter_map(|(output, dependencies)| match output {
                    Region::Exact { object, .. } | Region::UnknownObject(object)
                        if *object == formal =>
                    {
                        Some(dependencies.iter().copied())
                    }
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>();
            dependencies.sort_unstable();
            dependencies.dedup();
            dependencies.extend_from_slice(controls);
            for destination in outputs {
                self.write_destination(block, destination, dependencies.clone(), Vec::new());
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
                .collect::<Vec<_>>();
            dependencies.sort_unstable();
            dependencies.dedup();
            dependencies
        } else {
            Vec::new()
        }
    }

    fn map_formal_region_to_actual(
        &mut self,
        expression: &Expression,
        formal_region: Region<VarId>,
    ) -> Option<Vec<Region<VarId>>> {
        let Region::Exact {
            span: formal_span, ..
        } = formal_region
        else {
            return None;
        };
        if formal_span.end()? > expression.comptime().r#type.total_width()? {
            return None;
        }
        self.map_expression_span_to_actual(expression, formal_span)
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
                    Some(vec![Region::Exact {
                        object,
                        span: Span {
                            start: actual_span.start.checked_add(requested.start)?,
                            length: requested.length,
                        },
                    }])
                }
                Factor::Value(_) => Some(Vec::new()),
                Factor::SystemFunctionCall(call) => match &call.kind {
                    SystemFunctionKind::Bits(_)
                    | SystemFunctionKind::Size(_)
                    | SystemFunctionKind::Clog2(_) => Some(Vec::new()),
                    SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                        self.map_expression_span_to_actual(&input.0, requested)
                    }
                    _ => None,
                },
                Factor::FunctionCall(call) => {
                    self.map_function_return_span_to_actual(call, requested)
                }
                _ => None,
            },
            Expression::Concatenation(parts, _) => {
                let mut low = 0usize;
                let mut mapped = Vec::new();
                // Veryl and SystemVerilog list concatenation operands from
                // most to least significant. Walk in reverse so `low` stays
                // in the same LSB-based coordinate system as Region::Span.
                for (part, repeat) in parts.iter().rev() {
                    if repeat.is_some() {
                        return None;
                    }
                    let width = part.comptime().r#type.total_width()?;
                    let part_span = Span {
                        start: low,
                        length: width,
                    };
                    if let Some(overlap) = requested.intersection(part_span) {
                        let local = Span {
                            start: overlap.start.checked_sub(low)?,
                            length: overlap.length,
                        };
                        mapped.extend(self.map_expression_span_to_actual(part, local)?);
                    }
                    low = low.checked_add(width)?;
                }
                Some(mapped)
            }
            Expression::Unary(op, operand, _)
                if matches!(op, Op::BitNot | Op::Add)
                    && operand.comptime().r#type.total_width()?
                        == expression.comptime().r#type.total_width()? =>
            {
                self.map_expression_span_to_actual(operand, requested)
            }
            Expression::Binary(left, op, right, _)
                if matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor)
                    && left.comptime().r#type.total_width()?
                        == expression.comptime().r#type.total_width()?
                    && right.comptime().r#type.total_width()?
                        == expression.comptime().r#type.total_width()? =>
            {
                let mut mapped = self.map_expression_span_to_actual(left, requested)?;
                mapped.extend(self.map_expression_span_to_actual(right, requested)?);
                Some(mapped)
            }
            _ => None,
        }
    }

    fn map_function_return_span_to_actual(
        &mut self,
        call: &FunctionCall,
        requested: Span,
    ) -> Option<Vec<Region<VarId>>> {
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
            if !dependency.aligned {
                return None;
            }
            let Region::Exact {
                object: input,
                span: input_span,
            } = dependency.input
            else {
                continue;
            };
            let actual = actual_inputs.get(&input)?;
            if input_span.length != output_span.length {
                return None;
            }
            let relative_start = output_overlap.start.checked_sub(output_span.start)?;
            let formal_region = Region::Exact {
                object: input,
                span: Span {
                    start: input_span.start.checked_add(relative_start)?,
                    length: output_overlap.length,
                },
            };
            mapped.extend(self.map_formal_region_to_actual(actual, formal_region)?);
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
                Factor::FunctionCall(call) if !call.outputs.is_empty() => {
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
            Expression::Binary(left, _, right, _) => {
                self.lower_observer_expression(block, left);
                self.lower_observer_expression(block, right);
            }
            Expression::Ternary(condition, left, right, _) => {
                self.lower_observer_expression(block, condition);
                self.lower_observer_expression(block, left);
                self.lower_observer_expression(block, right);
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

    fn write_destination(
        &mut self,
        block: usize,
        destination: &AssignDestination,
        mut dependencies: Vec<(usize, EdgeKind)>,
        aligned_dependencies: Vec<AlignedDependency>,
    ) {
        for expression in destination
            .index
            .0
            .iter()
            .chain(destination.select.0.iter())
        {
            dependencies.extend(self.read_expression(block, expression, EdgeKind::Address));
        }
        if let Some((_, expression)) = &destination.select.1 {
            dependencies.extend(self.read_expression(block, expression, EdgeKind::Address));
        }
        let region = self.variable_region(destination.id, &destination.index, &destination.select);
        let id = self.next_write;
        self.next_write += 1;
        self.procedure.events[block].push(Event::Write {
            id,
            region,
            dependencies,
            aligned_dependencies,
        });
    }

    fn aligned_assignment_dependencies(
        &mut self,
        block: usize,
        expression: &Expression,
        destinations: &[AssignDestination],
        dependencies: &mut Vec<(usize, EdgeKind)>,
    ) -> Option<Vec<Vec<AlignedDependency>>> {
        let expression_width = expression.comptime().r#type.total_width()?;
        let mut high = expression_width;
        let mut destination_spans = Vec::with_capacity(destinations.len());
        for destination in destinations {
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

        let mut dependency_cursor = 0usize;
        let mut aligned = Vec::new();
        self.collect_aligned_expression_dependencies(
            block,
            expression,
            Span {
                start: 0,
                length: expression_width,
            },
            dependencies,
            &mut dependency_cursor,
            &mut aligned,
        )?;
        if dependency_cursor != dependencies.len() {
            return None;
        }
        dependencies.clear();
        let mut ret = vec![Vec::new(); destinations.len()];
        for dependency in aligned {
            for (index, destination) in destination_spans.iter().copied().enumerate() {
                let Some(overlap) = dependency.destination.intersection(destination) else {
                    continue;
                };
                let source_offset = overlap.start.checked_sub(dependency.destination.start)?;
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
                });
            }
        }
        Some(ret)
    }

    fn collect_aligned_expression_dependencies(
        &mut self,
        block: usize,
        expression: &Expression,
        destination: Span,
        dependencies: &[(usize, EdgeKind)],
        dependency_cursor: &mut usize,
        aligned: &mut Vec<AlignedDependency>,
    ) -> Option<()> {
        if expression.comptime().r#type.total_width()? != destination.length {
            return None;
        }
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::Value(_) => Some(()),
                Factor::Variable(id, index, select, _) => {
                    let Region::Exact {
                        span: source_span, ..
                    } = self.variable_region(*id, index, select)
                    else {
                        return None;
                    };
                    if source_span.length != destination.length {
                        return None;
                    }
                    let &(read, kind) = dependencies.get(*dependency_cursor)?;
                    if kind != EdgeKind::Value {
                        return None;
                    }
                    *dependency_cursor += 1;
                    aligned.push(AlignedDependency {
                        read,
                        kind,
                        source: Span {
                            start: 0,
                            length: source_span.length,
                        },
                        destination,
                    });
                    Some(())
                }
                Factor::FunctionCall(call) => {
                    let regions = self.map_function_return_span_to_actual(
                        call,
                        Span {
                            start: 0,
                            length: destination.length,
                        },
                    )?;
                    let dependency_count = regions.len();
                    for region in regions {
                        let Region::Exact { span, .. } = region else {
                            return None;
                        };
                        if span.length != destination.length {
                            return None;
                        }
                        aligned.push(AlignedDependency {
                            read: self.push_read(block, region),
                            kind: EdgeKind::Value,
                            source: Span {
                                start: 0,
                                length: span.length,
                            },
                            destination,
                        });
                    }
                    for _ in 0..dependency_count {
                        let (_, kind) = *dependencies.get(*dependency_cursor)?;
                        if kind != EdgeKind::Value {
                            return None;
                        }
                        *dependency_cursor += 1;
                    }
                    Some(())
                }
                _ => None,
            },
            Expression::Concatenation(parts, _) => {
                let mut low = destination.end()?;
                for (part, repeat) in parts {
                    if repeat.is_some() {
                        return None;
                    }
                    let width = part.comptime().r#type.total_width()?;
                    low = low.checked_sub(width)?;
                    self.collect_aligned_expression_dependencies(
                        block,
                        part,
                        Span {
                            start: low,
                            length: width,
                        },
                        dependencies,
                        dependency_cursor,
                        aligned,
                    )?;
                }
                (low == destination.start).then_some(())
            }
            Expression::Unary(op, operand, _)
                if matches!(op, Op::BitNot | Op::Add)
                    && operand.comptime().r#type.total_width()? == destination.length =>
            {
                self.collect_aligned_expression_dependencies(
                    block,
                    operand,
                    destination,
                    dependencies,
                    dependency_cursor,
                    aligned,
                )
            }
            Expression::Binary(left, op, right, _)
                if matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor)
                    && left.comptime().r#type.total_width()? == destination.length
                    && right.comptime().r#type.total_width()? == destination.length =>
            {
                self.collect_aligned_expression_dependencies(
                    block,
                    left,
                    destination,
                    dependencies,
                    dependency_cursor,
                    aligned,
                )?;
                self.collect_aligned_expression_dependencies(
                    block,
                    right,
                    destination,
                    dependencies,
                    dependency_cursor,
                    aligned,
                )
            }
            Expression::Binary(left, op, right, _)
                if matches!(op, Op::LogicShiftL | Op::ArithShiftL)
                    && left.comptime().r#type.total_width()? == destination.length =>
            {
                let shift = right
                    .clone()
                    .eval_value(&mut self.context)?
                    .to_usize()?
                    .min(destination.length);
                let mut shifted = Vec::new();
                self.collect_aligned_expression_dependencies(
                    block,
                    left,
                    Span {
                        start: 0,
                        length: destination.length,
                    },
                    dependencies,
                    dependency_cursor,
                    &mut shifted,
                )?;
                let live_source = Span {
                    start: 0,
                    length: destination.length.checked_sub(shift)?,
                };
                for dependency in shifted {
                    let Some(overlap) = dependency.destination.intersection(live_source) else {
                        continue;
                    };
                    let offset = overlap.start.checked_sub(dependency.destination.start)?;
                    aligned.push(AlignedDependency {
                        read: dependency.read,
                        kind: dependency.kind,
                        source: Span {
                            start: dependency.source.start.checked_add(offset)?,
                            length: overlap.length,
                        },
                        destination: Span {
                            start: destination
                                .start
                                .checked_add(overlap.start)?
                                .checked_add(shift)?,
                            length: overlap.length,
                        },
                    });
                }
                Some(())
            }
            _ => None,
        }
    }

    fn push_read(&mut self, block: usize, region: Region<VarId>) -> usize {
        let id = self.next_read;
        self.next_read += 1;
        self.procedure.events[block].push(Event::Read { id, region });
        id
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
        let Some(variable) = self.context.variables.get(&id).cloned() else {
            return Region::UnknownObject(id);
        };
        let Some(width) = variable.total_width() else {
            return Region::UnknownObject(id);
        };
        // `eval_value` maps an X/Z numeric index to zero for simulation
        // convenience. Alias analysis must not mistake that fallback for a
        // statically resolved address.
        if !index.is_const() || !select.is_const_with_range() {
            return Region::UnknownObject(id);
        }
        let Some(index_path) = index.eval_value(&mut self.context) else {
            return Region::UnknownObject(id);
        };
        let Some(array_index) = variable.r#type.array.calc_index(&index_path) else {
            return Region::UnknownObject(id);
        };
        let Some((high, low)) = select.eval_value(&mut self.context, &variable.r#type, false)
        else {
            return Region::UnknownObject(id);
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
        Region::Exact {
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
