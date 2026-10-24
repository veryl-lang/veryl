//! Coarse tier: a dependency graph on the regions a module accesses.
//!
//! A region is a variable, or the part of it one constant index and select
//! address. Each region has a write node and a read node. A statement adds an
//! edge from the read node of every region it reads, including the selectors
//! of its destinations and the conditions it runs under, to the write node of
//! every region it writes; the write node of a region reaches the read node
//! of every region of the same variable that overlaps it. Versions and path
//! conditions are ignored and dynamic accesses take every bit they may, so
//! every dependency of the precise graph maps onto a walk of this graph, and
//! every precise cycle onto a closed walk. A module whose coarse graph is
//! acyclic therefore has no combinational loop, and the precise tier need not
//! run on it.
//!
//! Conditions reach the writes under them through one control node per
//! scope, so each write takes one control edge. The graph has a node per
//! distinct access and scope, and an edge per pair of overlapping accesses
//! of a variable, which a sweep over their intervals finds.
//!
//! Constructs the precise tier adds no dependencies for either (black boxes,
//! `inout`, opaque reads outside functions, recursion, unsupported
//! statements, children without a summary) leave the module incomplete, as
//! they leave the precise tier. Positions beyond the precise tier's range
//! leave the module undecided instead: the precise tier reports them.

use super::region::{ArraySpan, PackedSpan, dst_writes, var_reads};
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, CaseArm, CasePattern, Component, Declaration, Expression,
    Factor, ForBound, ForRange, FunctionCall, InstDeclaration, MemberSelectDomain, Module, Op,
    Signature, Statement, SystemFunctionKind, VarId, VarIndex, VarKind, VarSelect,
};
use crate::symbol::Affiliation;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::kosaraju_scc;
use std::collections::BTreeSet;
use std::rc::Rc;

/// How much a coarse graph tells about its module.
#[derive(Clone, Copy, Default)]
struct Status {
    /// Some dependencies are unknown, as they are to the precise tier.
    incomplete: bool,
    /// The precise tier may know dependencies this graph does not.
    undecided: bool,
}

impl Status {
    fn merge(&mut self, other: Self) {
        self.incomplete |= other.incomplete;
        self.undecided |= other.undecided;
    }
}

/// What parents need from a module: which ports reach which.
#[derive(Default)]
pub(super) struct CoarseSummary {
    reach: HashMap<VarId, HashSet<VarId>>,
    status: Status,
}

pub(super) struct CoarseVerdict {
    acyclic: bool,
    status: Status,
}

impl CoarseVerdict {
    /// The module has no combinational loop the precise tier could find.
    pub(super) fn proves_acyclic(&self) -> bool {
        self.acyclic && !self.status.undecided
    }

    /// The module has no combinational loop at all, when it proves acyclic.
    pub(super) fn is_complete(&self) -> bool {
        !self.status.incomplete
    }
}

/// The bits one access may take: the whole variable, or a constant part.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Region {
    Whole,
    Part(ArraySpan, PackedSpan),
}

/// A region of a variable, by its index in [`Regions`].
type RegionId = (VarId, u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Node {
    Write(RegionId),
    Read(RegionId),
    /// What decides whether the writes of one scope run.
    Control(u32),
}

type Edges = HashMap<Node, HashSet<Node>>;
type Reads = HashSet<Node>;

/// Merge `from` into `into`, moving the larger set instead of copying it.
fn absorb(into: &mut Reads, mut from: Reads) {
    if into.len() < from.len() {
        std::mem::swap(into, &mut from);
    }
    into.extend(from);
}

/// The distinct regions a module accesses, per variable.
struct Regions {
    /// Evaluates constant indices and selects; built on the first access
    /// that has any.
    ctx: Option<Context>,
    regions: HashMap<VarId, Vec<Region>>,
    ids: HashMap<(VarId, Region), u32>,
    controls: u32,
}

impl Regions {
    fn new() -> Self {
        Self {
            ctx: None,
            regions: HashMap::default(),
            ids: HashMap::default(),
            controls: 0,
        }
    }

    fn intern(&mut self, id: VarId, region: Region) -> RegionId {
        let regions = self.regions.entry(id).or_default();
        let index = *self.ids.entry((id, region)).or_insert_with(|| {
            regions.push(region);
            (regions.len() - 1) as u32
        });
        (id, index)
    }

    fn region(&self, (id, index): RegionId) -> Region {
        self.regions[&id][index as usize]
    }

    fn control(&mut self) -> Node {
        self.controls += 1;
        Node::Control(self.controls)
    }
}

fn is_whole(index: &VarIndex, select: &VarSelect) -> bool {
    index.indices.is_empty() && index.range.is_none() && select.0.is_empty() && select.1.is_none()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Sink {
    Return,
    Argument(VarId),
    Write(Node),
}

/// What reaches each node a function body writes: its arguments, and the
/// read nodes of module variables it reads directly.
#[derive(Default)]
struct FunctionSummary {
    sinks: Vec<(Sink, Origins)>,
    status: Status,
}

/// What a function body's writes may depend on from outside it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Origin {
    Argument(VarId),
    Read(Node),
}

#[derive(Default)]
struct Origins {
    arguments: HashSet<VarId>,
    reads: HashSet<Node>,
}

type FunctionKey = (VarId, Option<usize>);

#[derive(Default)]
struct Functions {
    summaries: HashMap<FunctionKey, Rc<FunctionSummary>>,
    active: HashSet<FunctionKey>,
}

struct Builder<'a> {
    module: &'a Module,
    regions: &'a mut Regions,
    functions: &'a mut Functions,
    edges: Edges,
    reads: HashSet<RegionId>,
    writes: HashSet<RegionId>,
    /// Sources of an opaque read inside a function body: its arguments,
    /// which the precise tier reads for a recursive call.
    opaque_sources: Option<Reads>,
    /// Control nodes of the bodies around that may stop early. Every
    /// condition inside them decides whether their later writes run.
    early_exits: Vec<Node>,
    /// A function body read an opaque value somewhere.
    read_opaque: bool,
    status: Status,
}

impl<'a> Builder<'a> {
    fn new(module: &'a Module, regions: &'a mut Regions, functions: &'a mut Functions) -> Self {
        Self {
            module,
            regions,
            functions,
            edges: HashMap::default(),
            reads: HashSet::default(),
            writes: HashSet::default(),
            opaque_sources: None,
            early_exits: Vec::new(),
            read_opaque: false,
            status: Status::default(),
        }
    }

    fn context(&mut self) -> &mut Context {
        let module = self.module;
        self.regions
            .ctx
            .get_or_insert_with(|| super::module_context(module))
    }

    /// The region a read takes: its constant part, or the whole variable.
    fn region_of(
        &mut self,
        id: VarId,
        index: &VarIndex,
        select: &VarSelect,
        member_select_domain: Option<MemberSelectDomain>,
    ) -> Region {
        if is_whole(index, select) {
            return Region::Whole;
        }
        var_reads(id, index, select, member_select_domain, self.context())
            .into_iter()
            .next()
            .map_or(Region::Whole, |(array, packed)| Region::Part(array, packed))
    }

    fn read_node(&mut self, id: VarId, region: Region) -> Node {
        let region = self.regions.intern(id, region);
        self.reads.insert(region);
        Node::Read(region)
    }

    fn write_node(&mut self, id: VarId, region: Region) -> Node {
        let region = self.regions.intern(id, region);
        self.writes.insert(region);
        Node::Write(region)
    }

    fn add_edges(&mut self, sources: &Reads, destination: Node) {
        for &source in sources {
            self.edges.entry(source).or_default().insert(destination);
        }
    }

    /// The controls of a scope inside `controls` that also runs only as
    /// `condition` decides: one node, so its writes take one edge each.
    fn scope(&mut self, controls: &Reads, condition: &Reads) -> Reads {
        if condition.is_empty() {
            return controls.clone();
        }
        let control = self.regions.control();
        self.add_edges(controls, control);
        self.add_edges(condition, control);
        HashSet::from_iter([control])
    }

    /// A statement condition: it decides its own scope and, through the
    /// bodies around that may stop early, whether their later writes run.
    fn condition(&mut self, controls: &Reads, condition: &Reads) -> Reads {
        for index in 0..self.early_exits.len() {
            let exit = self.early_exits[index];
            self.add_edges(condition, exit);
        }
        self.scope(controls, condition)
    }

    /// Run `f` on a body that may stop early, under one control node that
    /// every condition inside the body reaches.
    fn early_exit(&mut self, controls: &Reads, f: impl FnOnce(&mut Self, &Reads)) {
        let exit = self.regions.control();
        self.add_edges(controls, exit);
        self.early_exits.push(exit);
        f(self, &HashSet::from_iter([exit]));
        self.early_exits.pop();
    }

    /// Run `f` on operands that run only as `guard` decides. Their control
    /// node is connected only if a write takes it, so operands without
    /// calls cost nothing.
    fn guarded(&mut self, guard: &Reads, controls: &Reads, f: impl FnOnce(&mut Self, &Reads)) {
        let control = self.regions.control();
        f(self, &HashSet::from_iter([control]));
        if self.edges.contains_key(&control) {
            self.add_edges(controls, control);
            self.add_edges(guard, control);
        }
    }

    fn write(&mut self, destination: &AssignDestination, sources: &Reads, controls: &Reads) {
        let mut sources = sources.clone();
        sources.extend(controls.iter().copied());
        self.index_reads(&destination.index, &mut sources, controls);
        self.select_reads(&destination.select, &mut sources, controls);
        let region = if is_whole(&destination.index, &destination.select) {
            Region::Whole
        } else {
            dst_writes(destination, self.context())
                .into_iter()
                .next()
                .map_or(Region::Whole, |(array, packed)| Region::Part(array, packed))
        };
        let node = self.write_node(destination.id, region);
        self.add_edges(&sources, node);
    }

    fn write_whole(&mut self, id: VarId, sources: &Reads) {
        let node = self.write_node(id, Region::Whole);
        self.add_edges(sources, node);
    }

    fn index_reads(&mut self, index: &VarIndex, reads: &mut Reads, controls: &Reads) {
        for expression in index.expressions() {
            self.expression(expression, reads, controls);
        }
    }

    fn select_reads(&mut self, select: &VarSelect, reads: &mut Reads, controls: &Reads) {
        for expression in &select.0 {
            self.expression(expression, reads, controls);
        }
        if let Some((_, expression)) = &select.1 {
            self.expression(expression, reads, controls);
        }
    }

    /// Collect the nodes an expression's value reads into `reads`, and add
    /// the edges of the writes its calls make under `controls`.
    fn expression(&mut self, expression: &Expression, reads: &mut Reads, controls: &Reads) {
        match expression {
            Expression::Term(factor) => self.factor(factor, reads, controls),
            Expression::Unary(_, operand, _) => self.expression(operand, reads, controls),
            // The left operand of a short-circuit operator decides whether
            // calls in the right one run.
            Expression::Binary(left, Op::LogicAnd | Op::LogicOr, right, _) => {
                let mut guard = HashSet::default();
                self.expression(left, &mut guard, controls);
                self.guarded(&guard, controls, |this, inner| {
                    this.expression(right, reads, inner)
                });
                absorb(reads, guard);
            }
            Expression::Binary(left, _, right, _) => {
                self.expression(left, reads, controls);
                self.expression(right, reads, controls);
            }
            Expression::Ternary(condition, then, otherwise, _) => {
                let mut guard = HashSet::default();
                self.expression(condition, &mut guard, controls);
                self.guarded(&guard, controls, |this, inner| {
                    this.expression(then, reads, inner);
                    this.expression(otherwise, reads, inner);
                });
                absorb(reads, guard);
            }
            Expression::Concatenation(items, _) => {
                for (item, repeat) in items {
                    self.expression(item, reads, controls);
                    if let Some(repeat) = repeat {
                        self.expression(repeat, reads, controls);
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(item, repeat) => {
                            self.expression(item, reads, controls);
                            if let Some(repeat) = repeat {
                                self.expression(repeat, reads, controls);
                            }
                        }
                        ArrayLiteralItem::Defaul(item) => self.expression(item, reads, controls),
                    }
                }
            }
            Expression::StructConstructor(_, members, _) => {
                for (_, member) in members {
                    self.expression(member, reads, controls);
                }
            }
        }
    }

    fn factor(&mut self, factor: &Factor, reads: &mut Reads, controls: &Reads) {
        match factor {
            Factor::Variable(id, index, select, comptime) => {
                let region = self.region_of(*id, index, select, comptime.member_select_domain);
                let node = self.read_node(*id, region);
                reads.insert(node);
                self.index_reads(index, reads, controls);
                self.select_reads(select, reads, controls);
            }
            // The variable inside the child is not followed, by the precise
            // tier either, but its selectors are read here.
            Factor::HierVariable(reference) => {
                self.index_reads(&reference.index, reads, controls);
                self.select_reads(&reference.select, reads, controls);
            }
            Factor::FunctionCall(call) => {
                let result = self.call(call, controls);
                absorb(reads, result);
            }
            Factor::SystemFunctionCall(call) => self.system_call(&call.kind, reads, controls),
            Factor::Unknown(_) => match &self.opaque_sources {
                Some(sources) => {
                    reads.extend(sources.iter().copied());
                    self.read_opaque = true;
                }
                None => self.status.incomplete = true,
            },
            Factor::Value(_) | Factor::ResetValue(..) | Factor::Anonymous(_) => {}
        }
    }

    fn system_call(&mut self, kind: &SystemFunctionKind, reads: &mut Reads, controls: &Reads) {
        let mut discarded = HashSet::default();
        match kind {
            SystemFunctionKind::Onehot(input)
            | SystemFunctionKind::Signed(input)
            | SystemFunctionKind::Unsigned(input) => self.expression(&input.0, reads, controls),
            SystemFunctionKind::Readmemh(input, output) => {
                let mut sources = HashSet::default();
                self.expression(&input.0, &mut sources, controls);
                for destination in output.local() {
                    self.write(destination, &sources, controls);
                }
            }
            SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
                for input in inputs {
                    self.expression(&input.0, &mut discarded, controls);
                }
            }
            SystemFunctionKind::Assert { cond, args, .. } => {
                self.expression(&cond.0, &mut discarded, controls);
                for input in args {
                    self.expression(&input.0, &mut discarded, controls);
                }
            }
            SystemFunctionKind::Bits(_)
            | SystemFunctionKind::Size(..)
            | SystemFunctionKind::Clog2(_)
            | SystemFunctionKind::Finish => {}
        }
    }

    /// Add the edges of a call's writes, and return what its result reads.
    fn call(&mut self, call: &FunctionCall, controls: &Reads) -> Reads {
        let module = self.module;
        let body = self
            .body_index(call)
            .map(|index| &module.functions[&call.id].functions[index]);
        let mut argument_reads: HashMap<VarId, Reads> = HashMap::default();
        let mut all_reads = HashSet::default();
        for (path, input) in &call.inputs {
            let mut reads = HashSet::default();
            self.expression(input, &mut reads, controls);
            if let Some(id) = body.and_then(|body| body.arg_map.get(path)) {
                argument_reads
                    .entry(*id)
                    .or_default()
                    .extend(reads.iter().copied());
            }
            all_reads.extend(reads);
        }

        let summary = self.function_summary(call);
        let (Some(body), Some(summary)) = (body, summary) else {
            // Without a summary, every output and the result depend on every
            // input, as in the precise tier. A recursive call also leaves
            // the precise tier incomplete; a missing body does not.
            if body.is_some() {
                self.status.incomplete = true;
            }
            for (_, destinations) in &call.outputs {
                for destination in destinations {
                    self.write(destination, &all_reads, controls);
                }
            }
            return all_reads;
        };
        self.status.merge(summary.status);

        let mut result = HashSet::default();
        let mut output_sources: HashMap<VarId, Reads> = HashMap::default();
        for (sink, origins) in &summary.sinks {
            let mut sources = origins.reads.clone();
            for &node in &origins.reads {
                if let Node::Read(region) = node {
                    self.reads.insert(region);
                }
            }
            for argument in &origins.arguments {
                if let Some(reads) = argument_reads.get(argument) {
                    sources.extend(reads.iter().copied());
                }
            }
            match *sink {
                Sink::Return => absorb(&mut result, sources),
                Sink::Argument(id) => output_sources.entry(id).or_default().extend(sources),
                Sink::Write(node) => {
                    let mut sources = sources;
                    sources.extend(controls.iter().copied());
                    if let Node::Write(region) = node {
                        self.writes.insert(region);
                    }
                    self.add_edges(&sources, node);
                }
            }
        }
        // Every binding is written, if only through its selectors.
        for (path, destinations) in &call.outputs {
            let sources = body
                .arg_map
                .get(path)
                .and_then(|id| output_sources.get(id))
                .cloned()
                .unwrap_or_default();
            for destination in destinations {
                self.write(destination, &sources, controls);
            }
        }
        result
    }

    fn is_module_variable(&self, id: VarId) -> bool {
        match self.module.variables.get(&id) {
            Some(variable) => variable.affiliation != Affiliation::Function,
            None => self.module.interface_members.contains_key(&id),
        }
    }

    /// The body a call takes, chosen as the precise tier chooses it.
    fn body_index(&self, call: &FunctionCall) -> Option<usize> {
        let function = self.module.functions.get(&call.id)?;
        let index = function
            .array
            .calc_index(call.index.as_deref().unwrap_or_default())?;
        (index < function.functions.len()).then_some(index)
    }

    /// The summary of the body a call takes, or `None` when the body is
    /// missing or the call is recursive. Every summary is kept, so each body
    /// is walked once.
    fn function_summary(&mut self, call: &FunctionCall) -> Option<Rc<FunctionSummary>> {
        let index = self.body_index(call)?;
        let key = (call.id, Some(index));
        if let Some(summary) = self.functions.summaries.get(&key) {
            return Some(summary.clone());
        }
        if !self.functions.active.insert(key) {
            return None;
        }
        let module = self.module;
        let function = &module.functions[&call.id];
        let mut summary = FunctionSummary::default();
        {
            let body = &function.functions[index];
            let arguments = body.arg_map.values().copied().collect::<HashSet<_>>();
            let mut inner = Builder::new(self.module, self.regions, self.functions);
            inner.opaque_sources = Some(
                arguments
                    .iter()
                    .map(|&id| inner.read_node(id, Region::Whole))
                    .collect(),
            );
            // A return or break skips the statements after it, so whatever
            // decides it controls everything the body writes.
            let none = HashSet::default();
            if statements_break(&body.statements) {
                inner.early_exit(&none, |inner, controls| {
                    inner.statements(&body.statements, controls)
                });
            } else {
                inner.statements(&body.statements, &none);
            }
            let written = inner.writes.iter().copied().collect::<Vec<_>>();
            let read_opaque = inner.read_opaque;
            let (edges, status) = inner.finish();
            summary.status.merge(status);
            // An opaque read anywhere in the body makes the result read every
            // argument, as the precise tier makes it.
            if read_opaque {
                summary.sinks.push((
                    Sink::Return,
                    Origins {
                        arguments: arguments.clone(),
                        reads: HashSet::default(),
                    },
                ));
            }

            // Origins are the arguments and the module variables read; sinks
            // are the result, the arguments and the module variables written.
            let mut origins: Vec<Origin> = Vec::new();
            let mut bits: HashMap<Origin, usize> = HashMap::default();
            let origin = |node: Node| match node {
                Node::Read((id, _)) if arguments.contains(&id) => Some(Origin::Argument(id)),
                Node::Read((id, _)) if self.is_module_variable(id) => Some(Origin::Read(node)),
                _ => None,
            };
            for node in edges.keys() {
                if let Some(key) = origin(*node) {
                    bits.entry(key).or_insert_with(|| {
                        origins.push(key);
                        origins.len() - 1
                    });
                }
            }
            let sink = |region: RegionId| {
                let (id, _) = region;
                if Some(id) == body.ret {
                    Some(Sink::Return)
                } else if arguments.contains(&id) {
                    Some(Sink::Argument(id))
                } else if self.is_module_variable(id) {
                    Some(Sink::Write(Node::Write(region)))
                } else {
                    None
                }
            };
            let graph = Condensed::new(&edges);
            let reached = graph.sources_reaching(
                // An origin without outgoing edges reaches nothing.
                &|node| origin(node).and_then(|key| bits.get(&key).copied()),
                origins.len(),
                &|node| matches!(node, Node::Write(region) if sink(region).is_some()),
            );
            for region in written {
                let Some(sink) = sink(region) else {
                    continue;
                };
                let mut found = Origins::default();
                if let Some(set) = graph
                    .index
                    .get(&Node::Write(region))
                    .and_then(|&node| reached[graph.component[node]].as_ref())
                {
                    for bit in set_bits(set) {
                        match origins[bit] {
                            Origin::Argument(id) => {
                                found.arguments.insert(id);
                            }
                            Origin::Read(node) => {
                                found.reads.insert(node);
                            }
                        }
                    }
                }
                summary.sinks.push((sink, found));
            }
        }
        self.functions.active.remove(&key);
        let summary = Rc::new(summary);
        self.functions.summaries.insert(key, summary.clone());
        Some(summary)
    }

    fn statements(&mut self, statements: &[Statement], controls: &Reads) {
        for statement in statements {
            match statement {
                Statement::Assign(assign) => {
                    let mut sources = HashSet::default();
                    self.expression(&assign.expr, &mut sources, controls);
                    for destination in &assign.dst {
                        self.write(destination, &sources, controls);
                    }
                }
                Statement::If(statement) => {
                    let mut condition = HashSet::default();
                    self.expression(&statement.cond, &mut condition, controls);
                    let inner = self.condition(controls, &condition);
                    self.statements(&statement.true_side, &inner);
                    self.statements(&statement.false_side, &inner);
                }
                Statement::Case(statement) => {
                    let mut condition = HashSet::default();
                    self.case_reads(
                        &statement.case_target,
                        &statement.arms,
                        &mut condition,
                        controls,
                    );
                    let inner = self.condition(controls, &condition);
                    for arm in &statement.arms {
                        self.statements(&arm.body, &inner);
                    }
                    self.statements(&statement.default, &inner);
                }
                Statement::For(statement) => {
                    let mut condition = HashSet::default();
                    for range in statement.ranges() {
                        self.range_reads(range, &mut condition, controls);
                    }
                    let inner = self.condition(controls, &condition);
                    for iterator in &statement.iterators {
                        self.write_whole(iterator.var_id, &inner);
                    }
                    if statements_break(&statement.body) {
                        self.early_exit(&inner, |this, inner| {
                            this.statements(&statement.body, inner)
                        });
                    } else {
                        self.statements(&statement.body, &inner);
                    }
                }
                Statement::SystemFunctionCall(call) => {
                    let mut discarded = HashSet::default();
                    self.system_call(&call.kind, &mut discarded, controls);
                }
                Statement::FunctionCall(call) => {
                    self.call(call, controls);
                }
                Statement::Unsupported(_) => self.status.incomplete = true,
                Statement::IfReset(_)
                | Statement::TbMethodCall(_)
                | Statement::Break
                | Statement::Null => {}
            }
        }
    }

    /// Read a case target and its patterns, whose calls run under `controls`.
    fn case_reads(
        &mut self,
        target: &Expression,
        arms: &[CaseArm],
        reads: &mut Reads,
        controls: &Reads,
    ) {
        self.expression(target, reads, controls);
        for arm in arms {
            for pattern in &arm.patterns {
                match pattern {
                    CasePattern::Eq(value) => self.expression(value, reads, controls),
                    CasePattern::Range { lo, hi, .. } => {
                        self.expression(lo, reads, controls);
                        self.expression(hi, reads, controls);
                    }
                }
            }
        }
    }

    /// Read the bounds of a loop, whose calls run under `controls`.
    fn range_reads(&mut self, range: &ForRange, reads: &mut Reads, controls: &Reads) {
        let (start, end) = range.bounds();
        for bound in [start, end] {
            if let ForBound::Expression(expression) = bound {
                self.expression(expression, reads, controls);
            }
        }
    }

    fn instance(&mut self, inst: &InstDeclaration, summary: &CoarseSummary) {
        let none = HashSet::default();
        let mut sources: HashMap<VarId, Reads> = HashMap::default();
        let mut destinations: HashMap<VarId, Vec<&AssignDestination>> = HashMap::default();
        let mut bound: HashMap<VarId, Vec<Node>> = HashMap::default();
        for input in &inst.inputs {
            let mut reads = HashSet::default();
            self.expression(&input.expr, &mut reads, &none);
            sources.entry(input.id).or_default().extend(reads);
        }
        for output in &inst.outputs {
            destinations
                .entry(output.id)
                .or_default()
                .extend(output.dst.iter());
            // A port drives its destinations even when nothing in the child
            // reaches it.
            for destination in &output.dst {
                self.write(destination, &none, &none);
            }
        }
        for binding in &inst.interface_bindings {
            let region = self.region_of(binding.parent, &binding.index, &binding.select, None);
            let mut reads = HashSet::from_iter([self.read_node(binding.parent, region)]);
            self.index_reads(&binding.index, &mut reads, &none);
            self.select_reads(&binding.select, &mut reads, &none);
            sources.entry(binding.child).or_default().extend(reads);
            let node = self.write_node(binding.parent, region);
            bound.entry(binding.child).or_default().push(node);
        }
        for (port, reads) in &sources {
            for reached in summary.reach.get(port).into_iter().flatten() {
                for destination in destinations.get(reached).into_iter().flatten() {
                    self.write(destination, reads, &none);
                }
                for &node in bound.get(reached).into_iter().flatten() {
                    self.add_edges(reads, node);
                }
            }
        }
    }

    /// Connect each written region to the regions of its variable that read
    /// any of its bits, and return the edges and what they tell.
    fn finish(mut self) -> (Edges, Status) {
        let mut accesses: HashMap<VarId, [Vec<RegionId>; 2]> = HashMap::default();
        for &region in &self.writes {
            accesses.entry(region.0).or_default()[0].push(region);
        }
        for &region in &self.reads {
            accesses.entry(region.0).or_default()[1].push(region);
        }
        for [writes, reads] in accesses.values() {
            for (write, read) in overlapping(self.regions, writes, reads) {
                self.edges
                    .entry(Node::Write(write))
                    .or_default()
                    .insert(Node::Read(read));
            }
        }
        (self.edges, self.status)
    }
}

/// The parts of one side of a variable, by the elements they take, with
/// their packed intervals.
type PartGroups = HashMap<ArraySpan, Vec<(RegionId, usize, usize)>>;

/// The pairs of a variable's written and read regions that share a bit. A
/// whole region shares one with every region. Parts are grouped by the
/// elements they take: groups whose elements overlap are found by a sweep
/// along the array dimension, and the parts of each such pair of groups by a
/// sweep along the packed one.
fn overlapping(
    regions: &Regions,
    writes: &[RegionId],
    reads: &[RegionId],
) -> Vec<(RegionId, RegionId)> {
    let mut pairs = Vec::new();
    let whole = |access: RegionId| regions.region(access) == Region::Whole;
    // The parts of each side, by the elements they take.
    let mut groups: [PartGroups; 2] = [HashMap::default(), HashMap::default()];
    for &write in writes {
        match regions.region(write) {
            Region::Whole => pairs.extend(reads.iter().map(|&read| (write, read))),
            Region::Part(array, packed) => {
                groups[0]
                    .entry(array)
                    .or_default()
                    .push((write, packed.start, packed.end()))
            }
        }
    }
    for &read in reads {
        match regions.region(read) {
            // The whole write met it above.
            Region::Whole => pairs.extend(
                writes
                    .iter()
                    .filter(|&&write| !whole(write))
                    .map(|&write| (write, read)),
            ),
            Region::Part(array, packed) => {
                groups[1]
                    .entry(array)
                    .or_default()
                    .push((read, packed.start, packed.end()))
            }
        }
    }
    // Array spans end within the variable, which `position_overflow` bounds.
    let spans = |groups: &PartGroups| {
        groups
            .keys()
            .map(|&array| {
                let end = array.end().expect("array spans end within isize::MAX");
                (array, array.start, end)
            })
            .collect::<Vec<_>>()
    };
    for (written, read) in intersecting(&spans(&groups[0]), &spans(&groups[1])) {
        pairs.extend(intersecting(&groups[0][&written], &groups[1][&read]));
    }
    pairs
}

/// The pairs of a left and a right item whose half-open intervals
/// intersect, by one sweep in the order of their starts.
fn intersecting<L: Copy, R: Copy>(
    left: &[(L, usize, usize)],
    right: &[(R, usize, usize)],
) -> Vec<(L, R)> {
    let mut events = left
        .iter()
        .enumerate()
        .map(|(index, &(_, start, _))| (start, false, index))
        .chain(
            right
                .iter()
                .enumerate()
                .map(|(index, &(_, start, _))| (start, true, index)),
        )
        .collect::<Vec<_>>();
    events.sort_unstable();
    // The items whose interval holds the current start, by end, per side.
    let mut active: [BTreeSet<(usize, usize)>; 2] = [BTreeSet::new(), BTreeSet::new()];
    let mut pairs = Vec::new();
    for (start, is_right, index) in events {
        for set in &mut active {
            while set.first().is_some_and(|&(end, _)| end <= start) {
                set.pop_first();
            }
        }
        if is_right {
            pairs.extend(
                active[0]
                    .iter()
                    .map(|&(_, other)| (left[other].0, right[index].0)),
            );
            active[1].insert((right[index].2, index));
        } else {
            pairs.extend(
                active[1]
                    .iter()
                    .map(|&(_, other)| (left[index].0, right[other].0)),
            );
            active[0].insert((left[index].2, index));
        }
    }
    pairs
}

/// Whether `statements` may stop before their end, outside nested loops.
fn statements_break(statements: &[Statement]) -> bool {
    statements.iter().any(|statement| match statement {
        Statement::Break => true,
        Statement::If(statement) => {
            statements_break(&statement.true_side) || statements_break(&statement.false_side)
        }
        Statement::Case(statement) => {
            statement.arms.iter().any(|arm| statements_break(&arm.body))
                || statements_break(&statement.default)
        }
        _ => false,
    })
}

/// Build the coarse graph of `module` on the summaries of its children.
pub(super) fn analyze(
    module: &Module,
    summaries: &HashMap<Signature, CoarseSummary>,
) -> (CoarseVerdict, CoarseSummary) {
    // Positions beyond the precise tier's range are left to it, which
    // reports them. Every span here is within that range.
    if super::position_overflow(module).is_some() {
        let status = Status {
            incomplete: true,
            undecided: true,
        };
        let verdict = CoarseVerdict {
            acyclic: false,
            status,
        };
        let summary = CoarseSummary {
            reach: HashMap::default(),
            status,
        };
        return (verdict, summary);
    }
    let mut regions = Regions::new();
    let mut functions = Functions::default();
    let mut builder = Builder::new(module, &mut regions, &mut functions);
    builder.status.incomplete = module
        .variables
        .values()
        .any(|variable| variable.kind == VarKind::Inout);
    let none = HashSet::default();
    for declaration in &module.declarations {
        match declaration {
            Declaration::Comb(comb) => builder.statements(&comb.statements, &none),
            Declaration::Inst(inst) => match inst.component.as_ref() {
                Component::Module(child) => match summaries.get(&child.signature) {
                    // A child the precise tier decides itself, or cannot represent,
                    // adds no more to its parents there than its coarse summary
                    // does here: it leaves them incomplete at most.
                    Some(summary) => {
                        builder.status.incomplete |=
                            summary.status.incomplete || summary.status.undecided;
                        builder.instance(inst, summary);
                    }
                    None => builder.status.incomplete = true,
                },
                Component::SystemVerilog(_) => builder.status.incomplete = true,
                // Interface signals are already lifted into the parent.
                Component::Interface(_) => {}
            },
            // The precise tier reads none of these.
            Declaration::Ff(_)
            | Declaration::External(_)
            | Declaration::Initial(_)
            | Declaration::Final(_)
            | Declaration::Unsupported(_)
            | Declaration::Null => {}
        }
    }

    let (edges, status) = builder.finish();
    let graph = Condensed::new(&edges);
    let verdict = CoarseVerdict {
        acyclic: graph.is_acyclic(),
        status,
    };
    let summary = CoarseSummary {
        reach: graph.port_reach(module),
        status,
    };
    (verdict, summary)
}

/// The bits set in a bit set.
fn set_bits(set: &[u64]) -> impl Iterator<Item = usize> + '_ {
    set.iter().enumerate().flat_map(|(index, &word)| {
        let mut word = word;
        std::iter::from_fn(move || {
            (word != 0).then(|| {
                let offset = word.trailing_zeros() as usize;
                word &= word - 1;
                index * 64 + offset
            })
        })
    })
}

/// The graph on dense indices, with its strongly connected components.
struct Condensed {
    nodes: Vec<Node>,
    index: HashMap<Node, usize>,
    successors: Vec<Vec<usize>>,
    /// The component of each node.
    component: Vec<usize>,
    /// Components each after every component it reaches.
    components: Vec<Vec<usize>>,
}

impl Condensed {
    fn new(edges: &Edges) -> Self {
        let mut graph = Graph::<Node, ()>::new();
        let mut index = HashMap::default();
        let mut add = |graph: &mut Graph<Node, ()>, node: Node| {
            *index.entry(node).or_insert_with(|| graph.add_node(node))
        };
        for (&source, destinations) in edges {
            let source = add(&mut graph, source);
            for &destination in destinations {
                let destination = add(&mut graph, destination);
                graph.add_edge(source, destination, ());
            }
        }
        let components = kosaraju_scc(&graph)
            .into_iter()
            .map(|members| members.into_iter().map(|node| node.index()).collect())
            .collect::<Vec<Vec<usize>>>();
        let mut component = vec![0; graph.node_count()];
        for (index, members) in components.iter().enumerate() {
            for &member in members {
                component[member] = index;
            }
        }
        let successors = graph
            .node_indices()
            .map(|node| graph.neighbors(node).map(|next| next.index()).collect())
            .collect();
        Self {
            nodes: graph.node_weights().copied().collect(),
            index: index
                .into_iter()
                .map(|(node, graph_index)| (node, graph_index.index()))
                .collect(),
            successors,
            component,
            components,
        }
    }

    fn is_acyclic(&self) -> bool {
        self.components.iter().all(|members| {
            let [node] = members.as_slice() else {
                return false;
            };
            !self.successors[*node].contains(node)
        })
    }

    /// For each component a source reaches and that reaches a target, the
    /// bits of the sources that reach it; `source` gives the bit of each
    /// source node. Sets propagate along the components in topological
    /// order, so each edge is taken once.
    fn sources_reaching(
        &self,
        source: &dyn Fn(Node) -> Option<usize>,
        bits: usize,
        target: &dyn Fn(Node) -> bool,
    ) -> Vec<Option<Vec<u64>>> {
        let count = self.components.len();
        let mut reaches_target = vec![false; count];
        for (component, members) in self.components.iter().enumerate() {
            reaches_target[component] = members.iter().any(|&node| {
                target(self.nodes[node])
                    || self.successors[node]
                        .iter()
                        .any(|&next| reaches_target[self.component[next]])
            });
        }
        let words = bits.div_ceil(64);
        let mut sets: Vec<Option<Vec<u64>>> = vec![None; count];
        for (node, &kind) in self.nodes.iter().enumerate() {
            let component = self.component[node];
            if let Some(bit) = source(kind)
                && reaches_target[component]
            {
                let set = sets[component].get_or_insert_with(|| vec![0; words]);
                set[bit / 64] |= 1 << (bit % 64);
            }
        }
        // Components close each after those it reaches, so the reverse order
        // takes every component after those that reach it.
        for component in (0..count).rev() {
            let Some(set) = sets[component].take() else {
                continue;
            };
            for &node in &self.components[component] {
                for &next in &self.successors[node] {
                    let next = self.component[next];
                    if next != component && reaches_target[next] {
                        let other = sets[next].get_or_insert_with(|| vec![0; words]);
                        for (word, set) in other.iter_mut().zip(&set) {
                            *word |= set;
                        }
                    }
                }
            }
            sets[component] = Some(set);
        }
        sets
    }

    /// The ports and interface members each port reaches: from a read of
    /// the first to a write of the second.
    fn port_reach(&self, module: &Module) -> HashMap<VarId, HashSet<VarId>> {
        let ports = module
            .ports
            .values()
            .copied()
            .chain(
                module
                    .variables
                    .values()
                    .filter(|variable| {
                        matches!(
                            variable.kind,
                            VarKind::Input | VarKind::Output | VarKind::Inout
                        )
                    })
                    .map(|variable| variable.id),
            )
            .chain(module.interface_members.keys().copied())
            .collect::<HashSet<_>>();
        let mut read: Vec<VarId> = Vec::new();
        let mut bit: HashMap<VarId, usize> = HashMap::default();
        for &node in &self.nodes {
            if let Node::Read((id, _)) = node
                && ports.contains(&id)
            {
                bit.entry(id).or_insert_with(|| {
                    read.push(id);
                    read.len() - 1
                });
            }
        }
        let sets = self.sources_reaching(
            &|node| match node {
                Node::Read((id, _)) => bit.get(&id).copied(),
                _ => None,
            },
            read.len(),
            &|node| matches!(node, Node::Write((id, _)) if ports.contains(&id)),
        );
        let mut reach: HashMap<VarId, HashSet<VarId>> = HashMap::default();
        for (node, &component) in self.nodes.iter().zip(&self.component) {
            let Node::Write((written, _)) = *node else {
                continue;
            };
            if !ports.contains(&written) {
                continue;
            }
            for bit in sets[component].iter().flat_map(|set| set_bits(set)) {
                reach.entry(read[bit]).or_default().insert(written);
            }
        }
        reach
    }
}
