//! IR- and alias-domain-independent access-based MemorySSA.
//!
//! This module owns only the sparse `LiveOnEntry`/`MemoryDef`/`MemoryPhi`
//! graph, program-point coordinates into that graph, and the generic clobber
//! walk.  Definition effects, byte ranges, read queries, value numbers, and
//! lowering certificates belong to client adapters.
//!
//! Graph construction uses `O(B + E + C + D + F)` storage, where `B/E`
//! describe the CFG, `C` is the number of captured program points, `D` is the
//! number of memory definitions, and `F` is the number of MemoryPhi inputs. A
//! clobber query is linear in the visited graph in the worst case and reuses
//! one `O(D + F)` scratch allocation across all of that query's start points.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::ssa::{self, Event, SsaCfg, Version};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryAccessId(usize);

/// One ordered program event.  An event with a definition creates one
/// `MemoryDef`; an event without one only records a queryable program point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAccessEvent<D, P> {
    pub point: P,
    pub definition: Option<D>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPointAccess {
    pub before: MemoryAccessId,
    pub after: MemoryAccessId,
}

/// Coordinate conversion produced while building a graph.
///
/// This is intentionally separate from `MemoryAccessGraph`: clients that
/// retain their own instruction-to-access mapping can discard it without
/// changing the graph or the clobber walker.
#[derive(Debug)]
pub struct MemoryPointMap<P> {
    events: BTreeMap<P, MemoryPointAccess>,
    block_entries: Vec<MemoryAccessId>,
    block_exits: Vec<MemoryAccessId>,
}

impl<P: Copy + Ord> MemoryPointMap<P> {
    #[must_use]
    pub fn event(&self, point: P) -> Option<MemoryPointAccess> {
        self.events.get(&point).copied()
    }

    #[must_use]
    pub fn block_entry(&self, block: usize) -> Option<MemoryAccessId> {
        self.block_entries.get(block).copied()
    }

    #[must_use]
    pub fn block_exit(&self, block: usize) -> Option<MemoryAccessId> {
        self.block_exits.get(block).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAccess<'a, D> {
    LiveOnEntry,
    Definition {
        definition: &'a D,
        previous: MemoryAccessId,
    },
    Phi {
        block: usize,
        inputs: &'a [(usize, MemoryAccessId)],
    },
}

#[derive(Debug)]
enum MemoryAccessNode<D> {
    LiveOnEntry,
    Definition {
        definition: D,
        previous: MemoryAccessId,
    },
    Phi {
        block: usize,
        /// `(predecessor block, reaching memory state)` in CFG predecessor
        /// order.  Keeping the edge identity lets clients build structural
        /// certificates instead of treating every phi in one block as equal.
        inputs: Vec<(usize, MemoryAccessId)>,
    },
}

/// Sparse memory-state graph.  It contains no alias information and no read
/// query results.
#[derive(Debug)]
pub struct MemoryAccessGraph<D> {
    nodes: Vec<MemoryAccessNode<D>>,
    definition_count: usize,
    phi_count: usize,
}

impl<D> MemoryAccessGraph<D> {
    #[must_use]
    pub fn access(&self, access: MemoryAccessId) -> Option<MemoryAccess<'_, D>> {
        match self.nodes.get(access.0)? {
            MemoryAccessNode::LiveOnEntry => Some(MemoryAccess::LiveOnEntry),
            MemoryAccessNode::Definition {
                definition,
                previous,
            } => Some(MemoryAccess::Definition {
                definition,
                previous: *previous,
            }),
            MemoryAccessNode::Phi { block, inputs } => Some(MemoryAccess::Phi {
                block: *block,
                inputs,
            }),
        }
    }

    #[must_use]
    pub fn access_count(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn definition_count(&self) -> usize {
        self.definition_count
    }

    #[must_use]
    pub fn phi_count(&self) -> usize {
        self.phi_count
    }
}

/// Alias policy supplied by a client.  The definition identity is the graph
/// payload; effects and query representation remain outside MemorySSA.
pub trait AliasOracle<D, Q> {
    fn may_alias(&self, definition: &D, query: &Q) -> bool;
}

impl<D, Q, F> AliasOracle<D, Q> for F
where
    F: Fn(&D, &Q) -> bool,
{
    fn may_alias(&self, definition: &D, query: &Q) -> bool {
        self(definition, query)
    }
}

/// Result of a clobber walk.  `Access` is a stable identity within the graph
/// and can therefore be embedded in a client-specific snapshot certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryClobber {
    Access(MemoryAccessId),
    /// A closed cycle without a resolvable MemoryPhi.  Graphs built from a
    /// normal reachable CFG should not produce this result.
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClobberResolution {
    Access(usize),
    Cycle,
}

#[derive(Debug)]
enum ResolveFrame {
    Enter(usize),
    FinishDefinition(usize),
    FinishPhi { access: usize, inputs: usize },
}

/// Reusable, query-local state for clobber walking.  It owns no graph and no
/// alias-domain data.
#[derive(Debug, Default)]
pub struct ClobberWalker {
    epochs: Vec<u32>,
    states: Vec<u8>,
    results: Vec<Option<ClobberResolution>>,
    epoch: u32,
    frames: Vec<ResolveFrame>,
    values: Vec<ClobberResolution>,
}

impl ClobberWalker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Find the nearest definition that may alias `query`.  Diverging
    /// incoming clobbers resolve to their nearest `MemoryPhi`.
    pub fn clobber<D, Q>(
        &mut self,
        graph: &MemoryAccessGraph<D>,
        start: MemoryAccessId,
        query: &Q,
        alias_oracle: &impl AliasOracle<D, Q>,
    ) -> Option<MemoryClobber> {
        self.query(graph, query, alias_oracle).clobber(start)
    }

    /// Start one alias query which may resolve several program points. Results
    /// are memoized only for the lifetime of this session and are discarded
    /// before a different alias query begins.
    pub fn query<'a, D, Q, A>(
        &'a mut self,
        graph: &'a MemoryAccessGraph<D>,
        query: &'a Q,
        alias_oracle: &'a A,
    ) -> ClobberQuery<'a, D, Q, A>
    where
        A: AliasOracle<D, Q>,
    {
        self.epochs.resize(graph.nodes.len(), 0);
        self.states.resize(graph.nodes.len(), 0);
        self.results.resize(graph.nodes.len(), None);
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epochs.fill(0);
            self.epoch = 1;
        }
        ClobberQuery {
            walker: self,
            graph,
            query,
            alias_oracle,
        }
    }

    fn find_clobber<D, Q>(
        &mut self,
        nodes: &[MemoryAccessNode<D>],
        start: usize,
        query: &Q,
        alias_oracle: &impl AliasOracle<D, Q>,
    ) -> MemoryClobber {
        let current_epoch = self.epoch;
        self.frames.clear();
        self.values.clear();
        self.frames.push(ResolveFrame::Enter(start));

        while let Some(frame) = self.frames.pop() {
            match frame {
                ResolveFrame::Enter(access) => {
                    if self.epochs[access] == current_epoch && self.states[access] != 0 {
                        match self.states[access] {
                            1 => self.values.push(ClobberResolution::Cycle),
                            2 => self.values.push(
                                self.results[access]
                                    .expect("a resolved clobber node retains its result"),
                            ),
                            _ => unreachable!("current-epoch clobber state is valid"),
                        }
                        continue;
                    }
                    self.epochs[access] = current_epoch;
                    self.states[access] = 1;
                    self.results[access] = None;
                    match &nodes[access] {
                        MemoryAccessNode::LiveOnEntry => {
                            let result = ClobberResolution::Access(access);
                            self.states[access] = 2;
                            self.results[access] = Some(result);
                            self.values.push(result);
                        }
                        MemoryAccessNode::Definition {
                            definition,
                            previous,
                        } => {
                            if alias_oracle.may_alias(definition, query) {
                                let result = ClobberResolution::Access(access);
                                self.states[access] = 2;
                                self.results[access] = Some(result);
                                self.values.push(result);
                            } else {
                                self.frames.push(ResolveFrame::FinishDefinition(access));
                                self.frames.push(ResolveFrame::Enter(previous.0));
                            }
                        }
                        MemoryAccessNode::Phi { inputs, .. } => {
                            self.frames.push(ResolveFrame::FinishPhi {
                                access,
                                inputs: inputs.len(),
                            });
                            self.frames.extend(
                                inputs
                                    .iter()
                                    .rev()
                                    .map(|(_, input)| ResolveFrame::Enter(input.0)),
                            );
                        }
                    }
                }
                ResolveFrame::FinishDefinition(access) => {
                    let result = self
                        .values
                        .pop()
                        .expect("a MemoryDef predecessor produces one clobber result");
                    if result == ClobberResolution::Cycle {
                        // A disjoint definition on a loop backedge cannot be
                        // resolved independently of the active MemoryPhi.
                        self.states[access] = 0;
                        self.results[access] = None;
                    } else {
                        self.states[access] = 2;
                        self.results[access] = Some(result);
                    }
                    self.values.push(result);
                }
                ResolveFrame::FinishPhi { access, inputs } => {
                    let first = self
                        .values
                        .len()
                        .checked_sub(inputs)
                        .expect("every MemoryPhi input produces one clobber result");
                    let mut common = None::<ClobberResolution>;
                    let mut diverged = false;
                    for result in self.values.drain(first..) {
                        if result == ClobberResolution::Cycle {
                            continue;
                        }
                        match common {
                            Some(previous) if previous != result => diverged = true,
                            Some(_) => {}
                            None => common = Some(result),
                        }
                    }
                    let result = if diverged {
                        ClobberResolution::Access(access)
                    } else {
                        common.unwrap_or(ClobberResolution::Access(access))
                    };
                    self.states[access] = 2;
                    self.results[access] = Some(result);
                    self.values.push(result);
                }
            }
        }

        let resolution = self
            .values
            .pop()
            .expect("one clobber query produces one result");
        debug_assert!(self.values.is_empty());
        match resolution {
            ClobberResolution::Access(access) => MemoryClobber::Access(MemoryAccessId(access)),
            ClobberResolution::Cycle => MemoryClobber::Indeterminate,
        }
    }
}

/// Multi-point clobber query for one immutable alias query. Resolving a phi
/// root and then its incoming states therefore visits each access at most once
/// instead of restarting a whole-graph walk per edge.
pub struct ClobberQuery<'a, D, Q, A> {
    walker: &'a mut ClobberWalker,
    graph: &'a MemoryAccessGraph<D>,
    query: &'a Q,
    alias_oracle: &'a A,
}

impl<D, Q, A> ClobberQuery<'_, D, Q, A>
where
    A: AliasOracle<D, Q>,
{
    pub fn clobber(&mut self, start: MemoryAccessId) -> Option<MemoryClobber> {
        (start.0 < self.graph.nodes.len()).then(|| {
            self.walker
                .find_clobber(&self.graph.nodes, start.0, self.query, self.alias_oracle)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySsaError {
    pub rule: &'static str,
    pub block: Option<usize>,
    pub message: String,
}

impl MemorySsaError {
    fn new(rule: &'static str, block: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            rule,
            block,
            message: message.into(),
        }
    }
}

impl fmt::Display for MemorySsaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.rule)?;
        if let Some(block) = self.block {
            write!(formatter, " at block {block}")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl std::error::Error for MemorySsaError {}

impl From<ssa::SsaError> for MemorySsaError {
    fn from(error: ssa::SsaError) -> Self {
        Self {
            rule: error.rule,
            block: error.block,
            message: error.message,
        }
    }
}

#[derive(Debug)]
struct PendingPoint<D, P> {
    point: P,
    before_usage: usize,
    definition: Option<D>,
}

#[derive(Debug)]
struct PendingDefinition<D> {
    definition: D,
    predecessor_usage: usize,
}

/// Build a standard access-based MemorySSA graph and a separate coordinate
/// map.  The caller decides which events are memory definitions; no effect or
/// alias representation crosses this interface.
pub fn build<D, P>(
    cfg: &impl SsaCfg,
    memory_events: &[Vec<MemoryAccessEvent<D, P>>],
) -> Result<(MemoryAccessGraph<D>, MemoryPointMap<P>), MemorySsaError>
where
    D: Copy + Ord,
    P: Copy + Ord,
{
    if memory_events.len() != cfg.successors().len() {
        return Err(MemorySsaError::new(
            "MEMORY_SSA.MODEL_SHAPE",
            None,
            "memory-event and CFG block tables have different lengths",
        ));
    }

    // A single SSA variable represents abstract memory state.  Uses at event
    // and block boundaries exist solely to build the separately returned
    // coordinate map.
    let mut events = vec![Vec::<Event<(), D, usize>>::new(); memory_events.len()];
    let mut pending_points = Vec::<PendingPoint<D, P>>::new();
    let mut pending_definitions = Vec::<PendingDefinition<D>>::new();
    let mut block_entry_usages = Vec::with_capacity(memory_events.len());
    let mut block_exit_usages = Vec::with_capacity(memory_events.len());
    let mut point_ids = BTreeSet::<P>::new();
    let mut next_usage = 0usize;

    for (block, block_events) in memory_events.iter().enumerate() {
        let entry_usage = allocate_usage(&mut next_usage)?;
        events[block].push(Event::Use {
            variable: (),
            usage: entry_usage,
        });
        block_entry_usages.push(entry_usage);

        for event in block_events {
            if !point_ids.insert(event.point) {
                return Err(MemorySsaError::new(
                    "MEMORY_SSA.POINT_IDENTITY",
                    Some(block),
                    "one memory program-point identity occurs more than once",
                ));
            }
            let before_usage = allocate_usage(&mut next_usage)?;
            events[block].push(Event::Use {
                variable: (),
                usage: before_usage,
            });
            pending_points.push(PendingPoint {
                point: event.point,
                before_usage,
                definition: event.definition,
            });
            if let Some(definition) = event.definition {
                events[block].push(Event::Definition {
                    variable: (),
                    definition,
                });
                pending_definitions.push(PendingDefinition {
                    definition,
                    predecessor_usage: before_usage,
                });
            }
        }

        let exit_usage = allocate_usage(&mut next_usage)?;
        events[block].push(Event::Use {
            variable: (),
            usage: exit_usage,
        });
        block_exit_usages.push(exit_usage);
    }

    let ssa = ssa::build(cfg, &events)?;
    let mut definition_accesses = BTreeMap::<D, MemoryAccessId>::new();
    for (definition_index, definition) in pending_definitions.iter().enumerate() {
        let access = MemoryAccessId(definition_index + 1);
        if definition_accesses
            .insert(definition.definition, access)
            .is_some()
        {
            return Err(MemorySsaError::new(
                "MEMORY_SSA.DEFINITION_IDENTITY",
                None,
                "one definition identity has multiple MemoryDefs",
            ));
        }
    }

    let mut phi_accesses = BTreeMap::<usize, MemoryAccessId>::new();
    let phi_start = pending_definitions.len() + 1;
    for (phi_index, phi) in ssa.phis.iter().enumerate() {
        let access = MemoryAccessId(phi_start + phi_index);
        if phi_accesses.insert(phi.block, access).is_some() {
            return Err(MemorySsaError::new(
                "MEMORY_SSA.PHI_IDENTITY",
                Some(phi.block),
                "one block has multiple phis for the single memory state",
            ));
        }
    }

    let mut nodes = Vec::with_capacity(1 + pending_definitions.len() + ssa.phis.len());
    nodes.push(MemoryAccessNode::LiveOnEntry);
    for definition in &pending_definitions {
        let access = definition_accesses[&definition.definition];
        let previous = ssa
            .uses
            .get(&definition.predecessor_usage)
            .copied()
            .ok_or_else(|| {
                MemorySsaError::new(
                    "MEMORY_SSA.DEFINITION_PREDECESSOR",
                    None,
                    "MemoryDef has no reaching memory state",
                )
            })?;
        let previous = access_for_version(previous, &definition_accesses, &phi_accesses)?;
        debug_assert_eq!(nodes.len(), access.0);
        nodes.push(MemoryAccessNode::Definition {
            definition: definition.definition,
            previous,
        });
    }
    for phi in &ssa.phis {
        let access = phi_accesses[&phi.block];
        let inputs = phi
            .inputs
            .iter()
            .map(|&(predecessor, version)| {
                access_for_version(version, &definition_accesses, &phi_accesses)
                    .map(|access| (predecessor, access))
            })
            .collect::<Result<Vec<_>, _>>()?;
        debug_assert_eq!(nodes.len(), access.0);
        nodes.push(MemoryAccessNode::Phi {
            block: phi.block,
            inputs,
        });
    }

    let usage_access = |usage| {
        let version = ssa.uses.get(&usage).copied().ok_or_else(|| {
            MemorySsaError::new(
                "MEMORY_SSA.POINT_VERSION",
                None,
                "memory program point has no reaching memory state",
            )
        })?;
        access_for_version(version, &definition_accesses, &phi_accesses)
    };
    let mut point_accesses = BTreeMap::<P, MemoryPointAccess>::new();
    for point in pending_points {
        let before = usage_access(point.before_usage)?;
        let after = point
            .definition
            .map_or(before, |definition| definition_accesses[&definition]);
        if point_accesses
            .insert(point.point, MemoryPointAccess { before, after })
            .is_some()
        {
            return Err(MemorySsaError::new(
                "MEMORY_SSA.POINT_ACCESS_IDENTITY",
                None,
                "one program point has multiple MemorySSA coordinate records",
            ));
        }
    }
    let block_entries = block_entry_usages
        .into_iter()
        .map(usage_access)
        .collect::<Result<Vec<_>, _>>()?;
    let block_exits = block_exit_usages
        .into_iter()
        .map(usage_access)
        .collect::<Result<Vec<_>, _>>()?;

    Ok((
        MemoryAccessGraph {
            nodes,
            definition_count: pending_definitions.len(),
            phi_count: ssa.phis.len(),
        },
        MemoryPointMap {
            events: point_accesses,
            block_entries,
            block_exits,
        },
    ))
}

fn allocate_usage(next_usage: &mut usize) -> Result<usize, MemorySsaError> {
    let usage = *next_usage;
    *next_usage = next_usage.checked_add(1).ok_or_else(|| {
        MemorySsaError::new(
            "MEMORY_SSA.USE_ID_RANGE",
            None,
            "memory use count exceeds usize",
        )
    })?;
    Ok(usage)
}

fn access_for_version<D: Copy + Ord>(
    version: Version<(), D>,
    definitions: &BTreeMap<D, MemoryAccessId>,
    phis: &BTreeMap<usize, MemoryAccessId>,
) -> Result<MemoryAccessId, MemorySsaError> {
    match version {
        Version::Entry(()) => Ok(MemoryAccessId(0)),
        Version::Definition { definition, .. } => {
            definitions.get(&definition).copied().ok_or_else(|| {
                MemorySsaError::new(
                    "MEMORY_SSA.DEFINITION_ACCESS",
                    None,
                    "SSA definition has no MemoryDef",
                )
            })
        }
        Version::Phi { block, .. } => phis.get(&block).copied().ok_or_else(|| {
            MemorySsaError::new(
                "MEMORY_SSA.PHI_ACCESS",
                Some(block),
                "SSA phi has no MemoryPhi",
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::ControlFlowGraph;
    use crate::memory::{MemoryEffect, MemoryLocation, effects_may_alias};

    type Object = u8;
    type Instruction = (usize, usize);

    #[derive(Debug)]
    struct EffectEvent {
        instruction: Instruction,
        reads: Vec<MemoryEffect<Object>>,
        writes: Vec<MemoryEffect<Object>>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestClobber {
        LiveOnEntry,
        Definition(Instruction),
        Phi(usize),
        Indeterminate,
    }

    struct Analysis {
        graph: MemoryAccessGraph<Instruction>,
        points: MemoryPointMap<Instruction>,
        writes: BTreeMap<Instruction, Vec<MemoryEffect<Object>>>,
        reads: Vec<(Instruction, usize, MemoryEffect<Object>)>,
    }

    impl Analysis {
        fn clobber(&self, instruction: Instruction, query: MemoryEffect<Object>) -> TestClobber {
            let start = self.points.event(instruction).unwrap().before;
            let oracle = |definition: &Instruction, query: &MemoryEffect<Object>| {
                self.writes[definition]
                    .iter()
                    .copied()
                    .any(|write| effects_may_alias(write, *query))
            };
            let mut walker = ClobberWalker::new();
            match walker.clobber(&self.graph, start, &query, &oracle).unwrap() {
                MemoryClobber::Indeterminate => TestClobber::Indeterminate,
                MemoryClobber::Access(access) => match self.graph.access(access).unwrap() {
                    MemoryAccess::LiveOnEntry => TestClobber::LiveOnEntry,
                    MemoryAccess::Definition { definition, .. } => {
                        TestClobber::Definition(*definition)
                    }
                    MemoryAccess::Phi { block, .. } => TestClobber::Phi(block),
                },
            }
        }

        fn only_clobber(&self) -> TestClobber {
            assert_eq!(self.reads.len(), 1);
            let (instruction, _, query) = self.reads[0];
            self.clobber(instruction, query)
        }
    }

    fn exact(object: Object, offset: i64, byte_len: usize) -> MemoryEffect<Object> {
        MemoryEffect::Exact(MemoryLocation {
            object,
            offset,
            byte_len,
        })
    }

    fn event(
        instruction: Instruction,
        reads: Vec<MemoryEffect<Object>>,
        writes: Vec<MemoryEffect<Object>>,
    ) -> EffectEvent {
        EffectEvent {
            instruction,
            reads,
            writes,
        }
    }

    fn analyze(cfg: &ControlFlowGraph, events: &[Vec<EffectEvent>]) -> Analysis {
        let mut access_events = vec![Vec::new(); events.len()];
        let mut writes = BTreeMap::new();
        let mut reads = Vec::new();
        for (block, block_events) in events.iter().enumerate() {
            for event in block_events {
                for (read_index, &read) in event.reads.iter().enumerate() {
                    reads.push((event.instruction, read_index, read));
                }
                if !event.writes.is_empty() {
                    writes.insert(event.instruction, event.writes.clone());
                }
                access_events[block].push(MemoryAccessEvent {
                    point: event.instruction,
                    definition: (!event.writes.is_empty()).then_some(event.instruction),
                });
            }
        }
        let (graph, points) = build(cfg, &access_events).unwrap();
        Analysis {
            graph,
            points,
            writes,
            reads,
        }
    }

    #[test]
    fn exact_store_reaches_later_load() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[vec![
                event((0, 0), vec![], vec![exact(1, 8, 4)]),
                event((0, 1), vec![exact(1, 8, 4)], vec![]),
            ]],
        );

        assert_eq!(analysis.only_clobber(), TestClobber::Definition((0, 0)));
        assert_eq!(analysis.graph.definition_count(), 1);
        assert_eq!(analysis.graph.phi_count(), 0);
        assert_eq!(analysis.graph.access_count(), 2);
    }

    #[test]
    fn point_map_supports_lowering_coordinates_without_owning_queries() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let analysis = analyze(&cfg, &[vec![event((0, 0), vec![], vec![exact(1, 8, 8)])]]);
        let event = analysis.points.event((0, 0)).unwrap();

        assert_eq!(
            analysis.clobber((0, 0), exact(1, 8, 8)),
            TestClobber::LiveOnEntry
        );
        let oracle = |definition: &Instruction, query: &MemoryEffect<Object>| {
            analysis.writes[definition]
                .iter()
                .copied()
                .any(|write| effects_may_alias(write, *query))
        };
        let mut walker = ClobberWalker::new();
        let after = walker
            .clobber(&analysis.graph, event.after, &exact(1, 8, 8), &oracle)
            .unwrap();
        let MemoryClobber::Access(after) = after else {
            panic!("a store's post-state has a concrete clobber")
        };
        assert!(matches!(
            analysis.graph.access(after),
            Some(MemoryAccess::Definition {
                definition: &(0, 0),
                ..
            })
        ));
        assert_eq!(analysis.points.block_entry(0), Some(event.before));
        assert_eq!(analysis.points.block_exit(0), Some(event.after));
    }

    #[test]
    fn disjoint_store_is_skipped_by_the_external_alias_oracle() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[vec![
                event((0, 0), vec![], vec![exact(1, 0, 8)]),
                event((0, 1), vec![exact(1, 16, 8)], vec![]),
            ]],
        );

        assert_eq!(analysis.only_clobber(), TestClobber::LiveOnEntry);
    }

    #[test]
    fn partial_overlap_and_unknown_object_are_clobbers() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let overlap = analyze(
            &cfg,
            &[vec![
                event((0, 0), vec![], vec![exact(1, 4, 8)]),
                event((0, 1), vec![exact(1, 8, 8)], vec![]),
            ]],
        );
        assert_eq!(overlap.only_clobber(), TestClobber::Definition((0, 0)));

        let unknown = analyze(
            &cfg,
            &[vec![
                event((0, 0), vec![], vec![MemoryEffect::UnknownObject(1)]),
                event((0, 1), vec![exact(1, 8, 8)], vec![]),
            ]],
        );
        assert_eq!(unknown.only_clobber(), TestClobber::Definition((0, 0)));
    }

    #[test]
    fn one_arm_store_resolves_to_the_join_phi() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1, 2], vec![3], vec![3], vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[
                vec![],
                vec![event((1, 0), vec![], vec![exact(1, 8, 8)])],
                vec![],
                vec![event((3, 0), vec![exact(1, 8, 8)], vec![])],
            ],
        );

        assert_eq!(analysis.only_clobber(), TestClobber::Phi(3));
        assert_eq!(analysis.graph.phi_count(), 1);
    }

    #[test]
    fn one_query_session_reuses_phi_input_clobbers() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1, 2], vec![3], vec![3], vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[
                vec![],
                vec![event((1, 0), vec![], vec![exact(1, 8, 8)])],
                vec![],
                vec![event((3, 0), vec![exact(1, 8, 8)], vec![])],
            ],
        );
        let calls = std::cell::Cell::new(0usize);
        let oracle = |definition: &Instruction, query: &MemoryEffect<Object>| {
            calls.set(calls.get() + 1);
            analysis.writes[definition]
                .iter()
                .copied()
                .any(|write| effects_may_alias(write, *query))
        };
        let query = exact(1, 8, 8);
        let mut walker = ClobberWalker::new();
        let mut session = walker.query(&analysis.graph, &query, &oracle);
        let join = analysis.points.event((3, 0)).unwrap().before;
        assert!(matches!(
            session.clobber(join),
            Some(MemoryClobber::Access(_))
        ));
        let calls_after_join = calls.get();
        let left_exit = analysis.points.block_exit(1).unwrap();
        assert!(matches!(
            session.clobber(left_exit),
            Some(MemoryClobber::Access(_))
        ));
        assert_eq!(calls.get(), calls_after_join);
    }

    #[test]
    fn same_dominating_store_is_found_through_a_join() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1, 2], vec![3], vec![3], vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[
                vec![event((0, 0), vec![], vec![exact(1, 8, 8)])],
                vec![],
                vec![],
                vec![event((3, 0), vec![exact(1, 8, 8)], vec![])],
            ],
        );

        assert_eq!(analysis.only_clobber(), TestClobber::Definition((0, 0)));
    }

    #[test]
    fn disjoint_loop_definition_does_not_hide_dominating_store() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1], vec![1, 2], vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[
                vec![event((0, 0), vec![], vec![exact(1, 8, 8)])],
                vec![
                    event((1, 0), vec![], vec![exact(1, 64, 8)]),
                    event((1, 1), vec![exact(1, 8, 8)], vec![]),
                ],
                vec![],
            ],
        );

        assert_eq!(analysis.only_clobber(), TestClobber::Definition((0, 0)));
        assert_eq!(analysis.graph.phi_count(), 1);
    }

    #[test]
    fn aliasing_loop_definition_resolves_to_the_header_phi() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1], vec![1, 2], vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[
                vec![event((0, 0), vec![], vec![exact(1, 8, 8)])],
                vec![event((1, 0), vec![], vec![exact(1, 8, 1)])],
                vec![],
            ],
        );

        let exit = analysis.points.block_exit(1).unwrap();
        let entry = analysis.points.block_entry(1).unwrap();
        let oracle = |definition: &Instruction, query: &MemoryEffect<Object>| {
            analysis.writes[definition]
                .iter()
                .copied()
                .any(|write| effects_may_alias(write, *query))
        };
        let classify = |clobber| match clobber {
            MemoryClobber::Indeterminate => TestClobber::Indeterminate,
            MemoryClobber::Access(access) => match analysis.graph.access(access).unwrap() {
                MemoryAccess::LiveOnEntry => TestClobber::LiveOnEntry,
                MemoryAccess::Definition { definition, .. } => TestClobber::Definition(*definition),
                MemoryAccess::Phi { block, .. } => TestClobber::Phi(block),
            },
        };
        let mut walker = ClobberWalker::new();
        assert_eq!(
            classify(
                walker
                    .clobber(&analysis.graph, exit, &exact(1, 8, 8), &oracle)
                    .unwrap()
            ),
            TestClobber::Definition((1, 0))
        );
        assert_eq!(
            classify(
                walker
                    .clobber(&analysis.graph, entry, &exact(1, 8, 8), &oracle)
                    .unwrap()
            ),
            TestClobber::Phi(1)
        );
    }

    #[test]
    fn graph_size_is_independent_of_effect_range_length() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let analysis = analyze(
            &cfg,
            &[vec![
                event((0, 0), vec![], vec![exact(1, 1_000_000, 16 * 1024 * 1024)]),
                event((0, 1), vec![exact(1, 2_000_000, 8)], vec![]),
            ]],
        );

        assert_eq!(analysis.only_clobber(), TestClobber::Definition((0, 0)));
        assert_eq!(analysis.graph.definition_count(), 1);
        assert_eq!(analysis.graph.access_count(), 2);
    }

    #[test]
    fn custom_alias_domain_needs_no_memory_effect_types() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let (graph, points) = build(
            &cfg,
            &[vec![
                MemoryAccessEvent {
                    point: 0u8,
                    definition: Some(10u8),
                },
                MemoryAccessEvent {
                    point: 1u8,
                    definition: None,
                },
            ]],
        )
        .unwrap();
        let oracle = |definition: &u8, query: &u8| definition % 10 == *query;
        let mut walker = ClobberWalker::new();
        let clobber = walker
            .clobber(&graph, points.event(1).unwrap().before, &0, &oracle)
            .unwrap();
        let MemoryClobber::Access(access) = clobber else {
            panic!("the custom oracle finds one definition")
        };
        assert!(matches!(
            graph.access(access),
            Some(MemoryAccess::Definition { definition: 10, .. })
        ));
    }
}
