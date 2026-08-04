//! Sparse region MemorySSA for one independently analyzable procedure.
//!
//! Region endpoints, rather than individual bits or array elements, form the
//! SSA variable domain.  Consequently construction depends on the number of
//! accesses and CFG edges, not on the numerical width of an RTL object.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::cfg::{CfgError, ForwardControlFlowGraph};
use crate::graph::{EdgeKind, IncompleteReason};
use crate::region::{Region, Span};
use crate::ssa::{self, Event as SsaEvent, Version};

pub type ReadId = usize;
pub type WriteId = usize;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MustAliasCandidate {
    pub read: ReadId,
    pub write: WriteId,
    /// Corresponding reads which determine the unresolved selectors. The
    /// candidate is proven only when every pair observes the same SSA version.
    pub selector_reads: Vec<(ReadId, ReadId)>,
}

/// A width-preserving positional transfer from a read region to a write
/// region. Boundaries discovered on either side are propagated to the other;
/// no per-bit expansion is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignedDependency {
    pub read: ReadId,
    pub kind: EdgeKind,
    /// LSB-based span relative to the enclosing read region.
    pub source: Span,
    /// LSB-based span relative to the enclosing write region.
    pub destination: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<O> {
    Read {
        id: ReadId,
        region: Region<O>,
    },
    Write {
        id: WriteId,
        region: Region<O>,
        /// Reads which determine the value, address, or execution of this
        /// write. Observer-only reads must not be listed here.
        dependencies: Vec<(ReadId, EdgeKind)>,
        /// Position-preserving dependencies such as `dst = src`. These are
        /// kept separate from all-to-all value dependencies so a later read
        /// of `dst[0]` depends only on `src[0]`.
        aligned_dependencies: Vec<AlignedDependency>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Procedure<O> {
    pub entry: usize,
    pub exit: usize,
    pub successors: Vec<Vec<usize>>,
    pub events: Vec<Vec<Event<O>>>,
    /// Declared bounds for objects which may be accessed dynamically. Bounds
    /// are partitioned only at observed access endpoints; they are never
    /// expanded into individual bits or elements.
    pub object_spans: BTreeMap<O, Span>,
    /// Syntactically corresponding unresolved accesses. MemorySSA additionally
    /// requires the write to dominate the read and every selector input to
    /// observe the same SSA version before accepting a must-alias proof.
    pub must_alias: BTreeSet<MustAliasCandidate>,
    pub incomplete: BTreeSet<IncompleteReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dependency<O> {
    pub input: Region<O>,
    pub output: Region<O>,
    pub kind: EdgeKind,
    /// Every step from input to output preserves bit position. Call adapters
    /// may safely project a requested output subspan back to the same relative
    /// input subspan only when this is true.
    pub aligned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureSummary<O> {
    pub dependencies: Vec<Dependency<O>>,
    /// Sparse output atoms which can be written on exit, including writes
    /// whose assigned value has no signal dependency.
    pub outputs: Vec<Region<O>>,
    pub incomplete: BTreeSet<IncompleteReason>,
    /// Objects touched through an unresolved region. Exact dependencies on
    /// other objects remain independently provable.
    pub uncertain_objects: BTreeSet<O>,
    /// Original unresolved regions which can source an entry dependency.
    /// These include reads and the retained previous value of a may-write.
    /// MemorySSA expands them to sparse exact atoms internally; adapters use
    /// this set to restore uncertainty without widening a known prefix.
    pub uncertain_input_regions: BTreeSet<Region<O>>,
    /// Exact atom dependency pairs which originated at an unresolved read.
    /// This prevents adapters from widening an unrelated exact read merely
    /// because the same procedure also reads that object dynamically.
    pub uncertain_input_dependencies: BTreeSet<(Region<O>, Region<O>)>,
    /// Objects written through an unresolved region. Adapters use this
    /// narrower set when exact SSA atoms must be projected back to a sparse
    /// object-level destination without conflating dynamic reads with writes.
    pub uncertain_write_objects: BTreeSet<O>,
    /// Original unresolved write regions; see `uncertain_input_regions`.
    pub uncertain_write_regions: BTreeSet<Region<O>>,
    pub unknown_all: bool,
    pub atom_count: usize,
    pub definition_count: usize,
    pub phi_count: usize,
}

#[derive(Debug)]
pub enum ProcedureError {
    Cfg(CfgError),
    Ssa(ssa::SsaError),
    Model(&'static str),
}

impl fmt::Display for ProcedureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cfg(error) => error.fmt(formatter),
            Self::Ssa(error) => error.fmt(formatter),
            Self::Model(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ProcedureError {}

impl From<CfgError> for ProcedureError {
    fn from(value: CfgError) -> Self {
        Self::Cfg(value)
    }
}

impl From<ssa::SsaError> for ProcedureError {
    fn from(value: ssa::SsaError) -> Self {
        Self::Ssa(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Atom<O> {
    object: O,
    span: Span,
}

type AtomIndex<O> = BTreeMap<O, Vec<usize>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Definition {
    write: WriteId,
    atom: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Usage {
    Read {
        read: ReadId,
        atom: usize,
    },
    /// The previous value retained by a may-write when this atom is not the
    /// dynamically selected destination.
    WeakWrite {
        write: WriteId,
        atom: usize,
    },
    Exit {
        atom: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DepVersion<O> {
    version: Version<usize, Definition>,
    kind: EdgeKind,
    aligned: bool,
    uncertain_input: bool,
    active_read: Option<ReadId>,
    marker: std::marker::PhantomData<O>,
}

/// Build a region MemorySSA summary for one procedure.
pub fn analyze<O>(procedure: &Procedure<O>) -> Result<ProcedureSummary<O>, ProcedureError>
where
    O: Copy + Ord,
{
    if procedure.events.len() != procedure.successors.len() {
        return Err(ProcedureError::Model(
            "event and successor tables cover different block domains",
        ));
    }
    if procedure.exit >= procedure.successors.len() {
        return Err(ProcedureError::Model("procedure exit is out of range"));
    }

    let cfg = ForwardControlFlowGraph::analyze(procedure.successors.clone(), procedure.entry)?;
    let (atoms, atoms_by_object) = build_atoms(&procedure.events, &procedure.object_spans)?;
    let mut read_atoms = BTreeMap::<ReadId, (Region<O>, Vec<usize>)>::new();
    let mut writes = BTreeMap::<
        WriteId,
        (
            Region<O>,
            Vec<usize>,
            Vec<(ReadId, EdgeKind)>,
            Vec<AlignedDependency>,
        ),
    >::new();
    let mut incomplete = procedure.incomplete.clone();
    let mut uncertain_objects = BTreeSet::new();
    let mut uncertain_input_regions = BTreeSet::new();
    let mut uncertain_write_objects = BTreeSet::new();
    let mut uncertain_write_regions = BTreeSet::new();
    let mut unknown_all = false;
    let mut ssa_events = vec![Vec::new(); procedure.events.len()];
    let mut read_locations = BTreeMap::new();
    let mut write_locations = BTreeMap::new();

    for (block, events) in procedure.events.iter().enumerate() {
        for (position, event) in events.iter().enumerate() {
            match event {
                Event::Read { id, region } => {
                    read_locations.insert(*id, (block, position));
                    let expanded = expand(*region, &atoms, &atoms_by_object);
                    record_unknown_region(
                        *region,
                        &mut incomplete,
                        &mut uncertain_objects,
                        &mut unknown_all,
                    );
                    if !region.is_exact() {
                        uncertain_input_regions.insert(*region);
                    }
                    if read_atoms
                        .insert(*id, (*region, expanded.clone()))
                        .is_some()
                    {
                        return Err(ProcedureError::Model("duplicate read identity"));
                    }
                    ssa_events[block].extend(expanded.into_iter().map(|atom| SsaEvent::Use {
                        variable: atom,
                        usage: Usage::Read { read: *id, atom },
                    }));
                }
                Event::Write {
                    id,
                    region,
                    dependencies,
                    aligned_dependencies,
                } => {
                    write_locations.insert(*id, (block, position));
                    let expanded = expand(*region, &atoms, &atoms_by_object);
                    record_unknown_region(
                        *region,
                        &mut incomplete,
                        &mut uncertain_objects,
                        &mut unknown_all,
                    );
                    match region {
                        Region::UnknownRegion { object, .. } | Region::UnknownObject(object) => {
                            uncertain_write_objects.insert(*object);
                            uncertain_write_regions.insert(*region);
                            uncertain_input_regions.insert(*region);
                        }
                        Region::Exact { .. } | Region::UnknownAll => {}
                    }
                    if writes
                        .insert(
                            *id,
                            (
                                *region,
                                expanded.clone(),
                                dependencies.clone(),
                                aligned_dependencies.clone(),
                            ),
                        )
                        .is_some()
                    {
                        return Err(ProcedureError::Model("duplicate write identity"));
                    }
                    for atom in expanded {
                        if !region.is_exact() {
                            ssa_events[block].push(SsaEvent::Use {
                                variable: atom,
                                usage: Usage::WeakWrite { write: *id, atom },
                            });
                        }
                        ssa_events[block].push(SsaEvent::Definition {
                            variable: atom,
                            definition: Definition { write: *id, atom },
                        });
                    }
                }
            }
        }
    }

    let written_atoms = writes
        .values()
        .flat_map(|(_, atoms, _, _)| atoms.iter().copied())
        .collect::<BTreeSet<_>>();
    ssa_events[procedure.exit].extend(written_atoms.iter().copied().map(|atom| SsaEvent::Use {
        variable: atom,
        usage: Usage::Exit { atom },
    }));

    let ssa = ssa::build(&cfg, &ssa_events)?;
    let must_alias = procedure
        .must_alias
        .iter()
        .filter_map(|candidate| {
            let &(write_block, write_position) = write_locations.get(&candidate.write)?;
            let &(read_block, read_position) = read_locations.get(&candidate.read)?;
            let ordered = if write_block == read_block {
                write_position < read_position
            } else {
                cfg.dominators.dominates(write_block, read_block)
            };
            if !ordered {
                return None;
            }
            let same_versions =
                candidate
                    .selector_reads
                    .iter()
                    .all(|&(write_selector, read_selector)| {
                        let Some((_, write_atoms)) = read_atoms.get(&write_selector) else {
                            return false;
                        };
                        let Some((_, read_atoms)) = read_atoms.get(&read_selector) else {
                            return false;
                        };
                        write_atoms.len() == read_atoms.len()
                            && write_atoms.iter().zip(read_atoms).all(
                                |(&write_atom, &read_atom)| {
                                    ssa.uses.get(&Usage::Read {
                                        read: write_selector,
                                        atom: write_atom,
                                    }) == ssa.uses.get(&Usage::Read {
                                        read: read_selector,
                                        atom: read_atom,
                                    })
                                },
                            )
                    });
            same_versions.then_some((candidate.read, candidate.write))
        })
        .collect::<BTreeSet<_>>();
    let definitions = writes.values().map(|(_, atoms, _, _)| atoms.len()).sum();
    let mut version_deps = BTreeMap::<
        Version<usize, Definition>,
        BTreeSet<(
            Version<usize, Definition>,
            EdgeKind,
            bool,
            bool,
            Option<ReadId>,
            bool,
            bool,
        )>,
    >::new();

    for phi in &ssa.phis {
        let deps = version_deps.entry(phi.version).or_default();
        deps.extend(
            phi.inputs
                .iter()
                .map(|(_, input)| (*input, EdgeKind::Value, true, false, None, true, false)),
        );
    }
    for (&write, (write_region, write_atoms, dependencies, aligned_dependencies)) in &writes {
        let mut incoming = BTreeSet::new();
        for &(read, kind) in dependencies {
            let Some((read_region, atoms)) = read_atoms.get(&read) else {
                return Err(ProcedureError::Model(
                    "write dependency refers to an unknown read",
                ));
            };
            for &atom in atoms {
                if let Some(&version) = ssa.uses.get(&Usage::Read { read, atom }) {
                    incoming.insert((
                        version,
                        kind,
                        false,
                        !read_region.is_exact(),
                        Some(read),
                        false,
                        false,
                    ));
                }
            }
        }
        for &atom in write_atoms {
            let mut atom_incoming = incoming.clone();
            if !write_region.is_exact()
                && let Some(&previous) = ssa.uses.get(&Usage::WeakWrite { write, atom })
            {
                atom_incoming.insert((previous, EdgeKind::Value, true, true, None, true, true));
            }
            for dependency in aligned_dependencies {
                let Some((read_region, source_atoms)) = read_atoms.get(&dependency.read) else {
                    return Err(ProcedureError::Model(
                        "aligned dependency refers to an unknown read",
                    ));
                };
                let (
                    Region::Exact {
                        span: source_span, ..
                    },
                    Region::Exact {
                        span: destination_span,
                        ..
                    },
                ) = (*read_region, *write_region)
                else {
                    incomplete.insert(IncompleteReason::DynamicRegion);
                    continue;
                };
                if dependency.source.length != dependency.destination.length
                    || dependency.source.end().is_none()
                    || dependency.source.end() > Some(source_span.length)
                    || dependency.destination.end().is_none()
                    || dependency.destination.end() > Some(destination_span.length)
                {
                    return Err(ProcedureError::Model(
                        "aligned dependency does not fit its source and destination",
                    ));
                }
                let Some(source_start) = source_span.start.checked_add(dependency.source.start)
                else {
                    return Err(ProcedureError::Model("aligned transfer overflows usize"));
                };
                let Some(destination_start) = destination_span
                    .start
                    .checked_add(dependency.destination.start)
                else {
                    return Err(ProcedureError::Model("aligned transfer overflows usize"));
                };
                let transfer_source = Span {
                    start: source_start,
                    length: dependency.source.length,
                };
                let transfer_destination = Span {
                    start: destination_start,
                    length: dependency.destination.length,
                };
                let output_atom = atoms[atom].span;
                let Some(overlap) = output_atom.intersection(transfer_destination) else {
                    continue;
                };
                let Some(relative_start) = overlap.start.checked_sub(transfer_destination.start)
                else {
                    return Err(ProcedureError::Model(
                        "write atom begins before its aligned transfer",
                    ));
                };
                let Some(mapped_start) = transfer_source.start.checked_add(relative_start) else {
                    return Err(ProcedureError::Model("aligned transfer overflows usize"));
                };
                let mapped = Span {
                    start: mapped_start,
                    length: overlap.length,
                };
                for &source_atom in source_atoms {
                    if atoms[source_atom].span.intersection(mapped).is_some()
                        && let Some(&version) = ssa.uses.get(&Usage::Read {
                            read: dependency.read,
                            atom: source_atom,
                        })
                    {
                        atom_incoming.insert((
                            version,
                            dependency.kind,
                            true,
                            false,
                            Some(dependency.read),
                            false,
                            false,
                        ));
                    }
                }
            }
            version_deps.insert(
                Version::Definition {
                    variable: atom,
                    definition: Definition { write, atom },
                },
                atom_incoming,
            );
        }
    }

    let mut dependencies = BTreeSet::new();
    let mut uncertain_input_dependencies = BTreeSet::new();
    for &output_atom in &written_atoms {
        let Some(&output_version) = ssa.uses.get(&Usage::Exit { atom: output_atom }) else {
            continue;
        };
        let mut stack = vec![(output_version, EdgeKind::Value, true, false, None)];
        let mut visited = BTreeSet::new();
        while let Some((version, path_kind, path_aligned, path_uncertain_input, active_read)) =
            stack.pop()
        {
            if !visited.insert(DepVersion::<O> {
                version,
                kind: path_kind,
                aligned: path_aligned,
                uncertain_input: path_uncertain_input,
                active_read,
                marker: std::marker::PhantomData,
            }) {
                continue;
            }
            if let Version::Entry(input_atom) = version {
                let dependency = Dependency {
                    input: atom_region(atoms[input_atom]),
                    output: atom_region(atoms[output_atom]),
                    kind: path_kind,
                    aligned: path_aligned,
                };
                dependencies.insert(dependency);
                if path_uncertain_input {
                    uncertain_input_dependencies.insert((dependency.input, dependency.output));
                }
                continue;
            }
            let current_write = match version {
                Version::Definition { definition, .. } => Some(definition.write),
                Version::Entry(_) | Version::Phi { .. } => None,
            };
            if let Some(inputs) = version_deps.get(&version) {
                for &(
                    input,
                    kind,
                    aligned,
                    uncertain_input,
                    source_read,
                    preserve_active_read,
                    weak_previous,
                ) in inputs
                {
                    if weak_previous {
                        let Some(read) = active_read else {
                            // Retention by itself is not value feedback. It
                            // becomes observable only through a later read of
                            // a possibly unselected candidate.
                            continue;
                        };
                        if current_write.is_some_and(|write| must_alias.contains(&(read, write))) {
                            continue;
                        }
                    }
                    stack.push((
                        input,
                        combine_edge_kinds(path_kind, kind),
                        path_aligned && aligned,
                        path_uncertain_input || uncertain_input,
                        if preserve_active_read {
                            active_read
                        } else {
                            source_read
                        },
                    ));
                }
            }
        }
    }

    let mut collapsed_dependencies = BTreeMap::new();
    for dependency in dependencies {
        collapsed_dependencies
            .entry((dependency.input, dependency.output, dependency.kind))
            .and_modify(|aligned| *aligned &= dependency.aligned)
            .or_insert(dependency.aligned);
    }

    Ok(ProcedureSummary {
        dependencies: collapsed_dependencies
            .into_iter()
            .map(|((input, output, kind), aligned)| Dependency {
                input,
                output,
                kind,
                aligned,
            })
            .collect(),
        outputs: written_atoms
            .iter()
            .copied()
            .map(|atom| atom_region(atoms[atom]))
            .collect(),
        incomplete,
        uncertain_objects,
        uncertain_input_regions,
        uncertain_input_dependencies,
        uncertain_write_objects,
        uncertain_write_regions,
        unknown_all,
        atom_count: atoms.len(),
        definition_count: definitions,
        phi_count: ssa.phis.len(),
    })
}

fn record_unknown_region<O: Copy + Ord>(
    region: Region<O>,
    incomplete: &mut BTreeSet<IncompleteReason>,
    uncertain_objects: &mut BTreeSet<O>,
    unknown_all: &mut bool,
) {
    match region {
        Region::Exact { .. } => {}
        Region::UnknownRegion { object, .. } | Region::UnknownObject(object) => {
            incomplete.insert(IncompleteReason::DynamicRegion);
            uncertain_objects.insert(object);
        }
        Region::UnknownAll => {
            incomplete.insert(IncompleteReason::DynamicRegion);
            *unknown_all = true;
        }
    }
}

fn combine_edge_kinds(outer: EdgeKind, inner: EdgeKind) -> EdgeKind {
    if outer == EdgeKind::Unknown || inner == EdgeKind::Unknown {
        EdgeKind::Unknown
    } else if outer == EdgeKind::Control || inner == EdgeKind::Control {
        EdgeKind::Control
    } else if outer == EdgeKind::Address || inner == EdgeKind::Address {
        EdgeKind::Address
    } else {
        EdgeKind::Value
    }
}

fn atom_region<O>(atom: Atom<O>) -> Region<O> {
    Region::Exact {
        object: atom.object,
        span: atom.span,
    }
}

fn build_atoms<O: Copy + Ord>(
    blocks: &[Vec<Event<O>>],
    object_spans: &BTreeMap<O, Span>,
) -> Result<(Vec<Atom<O>>, AtomIndex<O>), ProcedureError> {
    let mut endpoints = BTreeMap::<O, BTreeMap<usize, i64>>::new();
    let mut read_regions = BTreeMap::<ReadId, Region<O>>::new();
    let mut dynamic_regions = BTreeSet::new();
    for event in blocks.iter().flatten() {
        if let Event::Read { id, region } = event
            && read_regions.insert(*id, *region).is_some()
        {
            return Err(ProcedureError::Model("duplicate read identity"));
        }
        let region = match event {
            Event::Read { region, .. } | Event::Write { region, .. } => *region,
        };
        if matches!(
            region,
            Region::UnknownRegion { .. } | Region::UnknownObject(_)
        ) {
            dynamic_regions.insert(region);
        }
        let Region::Exact { object, span } = region else {
            continue;
        };
        let Some(end) = span.end() else {
            return Err(ProcedureError::Model("region end overflows usize"));
        };
        if span.length == 0 {
            return Err(ProcedureError::Model("empty region"));
        }
        *endpoints
            .entry(object)
            .or_default()
            .entry(span.start)
            .or_default() += 1;
        *endpoints.entry(object).or_default().entry(end).or_default() -= 1;
    }

    for region in dynamic_regions {
        let (object, span) = match region {
            Region::UnknownRegion { object, span } => (object, span),
            Region::UnknownObject(object) => {
                let Some(span) = object_spans.get(&object).copied() else {
                    continue;
                };
                (object, span)
            }
            Region::Exact { .. } | Region::UnknownAll => continue,
        };
        let Some(end) = span.end() else {
            return Err(ProcedureError::Model("object span end overflows usize"));
        };
        if span.length == 0 {
            return Err(ProcedureError::Model("empty object span"));
        }
        *endpoints
            .entry(object)
            .or_default()
            .entry(span.start)
            .or_default() += 1;
        *endpoints.entry(object).or_default().entry(end).or_default() -= 1;
    }

    #[derive(Clone, Copy)]
    struct Transfer<O> {
        source_object: O,
        source_span: Span,
        destination_object: O,
        destination_span: Span,
    }

    let mut transfers = Vec::<Transfer<O>>::new();
    for event in blocks.iter().flatten() {
        let Event::Write {
            region: destination,
            aligned_dependencies,
            ..
        } = event
        else {
            continue;
        };
        let Region::Exact {
            object: destination_object,
            span: destination_span,
        } = *destination
        else {
            continue;
        };
        for dependency in aligned_dependencies {
            let Some(Region::Exact {
                object: source_object,
                span: source_span,
            }) = read_regions.get(&dependency.read).copied()
            else {
                continue;
            };
            if dependency.source.length != dependency.destination.length
                || dependency.source.end().is_none()
                || dependency.source.end() > Some(source_span.length)
                || dependency.destination.end().is_none()
                || dependency.destination.end() > Some(destination_span.length)
            {
                return Err(ProcedureError::Model(
                    "aligned dependency regions have different lengths",
                ));
            }
            let Some(source_start) = source_span.start.checked_add(dependency.source.start) else {
                return Err(ProcedureError::Model("aligned transfer overflows usize"));
            };
            let Some(destination_start) = destination_span
                .start
                .checked_add(dependency.destination.start)
            else {
                return Err(ProcedureError::Model("aligned transfer overflows usize"));
            };
            let mapped_source = Span {
                start: source_start,
                length: dependency.source.length,
            };
            let mapped_destination = Span {
                start: destination_start,
                length: dependency.destination.length,
            };
            transfers.push(Transfer {
                source_object,
                source_span: mapped_source,
                destination_object,
                destination_span: mapped_destination,
            });
        }
    }

    // Copy relationships form a sparse coordinate graph. Propagate only
    // observed access boundaries through it, so even million-bit objects cost
    // O(accesses + propagated endpoints), never O(width).
    let mut transfer_index = BTreeMap::<O, Vec<(usize, bool)>>::new();
    for (index, transfer) in transfers.iter().enumerate() {
        transfer_index
            .entry(transfer.source_object)
            .or_default()
            .push((index, true));
        transfer_index
            .entry(transfer.destination_object)
            .or_default()
            .push((index, false));
    }
    let mut pending = std::collections::VecDeque::new();
    for (&object, points) in &endpoints {
        pending.extend(points.keys().map(|&point| (object, point)));
    }
    while let Some((object, point)) = pending.pop_front() {
        for &(index, from_source) in transfer_index.get(&object).into_iter().flatten() {
            let transfer = transfers[index];
            let (from, to_object, to) = if from_source {
                (
                    transfer.source_span,
                    transfer.destination_object,
                    transfer.destination_span,
                )
            } else {
                (
                    transfer.destination_span,
                    transfer.source_object,
                    transfer.source_span,
                )
            };
            let Some(from_end) = from.end() else {
                return Err(ProcedureError::Model("aligned transfer overflows usize"));
            };
            if point < from.start || point > from_end {
                continue;
            }
            let Some(mapped) = point
                .checked_sub(from.start)
                .and_then(|offset| to.start.checked_add(offset))
            else {
                return Err(ProcedureError::Model("aligned transfer overflows usize"));
            };
            let inserted = match endpoints.entry(to_object).or_default().entry(mapped) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(0);
                    true
                }
                std::collections::btree_map::Entry::Occupied(_) => false,
            };
            if inserted {
                pending.push_back((to_object, mapped));
            }
        }
    }

    let mut atoms = Vec::new();
    let mut atoms_by_object = BTreeMap::<O, Vec<usize>>::new();
    for (object, points) in endpoints {
        let mut active = 0i64;
        let mut previous = None;
        for (point, delta) in points {
            if let Some(start) = previous
                && active > 0
                && start < point
            {
                let atom = atoms.len();
                atoms.push(Atom {
                    object,
                    span: Span {
                        start,
                        length: point - start,
                    },
                });
                atoms_by_object.entry(object).or_default().push(atom);
            }
            active += delta;
            previous = Some(point);
        }
    }
    Ok((atoms, atoms_by_object))
}

fn expand<O: Copy + Ord>(
    region: Region<O>,
    atoms: &[Atom<O>],
    atoms_by_object: &BTreeMap<O, Vec<usize>>,
) -> Vec<usize> {
    let (object, span) = match region {
        Region::Exact { object, span } => (object, Some(span)),
        Region::UnknownRegion { object, span } => (object, Some(span)),
        Region::UnknownObject(object) => (object, None),
        Region::UnknownAll => return Vec::new(),
    };
    atoms_by_object
        .get(&object)
        .map(|object_atoms| {
            object_atoms
                .iter()
                .copied()
                .filter(|&atom| {
                    span.is_none_or(|span| atoms[atom].span.intersection(span).is_some())
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(object: u8, start: usize, length: usize) -> Region<u8> {
        Region::Exact {
            object,
            span: Span { start, length },
        }
    }

    #[test]
    fn sequential_overwrite_kills_the_entry_dependency() {
        let procedure: Procedure<u8> = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Write {
                    id: 0,
                    region: exact(1, 0, 8),
                    dependencies: vec![],
                    aligned_dependencies: vec![],
                },
                Event::Read {
                    id: 0,
                    region: exact(1, 0, 8),
                },
                Event::Write {
                    id: 1,
                    region: exact(1, 0, 8),
                    dependencies: vec![(0, EdgeKind::Value)],
                    aligned_dependencies: vec![],
                },
            ]],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };
        let summary = analyze(&procedure).unwrap();
        assert!(summary.dependencies.is_empty());
    }

    #[test]
    fn branch_merge_retains_only_the_live_entry_arm() {
        let procedure = Procedure {
            entry: 0,
            exit: 3,
            successors: vec![vec![1, 2], vec![3], vec![3], vec![]],
            events: vec![
                vec![],
                vec![Event::Write {
                    id: 0,
                    region: exact(1, 0, 1),
                    dependencies: vec![],
                    aligned_dependencies: vec![],
                }],
                vec![],
                vec![],
            ],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };
        let summary = analyze(&procedure).unwrap();
        assert_eq!(
            summary.dependencies,
            vec![Dependency {
                input: exact(1, 0, 1),
                output: exact(1, 0, 1),
                kind: EdgeKind::Value,
                aligned: true,
            }]
        );
        assert_eq!(summary.phi_count, 1);
    }

    #[test]
    fn cost_is_independent_of_numerical_width() {
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Read {
                    id: 0,
                    region: exact(1, 0, 1 << 30),
                },
                Event::Write {
                    id: 0,
                    region: exact(2, 0, 1 << 30),
                    dependencies: vec![(0, EdgeKind::Value)],
                    aligned_dependencies: vec![],
                },
            ]],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };
        let summary = analyze(&procedure).unwrap();
        assert_eq!(summary.atom_count, 2);
        assert_eq!(summary.dependencies.len(), 1);
    }

    #[test]
    fn aligned_copy_propagates_sparse_boundaries_without_cross_bit_taint() {
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Read {
                    id: 0,
                    region: exact(1, 0, 1 << 30),
                },
                Event::Write {
                    id: 0,
                    region: exact(2, 0, 1 << 30),
                    dependencies: vec![],
                    aligned_dependencies: vec![AlignedDependency {
                        read: 0,
                        kind: EdgeKind::Value,
                        source: Span {
                            start: 0,
                            length: 1 << 30,
                        },
                        destination: Span {
                            start: 0,
                            length: 1 << 30,
                        },
                    }],
                },
                Event::Read {
                    id: 1,
                    region: exact(2, 7, 1),
                },
                Event::Write {
                    id: 1,
                    region: exact(3, 0, 1),
                    dependencies: vec![(1, EdgeKind::Value)],
                    aligned_dependencies: vec![],
                },
            ]],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };

        let summary = analyze(&procedure).unwrap();
        assert_eq!(summary.atom_count, 7);
        let final_dependencies = summary
            .dependencies
            .iter()
            .copied()
            .filter(|dependency| dependency.output == exact(3, 0, 1))
            .collect::<Vec<_>>();
        assert_eq!(
            final_dependencies,
            vec![Dependency {
                input: exact(1, 7, 1),
                output: exact(3, 0, 1),
                kind: EdgeKind::Value,
                aligned: false,
            }]
        );
    }

    #[test]
    fn object_local_uncertainty_preserves_other_exact_dependencies() {
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Write {
                    id: 0,
                    region: Region::UnknownObject(1),
                    dependencies: vec![],
                    aligned_dependencies: vec![],
                },
                Event::Read {
                    id: 0,
                    region: exact(2, 0, 1),
                },
                Event::Write {
                    id: 1,
                    region: exact(2, 0, 1),
                    dependencies: vec![(0, EdgeKind::Value)],
                    aligned_dependencies: vec![],
                },
            ]],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };

        let summary = analyze(&procedure).unwrap();
        assert_eq!(summary.uncertain_objects, BTreeSet::from([1]));
        assert_eq!(summary.uncertain_write_objects, BTreeSet::from([1]));
        assert!(!summary.unknown_all);
        assert_eq!(
            summary.dependencies,
            vec![Dependency {
                input: exact(2, 0, 1),
                output: exact(2, 0, 1),
                kind: EdgeKind::Value,
                aligned: false,
            }]
        );
    }

    #[test]
    fn unknown_static_prefix_cost_is_independent_of_span_length() {
        // Why this case exists: a dynamic suffix may be confined to a very
        // large static array prefix. The causal graph must retain that prefix
        // as one interval rather than allocate one atom per possible element.
        let prefix = Region::UnknownRegion {
            object: 1u8,
            span: Span {
                start: 1 << 40,
                length: 1 << 39,
            },
        };
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Read {
                    id: 0,
                    region: prefix,
                },
                Event::Write {
                    id: 0,
                    region: prefix,
                    dependencies: vec![(0, EdgeKind::Value)],
                    aligned_dependencies: vec![],
                },
            ]],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };

        let summary = analyze(&procedure).unwrap();
        assert_eq!(summary.atom_count, 1);
        assert_eq!(summary.definition_count, 1);
        assert_eq!(summary.uncertain_input_regions, BTreeSet::from([prefix]));
        assert_eq!(summary.uncertain_write_regions, BTreeSet::from([prefix]));
    }

    #[test]
    fn dynamic_read_does_not_mark_object_as_dynamically_written() {
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![Event::Read {
                id: 0,
                region: Region::UnknownObject(1),
            }]],
            object_spans: BTreeMap::from([(
                1,
                Span {
                    start: 0,
                    length: 8,
                },
            )]),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };

        let summary = analyze(&procedure).unwrap();
        assert_eq!(summary.uncertain_objects, BTreeSet::from([1]));
        assert!(summary.uncertain_write_objects.is_empty());
    }

    #[test]
    fn dynamic_self_dependency_remains_structurally_visible() {
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Read {
                    id: 0,
                    region: Region::UnknownObject(1),
                },
                Event::Write {
                    id: 0,
                    region: Region::UnknownObject(1),
                    dependencies: vec![(0, EdgeKind::Value)],
                    aligned_dependencies: vec![],
                },
            ]],
            object_spans: BTreeMap::from([(
                1,
                Span {
                    start: 0,
                    length: 8,
                },
            )]),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };

        let summary = analyze(&procedure).unwrap();
        assert_eq!(
            summary.dependencies,
            vec![Dependency {
                input: exact(1, 0, 8),
                output: exact(1, 0, 8),
                kind: EdgeKind::Value,
                aligned: false,
            }]
        );
    }

    #[test]
    fn global_uncertainty_is_reported_separately() {
        let procedure: Procedure<u8> = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![Event::Write {
                id: 0,
                region: Region::UnknownAll,
                dependencies: vec![],
                aligned_dependencies: vec![],
            }]],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };

        let summary = analyze(&procedure).unwrap();
        assert!(summary.unknown_all);
        assert!(summary.uncertain_objects.is_empty());
        assert!(
            summary
                .incomplete
                .contains(&IncompleteReason::DynamicRegion)
        );
    }
}
