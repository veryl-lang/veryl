//! Hierarchy templates: a component instantiated many times inside one top
//! has its comb logic scheduled and compiled once, and every instance runs
//! the shared code shifted by its storage delta.
//!
//! The template boundary is de-aliased, so an instance's comb statements touch
//! only its own storage and relocate by one `(ff, comb)` delta.  The parent
//! cannot order an instance as a single block (an output may depend on an
//! input that the parent computes from another output), so the template's
//! statements are split into classes by how many template exits the parent
//! needs before each statement's inputs are final, per instance.  Statements
//! with the same label vector across all instances form one class; ordering
//! classes by that label keeps the parent's block graph acyclic.

use crate::backend::registry::compile_chunks_parallel;
use crate::backend::{ChunkArtifact, CompileCtx};
use crate::ir::big_array::BigArrayFold;
use crate::ir::context::Context;
use crate::ir::statement::{CompiledBlockStatement, ProtoStatement};
use crate::ir::variable::VarOffset;
use crate::{HashMap, HashSet};
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use std::sync::{Arc, Mutex};
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;

/// What a class's statements read and write, in template coordinates, for the
/// parent's analyses to take in place of walking every instance's copy of
/// them.  Each view matches the union its walk over the copies would give.
pub(crate) struct HierSummary {
    expanded_ins: Vec<VarOffset>,
    expanded_outs: Vec<VarOffset>,
    big_arrays: Vec<(bool, isize, isize)>,
    dynamic_reads: Vec<(bool, isize, isize, usize)>,
    dynamic_writes: Vec<(VarOffset, isize, usize)>,
    pub(crate) mass: usize,
    dep_ins: Vec<crate::ir::deps::Dep>,
    dep_outs: Vec<crate::ir::deps::Dep>,
    range_reads: Vec<crate::ir::opt::event_gate::Range>,
    range_writes: Vec<crate::ir::opt::event_gate::Range>,
    pub(crate) write_spans: Vec<(isize, isize)>,
    pub(crate) dynamic_spans: Vec<(isize, isize)>,
}

fn sorted_dedup<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort_unstable();
    v.dedup();
    v
}

fn offset_key(o: VarOffset) -> (bool, isize) {
    (o.is_ff(), o.raw())
}

fn dedup_offsets(mut v: Vec<VarOffset>) -> Vec<VarOffset> {
    v.sort_unstable_by_key(|&o| offset_key(o));
    v.dedup();
    v
}

impl HierSummary {
    pub(crate) fn new(stmts: &[ProtoStatement]) -> Self {
        let none = BigArrayFold::default();
        let (mut ins, mut outs) = (Vec::new(), Vec::new());
        let mut arrays = BigArrayFold::default();
        let (mut dyn_reads, mut dyn_writes) = (Vec::new(), Vec::new());
        let mut deps = crate::ir::deps::StmtDeps::default();
        let (mut r_reads, mut r_writes) = (Vec::new(), Vec::new());
        let (mut base_ins, mut base_outs) = (Vec::new(), Vec::new());
        let mut mass = 0;
        for st in stmts {
            st.gather_variable_offsets_expanded(&none, &mut ins, &mut outs);
            st.collect_big_arrays(&mut arrays);
            st.gather_dynamic_read_ranges(&mut dyn_reads);
            st.gather_dynamic_write_spans(&mut dyn_writes);
            crate::ir::deps::collect_stmt_deps(st, &mut deps);
            crate::ir::opt::event_gate::stmt_ranges(st, &mut r_reads, &mut r_writes);
            st.gather_variable_offsets(&mut base_ins, &mut base_outs);
            mass += st.statement_mass();
        }
        let mut write_spans = Vec::new();
        let mut dynamic_spans = Vec::new();
        crate::ir::comb_layout::collect_write_spans(stmts, &mut write_spans);
        crate::ir::comb_layout::collect_dynamic_spans(stmts, &mut dynamic_spans);
        let dep_key = |d: &crate::ir::deps::Dep| (offset_key(d.off), d.bytes);
        let dedup_deps = |mut v: Vec<crate::ir::deps::Dep>| {
            v.sort_unstable_by_key(dep_key);
            v.dedup_by_key(|d| dep_key(d));
            v
        };
        Self {
            expanded_ins: dedup_offsets(ins),
            expanded_outs: dedup_offsets(outs),
            big_arrays: sorted_dedup(arrays.recorded().to_vec()),
            dynamic_reads: sorted_dedup(dyn_reads),
            dynamic_writes: {
                let mut v = dyn_writes;
                v.sort_unstable_by_key(|&(o, stride, n)| (offset_key(o), stride, n));
                v.dedup();
                v
            },
            mass,
            dep_ins: dedup_deps(deps.ins),
            dep_outs: dedup_deps(deps.outs),
            range_reads: sorted_dedup(r_reads),
            range_writes: sorted_dedup(r_writes),
            write_spans: sorted_dedup(write_spans),
            dynamic_spans: sorted_dedup(dynamic_spans),
        }
    }

    fn shift(cb: &CompiledBlockStatement, off: VarOffset) -> VarOffset {
        off.adjust(cb.ff_delta_bytes, cb.comb_delta_bytes)
    }

    pub(crate) fn gather_expanded(
        &self,
        cb: &CompiledBlockStatement,
        fold: &BigArrayFold,
        inputs: &mut Vec<VarOffset>,
        outputs: &mut Vec<VarOffset>,
    ) {
        let (i0, o0) = (inputs.len(), outputs.len());
        inputs.extend(
            self.expanded_ins
                .iter()
                .map(|&o| fold.canon(Self::shift(cb, o))),
        );
        outputs.extend(
            self.expanded_outs
                .iter()
                .map(|&o| fold.canon(Self::shift(cb, o))),
        );
        #[cfg(debug_assertions)]
        {
            let (mut wi, mut wo) = (Vec::new(), Vec::new());
            for st in cb.originals().iter() {
                st.gather_variable_offsets_expanded(fold, &mut wi, &mut wo);
            }
            debug_assert_eq!(
                dedup_offsets(inputs[i0..].to_vec()),
                dedup_offsets(wi),
                "summary reads differ from the walk"
            );
            debug_assert_eq!(
                dedup_offsets(outputs[o0..].to_vec()),
                dedup_offsets(wo),
                "summary writes differ from the walk"
            );
        }
        let _ = (i0, o0);
    }

    /// The shifted views against the same walks over `cb`'s copy.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_check(&self, cb: &CompiledBlockStatement) {
        let walked = Self::new(&cb.originals());
        let shifted =
            |v: &[VarOffset]| dedup_offsets(v.iter().map(|&o| Self::shift(cb, o)).collect());
        debug_assert_eq!(shifted(&self.expanded_ins), walked.expanded_ins);
        debug_assert_eq!(shifted(&self.expanded_outs), walked.expanded_outs);
        let mut fold = BigArrayFold::default();
        self.collect_big_arrays(cb, &mut fold);
        debug_assert_eq!(sorted_dedup(fold.recorded().to_vec()), walked.big_arrays);
        let mut r = Vec::new();
        self.dynamic_reads(cb, &mut r);
        debug_assert_eq!(sorted_dedup(r), walked.dynamic_reads);
        let mut deps = crate::ir::deps::StmtDeps::default();
        self.deps(cb, &mut deps);
        let key = |v: &[crate::ir::deps::Dep]| {
            sorted_dedup(
                v.iter()
                    .map(|d| (offset_key(d.off), d.bytes))
                    .collect::<Vec<_>>(),
            )
        };
        debug_assert_eq!(key(&deps.ins), key(&walked.dep_ins));
        debug_assert_eq!(key(&deps.outs), key(&walked.dep_outs));
        let (mut rr, mut rw) = (Vec::new(), Vec::new());
        self.ranges(cb, &mut rr, &mut rw);
        debug_assert_eq!(sorted_dedup(rr), walked.range_reads);
        debug_assert_eq!(sorted_dedup(rw), walked.range_writes);
        let mut ws = Vec::new();
        self.spans(&self.write_spans, cb, &mut ws);
        debug_assert_eq!(sorted_dedup(ws), walked.write_spans);
        let mut ds = Vec::new();
        self.spans(&self.dynamic_spans, cb, &mut ds);
        debug_assert_eq!(sorted_dedup(ds), walked.dynamic_spans);
        debug_assert_eq!(self.mass, walked.mass);
    }

    pub(crate) fn collect_big_arrays(&self, cb: &CompiledBlockStatement, fold: &mut BigArrayFold) {
        for &span in &self.big_arrays {
            let delta = if span.0 {
                cb.ff_delta_bytes
            } else {
                cb.comb_delta_bytes
            };
            fold.record_span(span, delta);
        }
    }

    pub(crate) fn dynamic_reads(
        &self,
        cb: &CompiledBlockStatement,
        out: &mut Vec<(bool, isize, isize, usize)>,
    ) {
        out.extend(self.dynamic_reads.iter().map(|&(is_ff, a, b, n)| {
            let d = if is_ff {
                cb.ff_delta_bytes
            } else {
                cb.comb_delta_bytes
            };
            (is_ff, a + d, b, n)
        }));
    }

    pub(crate) fn dynamic_writes(
        &self,
        cb: &CompiledBlockStatement,
        out: &mut Vec<(VarOffset, isize, usize)>,
    ) {
        out.extend(
            self.dynamic_writes
                .iter()
                .map(|&(o, stride, n)| (Self::shift(cb, o), stride, n)),
        );
    }

    pub(crate) fn deps(&self, cb: &CompiledBlockStatement, deps: &mut crate::ir::deps::StmtDeps) {
        let shift = |d: &crate::ir::deps::Dep| crate::ir::deps::Dep {
            off: Self::shift(cb, d.off),
            bytes: d.bytes,
        };
        deps.ins.extend(self.dep_ins.iter().map(shift));
        deps.outs.extend(self.dep_outs.iter().map(shift));
    }

    pub(crate) fn ranges(
        &self,
        cb: &CompiledBlockStatement,
        reads: &mut Vec<crate::ir::opt::event_gate::Range>,
        writes: &mut Vec<crate::ir::opt::event_gate::Range>,
    ) {
        let shift = |&(is_ff, a, b): &crate::ir::opt::event_gate::Range| {
            let d = if is_ff {
                cb.ff_delta_bytes
            } else {
                cb.comb_delta_bytes
            };
            (is_ff, (a as isize + d) as usize, (b as isize + d) as usize)
        };
        reads.extend(self.range_reads.iter().map(shift));
        writes.extend(self.range_writes.iter().map(shift));
    }

    pub(crate) fn spans(
        &self,
        spans: &[(isize, isize)],
        cb: &CompiledBlockStatement,
        out: &mut Vec<(isize, isize)>,
    ) {
        let d = cb.comb_delta_bytes;
        out.extend(spans.iter().map(|&(a, b)| (a + d, b + d)));
    }

    /// Every comb offset the class reads, shifted to `cb`'s instance.
    pub(crate) fn comb_reads<'a>(
        &'a self,
        cb: &'a CompiledBlockStatement,
    ) -> impl Iterator<Item = isize> + 'a {
        self.expanded_ins
            .iter()
            .filter_map(move |&o| match Self::shift(cb, o) {
                VarOffset::Comb(c) => Some(c),
                VarOffset::Ff(_) => None,
            })
    }

    /// Every comb offset the class writes, shifted to `cb`'s instance.
    pub(crate) fn comb_writes<'a>(
        &'a self,
        cb: &'a CompiledBlockStatement,
    ) -> impl Iterator<Item = isize> + 'a {
        self.expanded_outs
            .iter()
            .filter_map(move |&o| match Self::shift(cb, o) {
                VarOffset::Comb(c) => Some(c),
                VarOffset::Ff(_) => None,
            })
    }
}

pub(crate) fn diag() -> bool {
    std::env::var("VERYL_HIER_DIAG").is_ok()
}

/// The templates `template_set` chose.
#[derive(Default)]
pub(crate) struct TemplateSet {
    pub(crate) templates: HashSet<usize>,
    /// The templates chosen by name, the only ones a template's conversion
    /// nests.
    pub(crate) named: HashSet<usize>,
    /// A `named` module's specializations that differ in parameters, each
    /// mapped to the group's first one: an instance of any of them runs the
    /// group's template once its statements match the template's.
    pub(crate) groups: HashMap<usize, usize>,
    /// Per group of several specializations, the parameters whose values
    /// differ between the specializations of one module in their subtrees:
    /// read from storage there, so the statements match.
    pub(crate) lifted: HashMap<usize, Arc<HashSet<StrId>>>,
    /// The `named` modules no top instantiates twice.
    pub(crate) unmatched: Vec<String>,
}

/// Components instantiated at least twice under one top that are `named`, or
/// have at least `min_decls` declarations of their own and their
/// descendants'.  A `named` module counts the instances of all its
/// specializations.  Below a chosen component only `named` ones are chosen
/// again, as templates nested in it; the rest of its descendants are
/// converted inside it.
pub(crate) fn template_set(
    ir: &air::Ir,
    tops: &[StrId],
    min_decls: Option<usize>,
    named: &[String],
) -> TemplateSet {
    fn children(m: &air::Module) -> impl Iterator<Item = (usize, &air::Module)> {
        m.declarations.iter().filter_map(|d| match d {
            air::Declaration::Inst(inst) => match inst.component.as_ref() {
                air::Component::Module(child) => {
                    Some((Arc::as_ptr(&inst.component) as usize, child))
                }
                _ => None,
            },
            _ => None,
        })
    }
    fn weight(key: usize, m: &air::Module, memo: &mut HashMap<usize, usize>) -> usize {
        if let Some(w) = memo.get(&key) {
            return *w;
        }
        let mut w = m.declarations.len();
        for (k, child) in children(m) {
            w = w.saturating_add(weight(k, child, memo));
        }
        memo.insert(key, w);
        w
    }
    fn count(
        m: &air::Module,
        counts: &mut HashMap<usize, usize>,
        names: &mut HashMap<usize, String>,
    ) {
        for (k, child) in children(m) {
            *counts.entry(k).or_default() += 1;
            names.entry(k).or_insert_with(|| child.name.to_string());
            count(child, counts, names);
        }
    }
    /// Every parameter value of each module in `m`'s subtree, `m` included.
    fn params(
        m: &air::Module,
        seen: &mut HashSet<usize>,
        key: usize,
        out: &mut HashMap<(StrId, StrId), HashSet<air::ValueVariant>>,
    ) {
        if !seen.insert(key) {
            return;
        }
        for (name, value) in &m.signature.parameters {
            out.entry((m.name, *name))
                .or_default()
                .insert(value.clone());
        }
        for (k, child) in children(m) {
            params(child, seen, k, out);
        }
    }
    struct Rule<'a> {
        counts: &'a HashMap<usize, usize>,
        names: &'a HashMap<usize, String>,
        named_counts: &'a HashMap<String, usize>,
        min_decls: Option<usize>,
        named: &'a [String],
    }
    struct Picked<'a> {
        out: HashSet<usize>,
        /// Each `named` module's chosen specializations, in visiting order.
        named: HashMap<String, Vec<(usize, &'a air::Module)>>,
    }
    fn pick<'a>(
        m: &'a air::Module,
        rule: &Rule,
        weights: &mut HashMap<usize, usize>,
        visited: &mut HashSet<(usize, bool)>,
        picked: &mut Picked<'a>,
        nested: bool,
    ) {
        for (k, child) in children(m) {
            // Weight does not choose below a template, so a component seen
            // there is looked at again where it is not.
            if !visited.insert((k, nested)) {
                continue;
            }
            let name = &rule.names[&k];
            let by_name = rule.named.contains(name) && rule.named_counts[name] >= 2;
            let by_weight = !nested
                && rule.counts.get(&k).copied().unwrap_or(0) >= 2
                && rule
                    .min_decls
                    .is_some_and(|min| weight(k, child, weights) >= min);
            if by_name || by_weight {
                if diag() {
                    eprintln!(
                        "[hier] pick {}: {} instances, weight {}",
                        child.name,
                        rule.counts[&k],
                        weight(k, child, weights)
                    );
                }
                // Several tops reach the same specialization: it is one.
                if picked.out.insert(k) && by_name {
                    picked
                        .named
                        .entry(name.clone())
                        .or_default()
                        .push((k, child));
                }
                pick(child, rule, weights, visited, picked, true);
            } else {
                pick(child, rule, weights, visited, picked, nested);
            }
        }
    }

    let mut picked = Picked {
        out: HashSet::default(),
        named: HashMap::default(),
    };
    let mut weights = HashMap::default();
    let mut repeated: HashSet<String> = HashSet::default();
    for &top in tops {
        let Some(air::Component::Module(m)) = ir
            .components
            .iter()
            .find(|c| matches!(c, air::Component::Module(m) if m.name == top))
        else {
            continue;
        };
        let (mut counts, mut names) = (HashMap::default(), HashMap::default());
        count(m, &mut counts, &mut names);
        let mut named_counts: HashMap<String, usize> = HashMap::default();
        for (k, &n) in &counts {
            *named_counts.entry(names[k].clone()).or_default() += n;
        }
        for (name, &n) in &named_counts {
            if n >= 2 {
                repeated.insert(name.clone());
            }
        }
        let rule = Rule {
            counts: &counts,
            names: &names,
            named_counts: &named_counts,
            min_decls,
            named,
        };
        let mut visited = HashSet::default();
        pick(m, &rule, &mut weights, &mut visited, &mut picked, false);
    }
    let mut set = TemplateSet {
        named: picked
            .named
            .values()
            .flat_map(|specs| specs.iter().map(|&(k, _)| k))
            .collect(),
        templates: picked.out,
        ..Default::default()
    };
    for specs in picked.named.values() {
        if specs.len() < 2 {
            continue;
        }
        let group = specs[0].0;
        let mut values = HashMap::default();
        let mut seen = HashSet::default();
        for &(k, m) in specs {
            set.groups.insert(k, group);
            params(m, &mut seen, k, &mut values);
        }
        let lifted: HashSet<StrId> = values
            .into_iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|((_, p), _)| p)
            .collect();
        if diag() {
            eprintln!(
                "[hier] group of {} specializations lifts {:?}",
                specs.len(),
                lifted.iter().map(|p| p.to_string()).collect::<Vec<_>>()
            );
        }
        set.lifted.insert(group, Arc::new(lifted));
    }
    set.unmatched = named
        .iter()
        .filter(|n| !repeated.contains(*n))
        .cloned()
        .collect();
    set
}

/// One compiled run of a class, in the reference instance's coordinates.
struct SharedChunk {
    stmts: Arc<Vec<ProtoStatement>>,
    artifact: Option<Arc<ChunkArtifact>>,
    inputs: Vec<VarOffset>,
    outputs: Vec<VarOffset>,
}

/// Dataflow cells over a template's statements, and the comb offsets its
/// events write.
type CellsPlan = (Vec<Vec<u32>>, HashSet<isize>);

/// A template's one-pass comb schedule, in the reference instance's
/// coordinates, and its compiled classes per partition.
pub(crate) struct HierTemplate {
    name: StrId,
    sorted: Vec<ProtoStatement>,
    /// The reference instance's comb bytes: an instance moves through the
    /// comb relayout as one unit, so code shared by the instances finds
    /// every comb variable at the same place relative to its delta.
    comb_region: (isize, isize),
    fingerprint: u128,
    compiled: Mutex<HashMap<u64, Arc<Vec<Vec<SharedChunk>>>>>,
    /// The dataflow plan's cells over `sorted`, and the comb offsets the
    /// template's events write; `None` when the plan declined.
    cells_plan: std::sync::OnceLock<Option<CellsPlan>>,
    cells: Mutex<HashMap<u64, Option<Arc<crate::ir::hier_cells::HierCells>>>>,
}

#[derive(Clone)]
pub(crate) struct HierInstance {
    pub template: Arc<HierTemplate>,
    pub ff_delta: isize,
    pub comb_delta: isize,
}

impl HierInstance {
    pub(crate) fn adjusted(&self, ff_delta: isize, comb_delta: isize) -> Self {
        Self {
            template: Arc::clone(&self.template),
            ff_delta: self.ff_delta + ff_delta,
            comb_delta: self.comb_delta + comb_delta,
        }
    }
}

/// Fold of every instance's identity into the parent's comb fingerprint: the
/// template statements are not in the parent's list.
pub(crate) fn fingerprint(instances: &[HierInstance]) -> u128 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for i in instances {
        i.template.fingerprint.hash(&mut h);
        i.ff_delta.hash(&mut h);
        i.comb_delta.hash(&mut h);
    }
    u128::from(h.finish())
}

/// Remove the template nodes of nested instances from `events`.
pub(crate) fn take_nested_events(
    events: &mut HashMap<crate::ir::Event, Vec<ProtoStatement>>,
) -> HashMap<crate::ir::Event, Vec<ProtoStatement>> {
    let mut out: HashMap<crate::ir::Event, Vec<ProtoStatement>> = HashMap::default();
    for (event, stmts) in events.iter_mut() {
        if !stmts
            .iter()
            .any(|s| matches!(s, ProtoStatement::HierEvents(_)))
        {
            continue;
        }
        let (nodes, rest): (Vec<_>, Vec<_>) = std::mem::take(stmts)
            .into_iter()
            .partition(|s| matches!(s, ProtoStatement::HierEvents(_)));
        *stmts = rest;
        out.insert(event.clone(), nodes);
    }
    events.retain(|event, stmts| !stmts.is_empty() || !out.contains_key(event));
    out
}

/// Have a template node read the parent nets in `map` (port to net, sorted
/// as `inputs`) in place of the ports: a node of this template takes them
/// all, as aliasing would; a nested one, which already reads its own ports
/// from nets here, follows those nets that are ports of this template.
pub(crate) fn read_from_parent(
    h: &mut crate::ir::ProtoHierEvents,
    map: &HashMap<VarOffset, VarOffset>,
    inputs: &[(VarOffset, VarOffset)],
) {
    if std::mem::replace(&mut h.at_boundary, true) {
        for (_, to) in &mut h.inputs {
            if let Some(&net) = map.get(to) {
                *to = net;
            }
        }
    } else {
        h.inputs = inputs.to_vec();
    }
}

/// The FF bytes `stmt` reads and writes, as `(reads, writes)`.
fn ff_io(stmt: &ProtoStatement) -> (Vec<isize>, Vec<isize>) {
    let fold = BigArrayFold::default();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    stmt.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
    let ff = |v: Vec<VarOffset>| -> Vec<isize> {
        let mut v: Vec<isize> = v.iter().filter(|o| o.is_ff()).map(|o| o.raw()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    (ff(ins), ff(outs))
}

fn comb_io(stmt: &ProtoStatement, reads: &mut Vec<isize>, writes: &mut Vec<isize>) {
    let fold = BigArrayFold::default();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    stmt.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
    reads.clear();
    writes.clear();
    reads.extend(ins.iter().filter(|o| !o.is_ff()).map(|o| o.raw()));
    writes.extend(outs.iter().filter(|o| !o.is_ff()).map(|o| o.raw()));
    reads.sort_unstable();
    reads.dedup();
    writes.sort_unstable();
    writes.dedup();
}

impl HierTemplate {
    /// Schedule `stmts` (the reference instance's comb statements, in its
    /// region) for one pass.  Rename temps are allocated inside the region, so
    /// the caller must measure the region after this returns.  On failure the
    /// statements come back for the flat path.
    pub(crate) fn build(
        context: &mut Context,
        name: StrId,
        mut stmts: Vec<ProtoStatement>,
        ff_start: isize,
        comb_start: isize,
    ) -> Result<Arc<HierTemplate>, Vec<ProtoStatement>> {
        crate::ir::comb_layout::expand_compiled_blocks(&mut stmts);
        let backup = stmts.clone();
        let use_4state = context.config.use_4state;
        let comb_total = &mut context.comb_total_bytes;
        // A temp is recorded as a relocation of the variable it serves, so
        // the owner tables adopt it (see `Context::comb_reloc`).
        let comb_reloc = &mut context.comb_reloc;
        let mut alloc = |width: usize, from: isize| -> isize {
            let nb = crate::ir::variable::native_bytes(width);
            let vs = crate::ir::variable::value_size(nb, use_4state);
            let off = *comb_total as isize;
            *comb_total += vs;
            comb_reloc.push((from, off, vs));
            off
        };
        // Whole blocks conflate their I/O, which the parent would read as
        // loops through the template.
        let sorted = match crate::ir::module::analyze_dependency_split(stmts, &mut alloc) {
            Ok((sorted, hint, _)) => {
                let sorted = crate::ir::module::reorder_by_level(sorted);
                let passes = hint.unwrap_or_else(|| {
                    crate::ir::module::compute_required_passes("template", &sorted)
                });
                if passes != 1 {
                    if diag() {
                        eprintln!("[hier] {name}: template needs {passes} passes");
                    }
                    return Err(sorted);
                }
                sorted
            }
            Err(_) => return Err(backup),
        };
        let ff_end = context.ff_total_bytes as isize;
        let comb_end = context.comb_total_bytes as isize;
        let fold = BigArrayFold::default();
        let mut ins = Vec::new();
        let mut outs = Vec::new();
        for s in &sorted {
            ins.clear();
            outs.clear();
            s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
            let outside = ins.iter().chain(outs.iter()).find(|o| {
                let (lo, hi) = if o.is_ff() {
                    (ff_start, ff_end)
                } else {
                    (comb_start, comb_end)
                };
                !(lo..hi).contains(&o.raw())
            });
            if let Some(o) = outside {
                if diag() {
                    eprintln!("[hier] {name}: statement touches {o:?} outside its region");
                }
                return Err(sorted);
            }
        }
        let fingerprint = crate::backend::registry::comb_list_fingerprint(use_4state, &sorted, 0);
        if diag() {
            eprintln!("[hier] {name}: template of {} statements", sorted.len());
        }
        Ok(Arc::new(HierTemplate {
            name,
            sorted,
            comb_region: (comb_start, comb_end),
            fingerprint,
            compiled: Mutex::default(),
            cells_plan: std::sync::OnceLock::new(),
            cells: Mutex::default(),
        }))
    }

    pub(crate) fn fingerprint(&self) -> u128 {
        self.fingerprint
    }

    /// Plan the template's dataflow cells from the reference instance's
    /// metadata and events.
    pub(crate) fn plan_cells(
        &self,
        name: &str,
        variable_meta: &HashMap<veryl_analyzer::ir::VarId, crate::ir::variable::VariableMeta>,
        children: &[crate::ir::variable::ModuleVariableMeta],
        events: &HashMap<crate::ir::Event, Vec<ProtoStatement>>,
        comb_reloc: &[(isize, isize, usize)],
        use_4state: bool,
    ) {
        let plan = crate::ir::module::collect_event_written_comb(events).and_then(|evt| {
            let inputs = crate::ir::opt::owners::build_inputs(
                name,
                variable_meta,
                children,
                &evt,
                comb_reloc,
                use_4state,
            );
            crate::ir::opt::dataflow::plan(&self.sorted, &inputs).map(|p| (p.cells, evt))
        });
        if diag() {
            match &plan {
                Some((c, _)) => eprintln!("[hier] {}: {} dataflow cells", self.name, c.len()),
                None => eprintln!("[hier] {}: no dataflow cells", self.name),
            }
        }
        let _ = self.cells_plan.set(plan);
    }
}

/// Per-template data the level iteration needs, in reference coordinates.
struct TemplateIo {
    /// Per statement: indices into `inputs` / `vars` it reads, `vars` it writes.
    reads_in: Vec<Vec<u32>>,
    reads_var: Vec<Vec<u32>>,
    writes_var: Vec<Vec<u32>>,
    /// Offsets read but never written by the template.
    inputs: Vec<isize>,
    /// Offsets the template writes.
    vars: Vec<isize>,
    /// Statements writing an offset some other statement also writes.
    shares_writes: Vec<bool>,
    /// Per statement: offsets it reads before a later statement writes them.
    /// The parent orders a reader after every writer, so such a reader must
    /// share its writers' class.
    early_reads: Vec<Vec<u32>>,
    /// Per statement: the FF bytes it writes from the comb (a comb store to
    /// an element of a flop array) and the ones it reads that some statement
    /// writes so, as indices into one numbering.  As for a comb offset, the
    /// writers and the readers before the last writer share one class, and a
    /// reader after the writers takes at least theirs.
    ff_writes: Vec<Vec<u32>>,
    ff_reads: Vec<Vec<u32>>,
    ff_early_reads: Vec<Vec<u32>>,
    ff_count: usize,
}

impl TemplateIo {
    fn new(sorted: &[ProtoStatement]) -> Self {
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        let mut written: HashMap<isize, u32> = HashMap::default();
        let mut vars = Vec::new();
        let mut per_stmt: Vec<(Vec<isize>, Vec<u32>)> = Vec::with_capacity(sorted.len());
        for s in sorted {
            comb_io(s, &mut reads, &mut writes);
            let w: Vec<u32> = writes
                .iter()
                .map(|&o| {
                    *written.entry(o).or_insert_with(|| {
                        vars.push(o);
                        vars.len() as u32 - 1
                    })
                })
                .collect();
            per_stmt.push((reads.clone(), w));
        }
        let mut input_of: HashMap<isize, u32> = HashMap::default();
        let mut inputs = Vec::new();
        let mut reads_in = Vec::with_capacity(sorted.len());
        let mut reads_var = Vec::with_capacity(sorted.len());
        let mut writes_var = Vec::with_capacity(sorted.len());
        for (r, w) in per_stmt {
            let mut ri = Vec::new();
            let mut rv = Vec::new();
            for o in r {
                if let Some(&v) = written.get(&o) {
                    rv.push(v);
                } else {
                    ri.push(*input_of.entry(o).or_insert_with(|| {
                        inputs.push(o);
                        inputs.len() as u32 - 1
                    }));
                }
            }
            reads_in.push(ri);
            reads_var.push(rv);
            writes_var.push(w);
        }
        let mut writers = vec![0u32; vars.len()];
        for w in &writes_var {
            for &v in w {
                writers[v as usize] += 1;
            }
        }
        let mut last_writer = vec![0usize; vars.len()];
        for (i, w) in writes_var.iter().enumerate() {
            for &v in w {
                last_writer[v as usize] = i;
            }
        }
        let early_reads: Vec<Vec<u32>> = reads_var
            .iter()
            .enumerate()
            .map(|(i, r)| {
                r.iter()
                    .copied()
                    .filter(|&v| last_writer[v as usize] > i)
                    .collect()
            })
            .collect();
        let shares_writes = writes_var
            .iter()
            .zip(&early_reads)
            .map(|(w, e)| !e.is_empty() || w.iter().any(|&v| writers[v as usize] > 1))
            .collect();
        let ff_io: Vec<(Vec<isize>, Vec<isize>)> = sorted.iter().map(ff_io).collect();
        let mut ff_index: HashMap<isize, u32> = HashMap::default();
        for (_, w) in &ff_io {
            for &o in w {
                let n = ff_index.len() as u32;
                ff_index.entry(o).or_insert(n);
            }
        }
        let numbered = |offsets: &[isize]| -> Vec<u32> {
            offsets
                .iter()
                .filter_map(|o| ff_index.get(o).copied())
                .collect()
        };
        let ff_reads: Vec<Vec<u32>> = ff_io.iter().map(|(r, _)| numbered(r)).collect();
        let ff_writes: Vec<Vec<u32>> = ff_io.iter().map(|(_, w)| numbered(w)).collect();
        let mut last_ff_writer = vec![0usize; ff_index.len()];
        for (i, w) in ff_writes.iter().enumerate() {
            for &f in w {
                last_ff_writer[f as usize] = i;
            }
        }
        let ff_early_reads = ff_reads
            .iter()
            .enumerate()
            .map(|(i, r)| {
                r.iter()
                    .copied()
                    .filter(|&f| last_ff_writer[f as usize] > i)
                    .collect()
            })
            .collect();
        Self {
            reads_in,
            reads_var,
            writes_var,
            inputs,
            vars,
            shares_writes,
            early_reads,
            ff_writes,
            ff_reads,
            ff_early_reads,
            ff_count: ff_index.len(),
        }
    }

    /// Statement labels from input levels; fills `var_level` on the way.
    /// Every writer of one offset gets the same label, so the offset belongs
    /// to one class.
    fn label(&self, in_level: &[u32], var_level: &mut [u32], labels: &mut Vec<u32>) {
        let n = self.reads_in.len();
        labels.clear();
        labels.resize(n, 0);
        let mut ff_level = vec![0u32; self.ff_count];
        loop {
            var_level.fill(0);
            ff_level.fill(0);
            for (i, label) in labels.iter_mut().enumerate() {
                let mut l = *label;
                for &k in &self.reads_in[i] {
                    l = l.max(in_level[k as usize]);
                }
                for &v in &self.reads_var[i] {
                    l = l.max(var_level[v as usize]);
                }
                for &f in &self.ff_reads[i] {
                    l = l.max(ff_level[f as usize]);
                }
                for &f in self.ff_writes[i].iter().chain(&self.ff_early_reads[i]) {
                    ff_level[f as usize] = ff_level[f as usize].max(l);
                }
                for &v in self.writes_var[i].iter().chain(&self.early_reads[i]) {
                    let slot = &mut var_level[v as usize];
                    *slot = (*slot).max(l);
                }
                *label = l;
            }
            let mut raised = false;
            for (i, label) in labels.iter_mut().enumerate() {
                for &f in self.ff_writes[i].iter().chain(&self.ff_early_reads[i]) {
                    if ff_level[f as usize] > *label {
                        *label = ff_level[f as usize];
                        raised = true;
                    }
                }
                if !self.shares_writes[i] {
                    continue;
                }
                for &v in self.writes_var[i].iter().chain(&self.early_reads[i]) {
                    if var_level[v as usize] > *label {
                        *label = var_level[v as usize];
                        raised = true;
                    }
                }
            }
            if !raised {
                return;
            }
        }
    }
}

/// The parent's comb statements as a bipartite statement/offset graph, in
/// topological order of its strongly connected components.
struct Glue {
    stmts: Vec<ProtoStatement>,
    node_of: HashMap<isize, u32>,
    /// Offset nodes first (`0..offsets`), then statements.
    offsets: usize,
    /// Per node: predecessors.
    preds: Vec<Vec<u32>>,
    /// SCCs in topological order.
    sccs: Vec<Vec<u32>>,
}

impl Glue {
    fn new(stmts: Vec<ProtoStatement>) -> Self {
        let mut node_of: HashMap<isize, u32> = HashMap::default();
        let mut io: Vec<(Vec<isize>, Vec<isize>)> = Vec::with_capacity(stmts.len());
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        for s in &stmts {
            comb_io(s, &mut reads, &mut writes);
            for &o in reads.iter().chain(writes.iter()) {
                let n = node_of.len() as u32;
                node_of.entry(o).or_insert(n);
            }
            io.push((reads.clone(), writes.clone()));
        }
        let offsets = node_of.len();
        let mut preds: Vec<Vec<u32>> = vec![Vec::new(); offsets + stmts.len()];
        let mut graph: Graph<(), ()> = Graph::with_capacity(preds.len(), 0);
        for _ in 0..preds.len() {
            graph.add_node(());
        }
        for (i, (r, w)) in io.iter().enumerate() {
            let s = (offsets + i) as u32;
            for o in r {
                let n = node_of[o];
                // A statement reading what it writes updates part of it.
                if w.binary_search(o).is_err() {
                    preds[s as usize].push(n);
                    graph.add_edge(n.into(), s.into(), ());
                }
            }
            for o in w {
                let n = node_of[o];
                preds[n as usize].push(s);
                graph.add_edge(s.into(), n.into(), ());
            }
        }
        let mut sccs: Vec<Vec<u32>> = tarjan_scc(&graph)
            .into_iter()
            .map(|c| c.into_iter().map(|n| n.index() as u32).collect())
            .collect();
        sccs.reverse();
        Self {
            stmts,
            node_of,
            offsets,
            preds,
            sccs,
        }
    }

    /// Longest-path levels, where reading an offset a template writes costs
    /// one: `exits` holds those offsets' levels.
    fn levels(&self, exits: &HashMap<u32, u32>, level: &mut [u32]) {
        level.fill(0);
        let mut member: Vec<bool> = Vec::new();
        for scc in &self.sccs {
            let mut l = 0;
            let multi = scc.len() > 1;
            if multi {
                member.resize(level.len(), false);
                for &n in scc {
                    member[n as usize] = true;
                }
            }
            for &n in scc {
                if (n as usize) < self.offsets
                    && let Some(&e) = exits.get(&n)
                {
                    l = l.max(e);
                }
                let is_stmt = n as usize >= self.offsets;
                for &p in &self.preds[n as usize] {
                    if multi && member[p as usize] {
                        continue;
                    }
                    let exit = is_stmt && exits.contains_key(&p);
                    l = l.max(level[p as usize] + exit as u32);
                }
            }
            for &n in scc {
                level[n as usize] = l;
                if multi {
                    member[n as usize] = false;
                }
            }
        }
    }
}

struct Group<'a> {
    template: &'a Arc<HierTemplate>,
    io: TemplateIo,
    deltas: Vec<(isize, isize)>,
    labels: Vec<Vec<u32>>,
}

/// Class of every template statement, and the classes in emission order, or
/// why the partition is unsound.
fn partition(group: &Group) -> Result<(Vec<u32>, Vec<u32>), String> {
    let n = group.io.reads_in.len();
    let mut class_of = vec![0u32; n];
    let mut key_of: HashMap<Vec<u32>, u32> = HashMap::default();
    let mut first: Vec<(u64, usize)> = Vec::new();
    for (s, class) in class_of.iter_mut().enumerate() {
        let key: Vec<u32> = group.labels.iter().map(|l| l[s]).collect();
        let next = key_of.len() as u32;
        *class = *key_of.entry(key.clone()).or_insert_with(|| {
            first.push((key.iter().map(|&x| x as u64).sum(), s));
            next
        });
    }
    // A block reading what another block also writes would lose that edge
    // as a self-reference in the parent's dependency analysis.
    let mut writer_class: Vec<Option<u32>> = vec![None; group.io.vars.len()];
    for (writes, &class) in group.io.writes_var.iter().zip(&class_of) {
        for &v in writes {
            match writer_class[v as usize] {
                None => writer_class[v as usize] = Some(class),
                Some(c) if c != class => {
                    return Err(format!(
                        "offset {} is written by two classes",
                        group.io.vars[v as usize]
                    ));
                }
                _ => {}
            }
        }
    }
    let mut order: Vec<u32> = (0..first.len() as u32).collect();
    order.sort_by_key(|&c| first[c as usize]);
    Ok((class_of, order))
}

#[cfg(test)]
thread_local! {
    /// Chunk budget override, so small designs exercise the ordering reads.
    pub(crate) static CHUNK_MASS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// One compile group of a class.
struct Planned {
    stmts: Vec<ProtoStatement>,
    jit: bool,
    /// An offset only earlier groups write, read to keep this group after
    /// the previous one where both write some offset: the parent's analysis
    /// drops a block's read of what it writes itself.
    after: Option<isize>,
}

/// Cut one class into compile groups by mass.  Jittable and interpreted runs
/// meet only where no offset has writers on both sides (an interpreted
/// statement cannot carry the ordering read); a group that would need one
/// keeps both kinds and stays interpreted.
fn plan_class(context: &Context, stmts: Vec<ProtoStatement>) -> Vec<Planned> {
    #[cfg(test)]
    let max_mass = CHUNK_MASS
        .with(|m| m.get())
        .unwrap_or_else(crate::ir::module::jit_chunk_size);
    #[cfg(not(test))]
    let max_mass = crate::ir::module::jit_chunk_size();
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut last_writer: HashMap<isize, usize> = HashMap::default();
    let mut first_writer: HashMap<isize, usize> = HashMap::default();
    let mut writes_of: Vec<Vec<isize>> = Vec::with_capacity(stmts.len());
    let mut reads_of: Vec<Vec<isize>> = Vec::with_capacity(stmts.len());
    for (i, s) in stmts.iter().enumerate() {
        comb_io(s, &mut reads, &mut writes);
        for &o in &writes {
            first_writer.entry(o).or_insert(i);
            last_writer.insert(o, i);
        }
        writes_of.push(writes.clone());
        reads_of.push(reads.clone());
    }
    // `fixed[i]`: something before `i` reads an offset written again from
    // `i` on.  The later group drops that read (the parent orders a reader
    // after every writer), so the cut needs the ordering read as well.
    let mut war_until = vec![0usize; stmts.len() + 1];
    for (i, r) in reads_of.iter().enumerate() {
        for o in r {
            if let Some(&l) = last_writer.get(o)
                && l > i
            {
                war_until[i] = war_until[i].max(l);
            }
        }
    }
    let mut fixed = vec![false; stmts.len() + 1];
    let mut reach = 0usize;
    for i in 0..stmts.len() {
        if i > 0 && reach >= i {
            fixed[i] = true;
        }
        reach = reach.max(war_until[i]);
    }
    // `blocked[i]`: some offset has writers both before and from `i` on.
    let mut open_until = vec![0usize; stmts.len() + 1];
    for (o, &f) in &first_writer {
        let l = last_writer[o];
        if l > f {
            open_until[f] = open_until[f].max(l);
        }
    }
    let mut blocked = vec![false; stmts.len() + 1];
    let mut reach = 0usize;
    for i in 0..stmts.len() {
        if i > 0 && reach >= i {
            blocked[i] = true;
        }
        reach = reach.max(open_until[i]);
    }
    let mut out: Vec<Planned> = Vec::new();
    let mut cur: Vec<ProtoStatement> = Vec::new();
    let mut cur_jit = true;
    let mut mixed = false;
    let mut mass = 0usize;
    let mut after = None;
    // An offset whose last writer is in `cur`.
    let mut token: Option<isize> = None;
    for (i, s) in stmts.into_iter().enumerate() {
        let jit = context.backends.any_supports_stmt(&s);
        let m = s.statement_mass();
        if !cur.is_empty() {
            let spanned = blocked[i] || fixed[i];
            let kind_cut = jit != cur_jit && !spanned;
            let mass_cut = jit == cur_jit && cur_jit && !mixed && mass + m > max_mass;
            let next_after = if spanned { token } else { None };
            if kind_cut || (mass_cut && (!spanned || token.is_some())) {
                out.push(Planned {
                    stmts: std::mem::take(&mut cur),
                    jit: cur_jit && !mixed,
                    after,
                });
                after = next_after;
                mass = 0;
                mixed = false;
                token = None;
            }
        }
        if cur.is_empty() {
            cur_jit = jit;
        } else if jit != cur_jit {
            mixed = true;
        }
        if let Some(&o) = writes_of[i].iter().find(|o| last_writer[*o] == i) {
            token = Some(o);
        }
        mass += m;
        cur.push(s);
    }
    if !cur.is_empty() {
        out.push(Planned {
            stmts: cur,
            jit: cur_jit && !mixed,
            after,
        });
    }
    out
}

fn compile_classes(
    context: &mut Context,
    template: &HierTemplate,
    class_of: &[u32],
    classes: usize,
) -> Option<Arc<Vec<Vec<SharedChunk>>>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    class_of.hash(&mut h);
    let key = h.finish();
    if let Some(c) = template.compiled.lock().unwrap().get(&key) {
        return Some(Arc::clone(c));
    }
    let mut per_class: Vec<Vec<ProtoStatement>> = vec![Vec::new(); classes];
    for (s, &c) in template.sorted.iter().zip(class_of) {
        per_class[c as usize].push(s.clone());
    }
    let mut plans: Vec<(usize, Planned)> = Vec::new();
    for (c, stmts) in per_class.into_iter().enumerate() {
        for p in plan_class(context, stmts) {
            plans.push((c, p));
        }
    }
    let to_compile: Vec<&[ProtoStatement]> = plans
        .iter()
        .filter(|p| p.1.jit)
        .map(|p| p.1.stmts.as_slice())
        .collect();
    let artifacts = {
        let ctx = CompileCtx {
            config: &context.config,
            use_4state: context.config.use_4state,
            contains_compiled_block: false,
            df_marks: None,
        };
        compile_chunks_parallel(&mut context.backends, &ctx, &to_compile)
    };
    let mut artifacts = artifacts.into_iter();
    let mut out: Vec<Vec<SharedChunk>> = (0..classes).map(|_| Vec::new()).collect();
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    // Group writing each offset last; a group reading an offset that a later
    // group writes again reads the earlier value, ordered by the chain.
    let mut last_group: HashMap<isize, (usize, usize)> = HashMap::default();
    for (idx, (c, p)) in plans.iter().enumerate() {
        for s in &p.stmts {
            comb_io(s, &mut reads, &mut writes);
            for &o in &writes {
                last_group.insert(o, (*c, idx));
            }
        }
    }
    for (idx, (c, Planned { stmts, jit, after })) in plans.into_iter().enumerate() {
        let artifact = if jit {
            artifacts.next().flatten()
        } else {
            None
        };
        // Interpreted statements cannot carry the ordering read.
        if after.is_some() && artifact.is_none() {
            return None;
        }
        let mut written: HashSet<isize> = HashSet::default();
        let mut read: Vec<isize> = Vec::new();
        for s in &stmts {
            comb_io(s, &mut reads, &mut writes);
            read.extend(reads.iter().copied());
            written.extend(writes.iter().copied());
        }
        read.sort_unstable();
        read.dedup();
        read.retain(|o| {
            !written.contains(o) && last_group.get(o).is_none_or(|&(gc, g)| gc != c || g < idx)
        });
        read.extend(after);
        let mut outputs: Vec<isize> = written.into_iter().collect();
        outputs.sort_unstable();
        out[c].push(SharedChunk {
            stmts: Arc::new(stmts),
            artifact,
            inputs: read.into_iter().map(VarOffset::Comb).collect(),
            outputs: outputs.into_iter().map(VarOffset::Comb).collect(),
        });
    }
    let out = Arc::new(out);
    template
        .compiled
        .lock()
        .unwrap()
        .insert(key, Arc::clone(&out));
    Some(out)
}

/// The template's pieces depend on their order where several write one
/// offset; a sequential block keeps that order through the parent's analysis.
fn push_flat(
    unified: &mut Vec<ProtoStatement>,
    template: &HierTemplate,
    ff_delta: isize,
    comb_delta: isize,
) {
    let body = template
        .sorted
        .iter()
        .map(|s| {
            let mut c = s.clone();
            c.adjust_offsets(ff_delta, comb_delta);
            c
        })
        .collect();
    unified.push(ProtoStatement::SequentialBlock(body));
}

/// Add every instance's comb logic to the parent's list: as shared compiled
/// blocks when the partition holds, else as relocated statements.
pub(crate) fn inject(
    context: &mut Context,
    unified: &mut Vec<ProtoStatement>,
    instances: &[HierInstance],
    names: Option<&HashMap<VarOffset, String>>,
) {
    if instances.is_empty() {
        return;
    }
    let t0 = std::time::Instant::now();
    let mut groups: Vec<Group> = Vec::new();
    for inst in instances {
        let pos = groups
            .iter()
            .position(|g| Arc::ptr_eq(g.template, &inst.template));
        let g = match pos {
            Some(p) => &mut groups[p],
            None => {
                groups.push(Group {
                    template: &inst.template,
                    io: TemplateIo::new(&inst.template.sorted),
                    deltas: Vec::new(),
                    labels: Vec::new(),
                });
                groups.last_mut().unwrap()
            }
        };
        g.deltas.push((inst.ff_delta, inst.comb_delta));
    }
    // A template whose classes do not hold runs flat; the others are planned
    // again around its statements.
    let (partitions, with_cells, compiled_all) = loop {
        if let Err(why) = plan_all(unified, &mut groups, names) {
            if diag() {
                eprintln!("[hier] flat fallback: {why}");
            }
            context.hier_fell_back = Some(why);
            for g in &groups {
                for &(ff_delta, comb_delta) in &g.deltas {
                    push_flat(unified, g.template, ff_delta, comb_delta);
                }
            }
            return;
        }
        let mut failed: Vec<(usize, String)> = Vec::new();
        let mut partitions = Vec::with_capacity(groups.len());
        for (gi, g) in groups.iter().enumerate() {
            match partition(g) {
                Ok(p) => partitions.push(p),
                Err(why) => {
                    failed.push((gi, why));
                    partitions.push(Default::default());
                }
            }
        }
        let mut with_cells = Vec::new();
        let mut compiled_all = Vec::with_capacity(groups.len());
        if failed.is_empty() {
            for (g, (class_of, order)) in groups.iter().zip(&partitions) {
                with_cells.push(template_cells(context, g.template, class_of, order));
            }
            for (gi, ((g, (class_of, order)), cells)) in
                groups.iter().zip(&partitions).zip(&with_cells).enumerate()
            {
                if cells.is_some() {
                    compiled_all.push(Arc::new(Vec::new()));
                    continue;
                }
                match compile_classes(context, g.template, class_of, order.len()) {
                    Some(c) => compiled_all.push(c),
                    None => failed.push((gi, "a chunk did not compile".to_string())),
                }
            }
        }
        if failed.is_empty() {
            break (partitions, with_cells, compiled_all);
        }
        let all = failed.len() == groups.len();
        for &(gi, ref why) in failed.iter().rev() {
            let g = groups.remove(gi);
            let why = format!("{}: {why}", g.template.name);
            if diag() {
                eprintln!("[hier] flat fallback: {why}");
            }
            for &(ff_delta, comb_delta) in &g.deltas {
                push_flat(unified, g.template, ff_delta, comb_delta);
            }
            if all {
                context.hier_fell_back = Some(why);
            } else if !context.config.shared_modules.is_empty() {
                // The others stay shared, so the build is not redone without
                // templates: say which one runs flat.
                log::warn!("[test] shared_modules: {why}, run flat");
            }
        }
        if all {
            return;
        }
    };
    let t1 = std::time::Instant::now();
    let mut runs = 0u32;
    for (((g, (class_of, order)), compiled), cells) in groups
        .iter()
        .zip(partitions)
        .zip(compiled_all)
        .zip(with_cells)
    {
        if let Some(cells) = cells {
            emit_cells(unified, g, &class_of, &order, &cells, &mut runs);
            continue;
        }
        if diag() {
            let chunks: usize = compiled.iter().map(|c| c.len()).sum();
            let interpreted: usize = compiled
                .iter()
                .flatten()
                .filter(|c| c.artifact.is_none())
                .map(|c| c.stmts.len())
                .sum();
            eprintln!(
                "[hier] {}: {} instances, {} classes, {chunks} chunks, {interpreted} interpreted statements",
                g.template.name,
                g.deltas.len(),
                order.len(),
            );
        }
        for (j, &(ff_delta, comb_delta)) in g.deltas.iter().enumerate() {
            // Within one instance its classes run by their label there.
            let mut inst_order = order.clone();
            let first_label = |c: u32| -> u32 {
                class_of
                    .iter()
                    .position(|&x| x == c)
                    .map(|s| g.labels[j][s])
                    .unwrap_or(0)
            };
            let label_of: HashMap<u32, u32> =
                inst_order.iter().map(|&c| (c, first_label(c))).collect();
            inst_order.sort_by_key(|c| label_of[c]);
            for c in inst_order {
                for chunk in &compiled[c as usize] {
                    let originals: Vec<ProtoStatement> = chunk
                        .stmts
                        .iter()
                        .map(|s| {
                            let mut s = s.clone();
                            s.adjust_offsets(ff_delta, comb_delta);
                            s
                        })
                        .collect();
                    match &chunk.artifact {
                        Some(artifact) => {
                            let adjust = |v: &[VarOffset]| -> Vec<VarOffset> {
                                v.iter().map(|o| o.adjust(ff_delta, comb_delta)).collect()
                            };
                            unified.push(ProtoStatement::CompiledBlock(CompiledBlockStatement {
                                artifact: Arc::clone(artifact),
                                ff_delta_bytes: ff_delta,
                                comb_delta_bytes: comb_delta,
                                input_offsets: adjust(&chunk.inputs),
                                output_offsets: adjust(&chunk.outputs),
                                ff_canonical_offsets: Vec::new(),
                                stmt_deps: Arc::new(Vec::new()),
                                original_stmts: Arc::new(originals),
                                shared: true,
                                hier: None,
                            }));
                        }
                        None => unified.push(ProtoStatement::SequentialBlock(originals)),
                    }
                }
            }
        }
    }
    if diag() {
        eprintln!(
            "[hier] plan and compile {:.1}s, emit {:.1}s",
            (t1 - t0).as_secs_f64(),
            t1.elapsed().as_secs_f64()
        );
    }
}

/// The template's dirty-tracked cells for this partition, built once.
fn template_cells(
    context: &mut Context,
    template: &HierTemplate,
    class_of: &[u32],
    order: &[u32],
) -> Option<Arc<crate::ir::hier_cells::HierCells>> {
    let (plan_cells, event_written) = template.cells_plan.get()?.as_ref()?;
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    class_of.hash(&mut h);
    order.hash(&mut h);
    let key = h.finish();
    if let Some(c) = template.cells.lock().unwrap().get(&key) {
        return c.clone();
    }
    let cells = crate::ir::hier_cells::build(
        context,
        &template.sorted,
        plan_cells,
        class_of,
        order,
        event_written,
    );
    if diag() {
        match &cells {
            Some(c) => eprintln!("[hier] {}: {}", template.name, c.summary()),
            None => eprintln!("[hier] {}: cells declined", template.name),
        }
    }
    template.cells.lock().unwrap().insert(key, cells.clone());
    cells
}

/// One atomic block per class and instance, run through the cells.
fn emit_cells(
    unified: &mut Vec<ProtoStatement>,
    g: &Group,
    class_of: &[u32],
    order: &[u32],
    cells: &Arc<crate::ir::hier_cells::HierCells>,
    runs: &mut u32,
) {
    let classes = order.len();
    let mut per_class: Vec<Vec<&ProtoStatement>> = vec![Vec::new(); classes];
    for (s, &c) in g.template.sorted.iter().zip(class_of) {
        per_class[c as usize].push(s);
    }
    let class_stmts: Vec<Arc<Vec<ProtoStatement>>> = per_class
        .iter()
        .map(|stmts| Arc::new(stmts.iter().map(|&s| s.clone()).collect()))
        .collect();
    let summaries: Vec<Arc<HierSummary>> = class_stmts
        .iter()
        .map(|stmts| Arc::new(HierSummary::new(stmts)))
        .collect();
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let io: Vec<(Vec<VarOffset>, Vec<VarOffset>)> = per_class
        .iter()
        .map(|stmts| {
            let mut read: Vec<isize> = Vec::new();
            let mut written: HashSet<isize> = HashSet::default();
            for s in stmts {
                comb_io(s, &mut reads, &mut writes);
                read.extend(reads.iter().copied());
                written.extend(writes.iter().copied());
            }
            read.sort_unstable();
            read.dedup();
            read.retain(|o| !written.contains(o));
            let mut out: Vec<isize> = written.into_iter().collect();
            out.sort_unstable();
            (
                read.into_iter().map(VarOffset::Comb).collect(),
                out.into_iter().map(VarOffset::Comb).collect(),
            )
        })
        .collect();
    for (j, &(ff_delta, comb_delta)) in g.deltas.iter().enumerate() {
        let mut inst_order = order.to_vec();
        let label_of: HashMap<u32, u32> = inst_order
            .iter()
            .map(|&c| {
                let l = class_of
                    .iter()
                    .position(|&x| x == c)
                    .map(|s| g.labels[j][s])
                    .unwrap_or(0);
                (c, l)
            })
            .collect();
        inst_order.sort_by_key(|c| label_of[c]);
        for c in inst_order {
            let Some(artifact) = cells.class_artifact(c) else {
                continue;
            };
            let adjust = |v: &[VarOffset]| -> Vec<VarOffset> {
                v.iter().map(|o| o.adjust(ff_delta, comb_delta)).collect()
            };
            unified.push(ProtoStatement::CompiledBlock(CompiledBlockStatement {
                artifact,
                ff_delta_bytes: ff_delta,
                comb_delta_bytes: comb_delta,
                input_offsets: adjust(&io[c as usize].0),
                output_offsets: adjust(&io[c as usize].1),
                ff_canonical_offsets: Vec::new(),
                stmt_deps: Arc::new(Vec::new()),
                original_stmts: Arc::new(Vec::new()),
                shared: true,
                hier: Some(crate::ir::statement::HierRef {
                    cells: Arc::clone(cells),
                    class: c,
                    partition: u32::MAX,
                    run: {
                        *runs += 1;
                        *runs - 1
                    },
                    summary: Arc::clone(&summaries[c as usize]),
                    region: g.template.comb_region,
                    class_stmts: Arc::clone(&class_stmts[c as usize]),
                }),
            }));
            #[cfg(debug_assertions)]
            if let Some(ProtoStatement::CompiledBlock(cb)) = unified.last() {
                summaries[c as usize].debug_check(cb);
            }
        }
    }
}

/// Iterate glue levels and template labels to a fixed point.
fn plan_all(
    unified: &[ProtoStatement],
    groups: &mut [Group],
    names: Option<&HashMap<VarOffset, String>>,
) -> Result<(), String> {
    let glue = Glue::new(crate::ir::module::split_for_analysis(unified.to_vec()));
    let nodes = glue.preds.len();
    let mut level = vec![0u32; nodes];
    // Glue nodes of every instance's template-written offsets, per group.
    let mut exit_nodes: Vec<Vec<Vec<(u32, u32)>>> = Vec::new();
    for g in groups.iter() {
        let mut per_inst = Vec::new();
        for &(_, comb_delta) in &g.deltas {
            let mut v = Vec::new();
            for (idx, &o) in g.io.vars.iter().enumerate() {
                if let Some(&n) = glue.node_of.get(&(o + comb_delta)) {
                    if !glue.preds[n as usize].is_empty() {
                        return Err(format!(
                            "offset {} is written by the template and the parent",
                            o + comb_delta
                        ));
                    }
                    v.push((n, idx as u32));
                }
            }
            per_inst.push(v);
        }
        exit_nodes.push(per_inst);
    }
    let mut exits: HashMap<u32, u32> = HashMap::default();
    for per_inst in &exit_nodes {
        for v in per_inst {
            for &(n, _) in v {
                exits.insert(n, 0);
            }
        }
    }
    // A nested template's ports are copied by its parent template, so one
    // instance reads what another writes with no glue statement between
    // them.  Such a read waits for the writer as if a copy were in the glue.
    let mut read_by_instances: HashSet<isize> = HashSet::default();
    for g in groups.iter() {
        for &(_, comb_delta) in &g.deltas {
            read_by_instances.extend(g.io.inputs.iter().map(|&o| o + comb_delta));
        }
    }
    let mut direct: HashMap<isize, u32> = HashMap::default();
    let mut direct_outs: Vec<Vec<Vec<(u32, isize)>>> = Vec::new();
    let mut direct_ins: Vec<Vec<Vec<(u32, isize)>>> = Vec::new();
    let mut writer_of: HashMap<isize, (usize, usize)> = HashMap::default();
    for (gi, g) in groups.iter().enumerate() {
        let mut outs = Vec::new();
        let mut ins = Vec::new();
        for (j, &(_, comb_delta)) in g.deltas.iter().enumerate() {
            let mut o_list = Vec::new();
            for (v, &o) in g.io.vars.iter().enumerate() {
                let at = o + comb_delta;
                if !read_by_instances.contains(&at) {
                    continue;
                }
                if writer_of.insert(at, (gi, j)).is_some_and(|w| w != (gi, j)) {
                    return Err(format!("offset {at} is written by two template instances"));
                }
                o_list.push((v as u32, at));
                direct.insert(at, 0);
            }
            outs.push(o_list);
            ins.push(Vec::new());
        }
        direct_outs.push(outs);
        direct_ins.push(ins);
    }
    for (gi, g) in groups.iter().enumerate() {
        for (j, &(_, comb_delta)) in g.deltas.iter().enumerate() {
            for (k, &o) in g.io.inputs.iter().enumerate() {
                let at = o + comb_delta;
                if direct.contains_key(&at) {
                    direct_ins[gi][j].push((k as u32, at));
                }
            }
        }
    }
    let max_iter = 64;
    for iter in 0..max_iter {
        glue.levels(&exits, &mut level);
        let mut changed = false;
        for (gi, (g, per_inst)) in groups.iter_mut().zip(&exit_nodes).enumerate() {
            g.labels.resize(g.deltas.len(), Vec::new());
            let mut var_level = vec![0u32; g.io.vars.len()];
            let mut in_level = vec![0u32; g.io.inputs.len()];
            for (j, &(_, comb_delta)) in g.deltas.iter().enumerate() {
                for (k, &o) in g.io.inputs.iter().enumerate() {
                    in_level[k] = glue
                        .node_of
                        .get(&(o + comb_delta))
                        .map(|&n| level[n as usize])
                        .unwrap_or(0);
                }
                for &(k, at) in &direct_ins[gi][j] {
                    let slot = &mut in_level[k as usize];
                    *slot = (*slot).max(direct[&at] + 1);
                }
                g.io.label(&in_level, &mut var_level, &mut g.labels[j]);
                for &(n, v) in &per_inst[j] {
                    let l = var_level[v as usize];
                    let slot = exits.get_mut(&n).unwrap();
                    if *slot != l {
                        *slot = l;
                        changed = true;
                    }
                }
                for &(v, at) in &direct_outs[gi][j] {
                    let l = var_level[v as usize];
                    let slot = direct.get_mut(&at).unwrap();
                    if *slot != l {
                        *slot = l;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            if diag() {
                eprintln!("[hier] levels settled after {} iterations", iter + 1);
            }
            return Ok(());
        }
    }
    if let Some(names) = names {
        trace_loop(&glue, groups, &exit_nodes, &exits, &level, names);
    }
    Err(format!(
        "levels did not settle in {max_iter} iterations (a loop through a template)"
    ))
}

/// Diagnostic: walk back from the highest exit along the arguments that set
/// each level, printing the offsets, until an exit repeats.
fn trace_loop(
    glue: &Glue,
    groups: &[Group],
    exit_nodes: &[Vec<Vec<(u32, u32)>>],
    exits: &HashMap<u32, u32>,
    level: &[u32],
    names: &HashMap<VarOffset, String>,
) {
    let name = |o: isize| -> String {
        names
            .get(&VarOffset::Comb(o))
            .cloned()
            .unwrap_or_else(|| format!("comb@{o}"))
    };
    let mut offset_of: Vec<isize> = vec![0; glue.offsets];
    for (&o, &n) in &glue.node_of {
        offset_of[n as usize] = o;
    }
    let mut owner: HashMap<u32, (usize, usize, u32)> = HashMap::default();
    for (gi, per_inst) in exit_nodes.iter().enumerate() {
        for (j, v) in per_inst.iter().enumerate() {
            for &(n, var) in v {
                owner.insert(n, (gi, j, var));
            }
        }
    }
    let Some((&start, _)) = exits.iter().max_by_key(|(_, l)| **l) else {
        return;
    };
    let mut seen: HashSet<u32> = HashSet::default();
    let mut n = start;
    for _ in 0..64 {
        if !seen.insert(n) {
            eprintln!(
                "[hier] trace: loop closes at {}",
                name(offset_of[n as usize])
            );
            return;
        }
        let (gi, j, var) = owner[&n];
        let g = &groups[gi];
        let comb_delta = g.deltas[j].1;
        let l = exits[&n];
        eprintln!(
            "[hier] trace: exit {} level {l}",
            name(offset_of[n as usize])
        );
        // Inside the template: the writer of `var` at this level, back to an input.
        let mut want = (var, l);
        let mut input = None;
        let mut var_level = vec![0u32; g.io.vars.len()];
        let mut in_level = vec![0u32; g.io.inputs.len()];
        for (k, &o) in g.io.inputs.iter().enumerate() {
            in_level[k] = glue
                .node_of
                .get(&(o + comb_delta))
                .map(|&n| level[n as usize])
                .unwrap_or(0);
        }
        let mut labels = Vec::new();
        g.io.label(&in_level, &mut var_level, &mut labels);
        'inner: for _ in 0..100000 {
            let Some(s) = (0..labels.len())
                .rev()
                .find(|&s| labels[s] == want.1 && g.io.writes_var[s].contains(&want.0))
            else {
                break;
            };
            let text = format!("{:?}", g.template.sorted[s]);
            eprintln!(
                "[hier] trace:   template stmt {}",
                text.chars().take(300).collect::<String>()
            );
            if let Some(&k) = g.io.reads_in[s]
                .iter()
                .find(|&&k| in_level[k as usize] == want.1)
            {
                input = Some(k);
                break 'inner;
            }
            match g.io.reads_var[s]
                .iter()
                .find(|&&v| var_level[v as usize] == want.1 && v != want.0)
            {
                Some(&v) => want = (v, want.1),
                None => break,
            }
        }
        let Some(k) = input else {
            eprintln!("[hier] trace: lost inside the template");
            return;
        };
        let o = g.io.inputs[k as usize] + comb_delta;
        eprintln!("[hier] trace:   from input {}", name(o));
        // Through the parent: predecessors at the same level, to an exit.
        let mut m = glue.node_of[&o];
        let mut found = None;
        for _ in 0..100000 {
            let lm = level[m as usize];
            let is_stmt = m as usize >= glue.offsets;
            let next = glue.preds[m as usize].iter().copied().find(|&p| {
                let exit = is_stmt && exits.contains_key(&p);
                level[p as usize] + exit as u32 == lm
            });
            let Some(p) = next else {
                break;
            };
            if is_stmt && exits.contains_key(&p) {
                found = Some(p);
                break;
            }
            if (p as usize) < glue.offsets {
                eprintln!("[hier] trace:   via {}", name(offset_of[p as usize]));
            } else {
                let text = format!("{:?}", glue.stmts[p as usize - glue.offsets]);
                eprintln!(
                    "[hier] trace:   glue stmt {}",
                    text.chars().take(300).collect::<String>()
                );
            }
            m = p;
        }
        match found {
            Some(p) => n = p,
            None => {
                eprintln!("[hier] trace: lost in the parent");
                return;
            }
        }
    }
}
