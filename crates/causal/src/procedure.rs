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
    /// Number of copies of this transfer. A value greater than one represents
    /// the same source span at regularly spaced destination positions without
    /// materializing one dependency per copy.
    pub repetitions: usize,
    /// Distance in bits between consecutive destination copies. Ignored when
    /// `repetitions == 1`.
    pub destination_stride: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeriodicAxis {
    pub repetitions: usize,
    pub destination_stride: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeriodicDependency<O> {
    pub input: Region<O>,
    pub output_object: O,
    /// The first output copy influenced by `input`.
    pub output: Span,
    /// Cartesian repetition axes, ordered from the innermost to the
    /// outermost transfer. Representation size depends on nesting depth, not
    /// on the number of copies along any axis.
    pub axes: Vec<PeriodicAxis>,
    pub kind: EdgeKind,
    /// Each output copy preserves positions within `input`.
    pub aligned: bool,
    /// The first concrete write encountered from the procedure exit toward
    /// this input. `None` denotes retained entry state rather than an
    /// assignment which can anchor a source diagnostic.
    pub origin: Option<WriteId>,
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
    /// The first concrete write encountered from the procedure exit toward
    /// this input. Distinct reaching assignments remain distinct dependencies
    /// so adapters can attach an exact source location to each causal edge.
    pub origin: Option<WriteId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureSummary<O> {
    pub dependencies: Vec<Dependency<O>>,
    /// Regular one-to-many transfers retained without expanding every copy.
    pub periodic_dependencies: Vec<PeriodicDependency<O>>,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DepVersion<O> {
    version: Version<usize, Definition>,
    kind: EdgeKind,
    aligned: bool,
    uncertain_input: bool,
    active_read: Option<ReadId>,
    translation: Option<i128>,
    periodic_output: Option<PeriodicProjection>,
    origin: Option<WriteId>,
    marker: std::marker::PhantomData<O>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PeriodicProjection {
    source: Span,
    output: Span,
    axes: Vec<PeriodicAxis>,
}

fn normalize_periodic_axes(axes: &mut Vec<PeriodicAxis>) -> Option<()> {
    if axes.iter().any(|axis| axis.repetitions == 0) {
        return None;
    }
    axes.retain(|axis| axis.repetitions > 1);
    let mut normalized: Vec<PeriodicAxis> = Vec::with_capacity(axes.len());
    for axis in axes.drain(..) {
        if axis.destination_stride == 0 {
            return None;
        }
        if let Some(inner) = normalized.last_mut()
            && inner.destination_stride.checked_mul(inner.repetitions)
                == Some(axis.destination_stride)
        {
            inner.repetitions = inner.repetitions.checked_mul(axis.repetitions)?;
            continue;
        }
        normalized.push(axis);
    }
    *axes = normalized;
    Some(())
}

fn periodic_extent(length: usize, axes: &[PeriodicAxis]) -> Option<usize> {
    axes.iter().try_fold(length, |extent, axis| {
        if axis.repetitions == 0 || axis.destination_stride < extent {
            return None;
        }
        axis.destination_stride
            .checked_mul(axis.repetitions - 1)
            .and_then(|offset| extent.checked_add(offset))
    })
}

fn periodic_end(output: Span, axes: &[PeriodicAxis]) -> Option<usize> {
    output
        .start
        .checked_add(periodic_extent(output.length, axes)?)
}

fn compose_periodic_projections(
    inner: &PeriodicProjection,
    outer: &PeriodicProjection,
) -> Option<PeriodicProjection> {
    let relative = inner.output.start.checked_sub(outer.source.start)?;
    if periodic_end(inner.output, &inner.axes)? > outer.source.end()? {
        return None;
    }
    let start = outer.output.start.checked_add(relative)?;
    let mut axes = Vec::with_capacity(inner.axes.len().checked_add(outer.axes.len())?);
    axes.extend_from_slice(&inner.axes);
    axes.extend_from_slice(&outer.axes);
    normalize_periodic_axes(&mut axes)?;
    periodic_extent(inner.output.length, &axes)?;
    Some(PeriodicProjection {
        source: inner.source,
        output: Span {
            start,
            length: inner.output.length,
        },
        axes,
    })
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
            Option<i128>,
            Option<PeriodicProjection>,
        )>,
    >::new();

    for phi in &ssa.phis {
        let deps = version_deps.entry(phi.version).or_default();
        deps.extend(phi.inputs.iter().map(|(_, input)| {
            (
                *input,
                EdgeKind::Value,
                true,
                false,
                None,
                true,
                false,
                Some(0),
                None,
            )
        }));
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
                        None,
                        None,
                    ));
                }
            }
        }
        for &atom in write_atoms {
            let mut atom_incoming = incoming.clone();
            if !write_region.is_exact()
                && let Some(&previous) = ssa.uses.get(&Usage::WeakWrite { write, atom })
            {
                atom_incoming.insert((
                    previous,
                    EdgeKind::Value,
                    true,
                    true,
                    None,
                    true,
                    true,
                    Some(0),
                    None,
                ));
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
                    || dependency.repetitions == 0
                    || dependency.source.end().is_none()
                    || dependency.source.end() > Some(source_span.length)
                    || dependency.destination.end().is_none()
                    || dependency
                        .destination_stride
                        .checked_mul(dependency.repetitions.saturating_sub(1))
                        .and_then(|offset| dependency.destination.end()?.checked_add(offset))
                        > Some(destination_span.length)
                    || (dependency.repetitions > 1
                        && dependency.destination_stride < dependency.destination.length)
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
                if dependency.repetitions > 1 {
                    let Some(output_end) = output_atom.end() else {
                        return Err(ProcedureError::Model("output atom overflows usize"));
                    };
                    for &source_atom in source_atoms {
                        let Some(source_overlap) =
                            atoms[source_atom].span.intersection(transfer_source)
                        else {
                            continue;
                        };
                        let Some(relative) =
                            source_overlap.start.checked_sub(transfer_source.start)
                        else {
                            continue;
                        };
                        let Some(phase_start) = transfer_destination.start.checked_add(relative)
                        else {
                            return Err(ProcedureError::Model("periodic transfer overflows usize"));
                        };
                        let phase = Span {
                            start: phase_start,
                            length: source_overlap.length,
                        };
                        let Some(phase_end) = phase.end() else {
                            return Err(ProcedureError::Model("periodic transfer overflows usize"));
                        };
                        let first = if output_atom.start < phase_end {
                            0
                        } else {
                            output_atom
                                .start
                                .checked_sub(phase_end)
                                .map(|distance| distance / dependency.destination_stride + 1)
                                .unwrap_or(0)
                        };
                        let end = if output_end <= phase.start {
                            0
                        } else {
                            output_end
                                .checked_sub(phase.start)
                                .and_then(|distance| {
                                    distance.checked_add(dependency.destination_stride - 1)
                                })
                                .map(|distance| distance / dependency.destination_stride)
                                .unwrap_or(dependency.repetitions)
                        }
                        .min(dependency.repetitions);
                        if first >= end {
                            continue;
                        }
                        let Some(first_start) = first
                            .checked_mul(dependency.destination_stride)
                            .and_then(|offset| phase.start.checked_add(offset))
                        else {
                            return Err(ProcedureError::Model("periodic transfer overflows usize"));
                        };
                        let projection = PeriodicProjection {
                            source: source_overlap,
                            output: Span {
                                start: first_start,
                                length: phase.length,
                            },
                            axes: vec![PeriodicAxis {
                                repetitions: end - first,
                                destination_stride: dependency.destination_stride,
                            }],
                        };
                        if let Some(&version) = ssa.uses.get(&Usage::Read {
                            read: dependency.read,
                            atom: source_atom,
                        }) {
                            atom_incoming.insert((
                                version,
                                dependency.kind,
                                true,
                                false,
                                Some(dependency.read),
                                false,
                                false,
                                None,
                                Some(projection),
                            ));
                        }
                    }
                    continue;
                }
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
                            Some(overlap.start as i128 - mapped.start as i128),
                            None,
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
    let mut periodic_dependencies = BTreeSet::new();
    let mut uncertain_input_dependencies = BTreeSet::new();
    for &output_atom in &written_atoms {
        let Some(&output_version) = ssa.uses.get(&Usage::Exit { atom: output_atom }) else {
            continue;
        };
        let mut stack = vec![(
            output_version,
            EdgeKind::Value,
            true,
            false,
            None,
            Some(0i128),
            None,
            None,
        )];
        let mut visited = BTreeSet::new();
        while let Some((
            version,
            path_kind,
            path_aligned,
            path_uncertain_input,
            active_read,
            path_translation,
            path_periodic_output,
            path_origin,
        )) = stack.pop()
        {
            if !visited.insert(DepVersion::<O> {
                version,
                kind: path_kind,
                aligned: path_aligned,
                uncertain_input: path_uncertain_input,
                active_read,
                translation: path_translation,
                periodic_output: path_periodic_output.clone(),
                origin: path_origin,
                marker: std::marker::PhantomData,
            }) {
                continue;
            }
            if let Version::Entry(input_atom) = version {
                let input = atom_region(atoms[input_atom]);
                if let Some(periodic) = path_periodic_output {
                    periodic_dependencies.insert(PeriodicDependency {
                        input,
                        output_object: atoms[output_atom].object,
                        output: periodic.output,
                        axes: periodic.axes,
                        kind: path_kind,
                        aligned: path_aligned,
                        origin: path_origin,
                    });
                } else {
                    let dependency = Dependency {
                        input,
                        output: atom_region(atoms[output_atom]),
                        kind: path_kind,
                        aligned: path_aligned,
                        origin: path_origin,
                    };
                    dependencies.insert(dependency);
                    if path_uncertain_input {
                        uncertain_input_dependencies.insert((dependency.input, dependency.output));
                    }
                }
                continue;
            }
            let current_write = match version {
                Version::Definition { definition, .. } => Some(definition.write),
                Version::Entry(_) | Version::Phi { .. } => None,
            };
            let next_origin = path_origin.or(current_write);
            if let Some(inputs) = version_deps.get(&version) {
                for (
                    input,
                    kind,
                    aligned,
                    uncertain_input,
                    source_read,
                    preserve_active_read,
                    weak_previous,
                    translation,
                    periodic_output,
                ) in inputs
                {
                    let input = *input;
                    let kind = *kind;
                    let aligned = *aligned;
                    let uncertain_input = *uncertain_input;
                    let source_read = *source_read;
                    let preserve_active_read = *preserve_active_read;
                    let weak_previous = *weak_previous;
                    let translation = *translation;
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
                    let mut next_aligned = path_aligned && aligned;
                    let (next_translation, next_periodic_output) =
                        if let Some(periodic) = periodic_output {
                            if let Some(outer) = &path_periodic_output {
                                let Some(composed) = compose_periodic_projections(periodic, outer)
                                else {
                                    continue;
                                };
                                (None, Some(composed))
                            } else if let Some(offset) = path_translation {
                                let shifted = if offset >= 0 {
                                    periodic.output.start.checked_add(offset as usize)
                                } else {
                                    periodic.output.start.checked_sub((-offset) as usize)
                                };
                                let Some(start) = shifted else {
                                    continue;
                                };
                                (
                                    None,
                                    Some(PeriodicProjection {
                                        source: periodic.source,
                                        output: Span {
                                            start,
                                            length: periodic.output.length,
                                        },
                                        axes: periodic.axes.clone(),
                                    }),
                                )
                            } else {
                                next_aligned = false;
                                (None, None)
                            }
                        } else if path_periodic_output.is_some() {
                            (None, path_periodic_output.clone())
                        } else {
                            (
                                path_translation.and_then(|path| {
                                    translation.and_then(|edge| path.checked_add(edge))
                                }),
                                None,
                            )
                        };
                    stack.push((
                        input,
                        combine_edge_kinds(path_kind, kind),
                        next_aligned,
                        path_uncertain_input || uncertain_input,
                        if preserve_active_read {
                            active_read
                        } else {
                            source_read
                        },
                        next_translation,
                        next_periodic_output,
                        next_origin,
                    ));
                }
            }
        }
    }

    let mut collapsed_dependencies = BTreeMap::new();
    for dependency in dependencies {
        collapsed_dependencies
            .entry((
                dependency.input,
                dependency.output,
                dependency.kind,
                dependency.origin,
            ))
            .and_modify(|aligned| *aligned &= dependency.aligned)
            .or_insert(dependency.aligned);
    }

    Ok(ProcedureSummary {
        dependencies: collapsed_dependencies
            .into_iter()
            .map(|((input, output, kind, origin), aligned)| Dependency {
                input,
                output,
                kind,
                aligned,
                origin,
            })
            .collect(),
        periodic_dependencies: periodic_dependencies.into_iter().collect(),
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
        repetitions: usize,
        destination_stride: usize,
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
                || dependency.repetitions == 0
                || dependency.source.end().is_none()
                || dependency.source.end() > Some(source_span.length)
                || dependency.destination.end().is_none()
                || dependency
                    .destination_stride
                    .checked_mul(dependency.repetitions.saturating_sub(1))
                    .and_then(|offset| dependency.destination.end()?.checked_add(offset))
                    > Some(destination_span.length)
                || (dependency.repetitions > 1
                    && dependency.destination_stride < dependency.destination.length)
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
                repetitions: dependency.repetitions,
                destination_stride: dependency.destination_stride,
            });
        }
    }

    // Copy relationships form a sparse coordinate graph. Propagate only
    // observed access boundaries through it, so even million-bit objects cost
    // O(accesses + propagated endpoints), never O(width).
    #[derive(Clone, Copy)]
    struct TransferRef {
        index: usize,
        from_source: bool,
        start: usize,
        end: usize,
    }

    struct TransferIndex {
        transfers: Vec<TransferRef>,
        leaf_base: usize,
        max_end: Vec<usize>,
    }

    impl TransferIndex {
        fn new(mut transfers: Vec<TransferRef>) -> Self {
            transfers.sort_unstable_by_key(|transfer| transfer.start);
            let leaf_base = transfers.len().next_power_of_two().max(1);
            let mut max_end = vec![0; leaf_base * 2];
            for (index, transfer) in transfers.iter().enumerate() {
                max_end[leaf_base + index] = transfer.end;
            }
            for index in (1..leaf_base).rev() {
                max_end[index] = max_end[index * 2].max(max_end[index * 2 + 1]);
            }
            Self {
                transfers,
                leaf_base,
                max_end,
            }
        }

        fn containing(&self, point: usize) -> Vec<TransferRef> {
            let high = self
                .transfers
                .partition_point(|transfer| transfer.start <= point);
            let mut matches = Vec::new();
            self.visit_containing(1, 0, self.leaf_base, high, point, &mut matches);
            matches
        }

        fn visit_containing(
            &self,
            node: usize,
            low: usize,
            high: usize,
            query_high: usize,
            point: usize,
            matches: &mut Vec<TransferRef>,
        ) {
            if low >= query_high || self.max_end[node] < point {
                return;
            }
            if high - low == 1 {
                if let Some(&transfer) = self.transfers.get(low)
                    && transfer.end >= point
                {
                    matches.push(transfer);
                }
                return;
            }
            let middle = low + (high - low) / 2;
            self.visit_containing(node * 2, low, middle, query_high, point, matches);
            self.visit_containing(node * 2 + 1, middle, high, query_high, point, matches);
        }
    }

    let mut transfer_refs = BTreeMap::<O, Vec<TransferRef>>::new();
    for (index, transfer) in transfers.iter().enumerate() {
        transfer_refs
            .entry(transfer.source_object)
            .or_default()
            .push(TransferRef {
                index,
                from_source: true,
                start: transfer.source_span.start,
                end: transfer.source_span.end().unwrap_or(usize::MAX),
            });
        transfer_refs
            .entry(transfer.destination_object)
            .or_default()
            .push(TransferRef {
                index,
                from_source: false,
                start: transfer.destination_span.start,
                end: transfer
                    .destination_stride
                    .checked_mul(transfer.repetitions.saturating_sub(1))
                    .and_then(|offset| transfer.destination_span.end()?.checked_add(offset))
                    .unwrap_or(usize::MAX),
            });
    }
    let transfer_index = transfer_refs
        .into_iter()
        .map(|(object, transfers)| (object, TransferIndex::new(transfers)))
        .collect::<BTreeMap<_, _>>();
    let mut pending = std::collections::VecDeque::new();
    for (&object, points) in &endpoints {
        pending.extend(points.keys().map(|&point| (object, point)));
    }
    while let Some((object, point)) = pending.pop_front() {
        let Some(object_transfers) = transfer_index.get(&object) else {
            continue;
        };
        for transfer_ref in object_transfers.containing(point) {
            let transfer = transfers[transfer_ref.index];
            if transfer.repetitions > 1 {
                if transfer_ref.from_source {
                    // Destination copy boundaries stay symbolic. Observed
                    // destination boundaries are projected back below.
                    continue;
                }
                let Some(relative) = point.checked_sub(transfer.destination_span.start) else {
                    continue;
                };
                let copy = relative / transfer.destination_stride;
                for candidate in [copy.checked_sub(1), Some(copy)]
                    .into_iter()
                    .flatten()
                    .filter(|copy| *copy < transfer.repetitions)
                {
                    let Some(copy_start) = candidate
                        .checked_mul(transfer.destination_stride)
                        .and_then(|offset| transfer.destination_span.start.checked_add(offset))
                    else {
                        continue;
                    };
                    let Some(copy_end) = copy_start.checked_add(transfer.destination_span.length)
                    else {
                        continue;
                    };
                    if point < copy_start || point > copy_end {
                        continue;
                    }
                    let Some(mapped) = point
                        .checked_sub(copy_start)
                        .and_then(|offset| transfer.source_span.start.checked_add(offset))
                    else {
                        return Err(ProcedureError::Model("aligned transfer overflows usize"));
                    };
                    let inserted = match endpoints
                        .entry(transfer.source_object)
                        .or_default()
                        .entry(mapped)
                    {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert(0);
                            true
                        }
                        std::collections::btree_map::Entry::Occupied(_) => false,
                    };
                    if inserted {
                        pending.push_back((transfer.source_object, mapped));
                    }
                }
                continue;
            }
            let (from, to_object, to) = if transfer_ref.from_source {
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
    let Some(object_atoms) = atoms_by_object.get(&object) else {
        return Vec::new();
    };
    let Some(span) = span else {
        return object_atoms.clone();
    };
    let Some(end) = span.end() else {
        return Vec::new();
    };

    // Object atoms are disjoint and ordered by start. Query only the interval
    // which can overlap this region instead of scanning every atom of a wide
    // object for every individual read or write.
    let low = object_atoms.partition_point(|&atom| {
        atoms[atom]
            .span
            .end()
            .is_some_and(|atom_end| atom_end <= span.start)
    });
    let high = object_atoms.partition_point(|&atom| atoms[atom].span.start < end);
    object_atoms[low..high].to_vec()
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
                origin: None,
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
                        repetitions: 1,
                        destination_stride: 0,
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
                origin: Some(1),
            }]
        );
    }

    #[test]
    fn periodic_copy_keeps_summary_size_and_phase_independent_of_copy_count() {
        // Why this case exists: a replicated aligned transfer is succinct in
        // RTL but used to create one transfer and atom boundary per copy. A
        // distant sparse read must project back to the matching source phase
        // while the summary remains constant-sized at 200,000 copies.
        let copies = 200_000usize;
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Read {
                    id: 0,
                    region: exact(1, 0, 2),
                },
                Event::Write {
                    id: 0,
                    region: exact(2, 0, copies * 2),
                    dependencies: vec![],
                    aligned_dependencies: vec![AlignedDependency {
                        read: 0,
                        kind: EdgeKind::Value,
                        source: Span {
                            start: 0,
                            length: 2,
                        },
                        destination: Span {
                            start: 0,
                            length: 2,
                        },
                        repetitions: copies,
                        destination_stride: 2,
                    }],
                },
                Event::Read {
                    id: 1,
                    region: exact(2, 123_456 * 2, 1),
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
        assert!(summary.atom_count < 16, "{summary:#?}");
        assert!(summary.periodic_dependencies.len() < 8, "{summary:#?}");
        for bit in 0..2 {
            let phase = summary
                .periodic_dependencies
                .iter()
                .filter(|dependency| {
                    dependency.input == exact(1, bit, 1) && dependency.output_object == 2
                })
                .collect::<Vec<_>>();
            assert_eq!(
                phase
                    .iter()
                    .map(|dependency| {
                        dependency
                            .axes
                            .iter()
                            .map(|axis| axis.repetitions)
                            .product::<usize>()
                    })
                    .sum::<usize>(),
                copies,
                "{summary:#?}"
            );
            assert!(
                phase.iter().all(|dependency| {
                    dependency.output.length == 1
                        && dependency.output.start % 2 == bit
                        && dependency.axes.len() == 1
                        && dependency.axes[0].destination_stride == 2
                }),
                "{summary:#?}"
            );
        }
        let final_inputs = summary
            .dependencies
            .iter()
            .filter(|dependency| dependency.output == exact(3, 0, 1))
            .map(|dependency| dependency.input)
            .collect::<BTreeSet<_>>();
        assert_eq!(final_inputs, [exact(1, 0, 1)].into());
    }

    #[test]
    fn nested_periodic_copy_keeps_summary_size_independent_of_axis_counts() {
        // Why this case exists: composing two non-flattenable periodic copies
        // used to emit one inner projection per outer copy. Both axes must
        // remain exact while summary size stays independent of 200,000²
        // concrete destinations.
        let copies = 200_000usize;
        let inner_stride = 2usize;
        let inner_length = (copies - 1) * inner_stride + 1;
        let outer_stride = 500_000usize;
        let outer_length = (copies - 1) * outer_stride + inner_length;
        let procedure = Procedure {
            entry: 0,
            exit: 0,
            successors: vec![vec![]],
            events: vec![vec![
                Event::Read {
                    id: 0,
                    region: exact(1, 0, 1),
                },
                Event::Write {
                    id: 0,
                    region: exact(2, 0, inner_length),
                    dependencies: vec![],
                    aligned_dependencies: vec![AlignedDependency {
                        read: 0,
                        kind: EdgeKind::Value,
                        source: Span {
                            start: 0,
                            length: 1,
                        },
                        destination: Span {
                            start: 0,
                            length: 1,
                        },
                        repetitions: copies,
                        destination_stride: inner_stride,
                    }],
                },
                Event::Read {
                    id: 1,
                    region: exact(2, 0, inner_length),
                },
                Event::Write {
                    id: 1,
                    region: exact(3, 0, outer_length),
                    dependencies: vec![],
                    aligned_dependencies: vec![AlignedDependency {
                        read: 1,
                        kind: EdgeKind::Value,
                        source: Span {
                            start: 0,
                            length: inner_length,
                        },
                        destination: Span {
                            start: 0,
                            length: inner_length,
                        },
                        repetitions: copies,
                        destination_stride: outer_stride,
                    }],
                },
            ]],
            object_spans: BTreeMap::new(),
            must_alias: BTreeSet::new(),
            incomplete: BTreeSet::new(),
        };

        let summary = analyze(&procedure).unwrap();
        assert!(summary.atom_count < 16, "{summary:#?}");
        assert!(summary.periodic_dependencies.len() < 16, "{summary:#?}");
        let nested = summary
            .periodic_dependencies
            .iter()
            .filter(|dependency| {
                dependency.input == exact(1, 0, 1) && dependency.output_object == 3
            })
            .collect::<Vec<_>>();
        assert_eq!(nested.len(), 1, "{summary:#?}");
        assert_eq!(
            nested[0].output,
            Span {
                start: 0,
                length: 1
            }
        );
        assert_eq!(
            nested[0].axes,
            vec![
                PeriodicAxis {
                    repetitions: copies,
                    destination_stride: inner_stride,
                },
                PeriodicAxis {
                    repetitions: copies,
                    destination_stride: outer_stride,
                },
            ]
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
                origin: Some(1),
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
                origin: Some(0),
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
