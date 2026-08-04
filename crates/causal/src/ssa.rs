//! Sparse pruned SSA construction over caller-defined variable identities.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use crate::cfg::{ControlFlowGraph, ForwardControlFlowGraph};

/// Minimal CFG view required by SSA construction.
///
/// Clients that already own dominance information can implement this view
/// without rebuilding postdominators, control dependence, SCCs, or loops.
pub trait SsaCfg {
    type FrontierIter<'a>: Iterator<Item = usize>
    where
        Self: 'a;

    fn root(&self) -> usize;
    fn predecessors(&self) -> &[Vec<usize>];
    fn successors(&self) -> &[Vec<usize>];
    fn dominator_children(&self) -> &[Vec<usize>];
    fn dominance_frontier_len(&self) -> usize;
    fn dominance_frontier(&self, block: usize) -> Self::FrontierIter<'_>;
}

impl SsaCfg for ControlFlowGraph {
    type FrontierIter<'a> = std::iter::Copied<std::slice::Iter<'a, usize>>;

    fn root(&self) -> usize {
        self.root
    }

    fn predecessors(&self) -> &[Vec<usize>] {
        &self.predecessors
    }

    fn successors(&self) -> &[Vec<usize>] {
        &self.successors
    }

    fn dominator_children(&self) -> &[Vec<usize>] {
        &self.dominators.children
    }

    fn dominance_frontier_len(&self) -> usize {
        self.dominance_frontier.len()
    }

    fn dominance_frontier(&self, block: usize) -> Self::FrontierIter<'_> {
        self.dominance_frontier[block].iter().copied()
    }
}

impl SsaCfg for ForwardControlFlowGraph {
    type FrontierIter<'a> = std::iter::Copied<std::slice::Iter<'a, usize>>;

    fn root(&self) -> usize {
        self.root
    }

    fn predecessors(&self) -> &[Vec<usize>] {
        &self.predecessors
    }

    fn successors(&self) -> &[Vec<usize>] {
        &self.successors
    }

    fn dominator_children(&self) -> &[Vec<usize>] {
        &self.dominators.children
    }

    fn dominance_frontier_len(&self) -> usize {
        self.dominance_frontier.len()
    }

    fn dominance_frontier(&self, block: usize) -> Self::FrontierIter<'_> {
        self.dominance_frontier[block].iter().copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version<V, D> {
    Entry(V),
    Definition { variable: V, definition: D },
    Phi { variable: V, block: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<V, D, U> {
    Use { variable: V, usage: U },
    Definition { variable: V, definition: D },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phi<V, D> {
    pub variable: V,
    pub block: usize,
    pub version: Version<V, D>,
    pub inputs: Vec<(usize, Version<V, D>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparseSsa<V, D, U> {
    pub phis: Vec<Phi<V, D>>,
    pub phis_by_block: Vec<Vec<usize>>,
    pub uses: BTreeMap<U, Version<V, D>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsaError {
    pub rule: &'static str,
    pub block: Option<usize>,
    pub message: String,
}

impl SsaError {
    fn new(rule: &'static str, block: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            rule,
            block,
            message: message.into(),
        }
    }
}

impl fmt::Display for SsaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.rule)?;
        if let Some(block) = self.block {
            write!(formatter, " at block {block}")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl std::error::Error for SsaError {}

/// Construct pruned SSA from events already ordered within each dense block.
pub fn build<V, D, U>(
    cfg: &impl SsaCfg,
    events: &[Vec<Event<V, D, U>>],
) -> Result<SparseSsa<V, D, U>, SsaError>
where
    V: Copy + Ord,
    D: Copy + Ord,
    U: Copy + Ord,
{
    let blocks = cfg.successors().len();
    if events.len() != blocks
        || cfg.predecessors().len() != blocks
        || cfg.dominator_children().len() != blocks
        || cfg.dominance_frontier_len() != blocks
    {
        return Err(SsaError::new(
            "SSA.MODEL_SHAPE",
            None,
            "CFG and event tables do not cover the same block domain",
        ));
    }

    let mut definitions = BTreeMap::<V, BTreeSet<usize>>::new();
    let mut definition_ids = BTreeSet::<(V, D)>::new();
    let mut upward_uses = BTreeSet::<(V, usize)>::new();
    let mut usage_ids = BTreeSet::<U>::new();
    for (block, block_events) in events.iter().enumerate() {
        let mut locally_defined = BTreeSet::<V>::new();
        for event in block_events {
            match *event {
                Event::Use { variable, usage } => {
                    if !usage_ids.insert(usage) {
                        return Err(SsaError::new(
                            "SSA.USE_IDENTITY",
                            Some(block),
                            "one use identity occurs more than once",
                        ));
                    }
                    if !locally_defined.contains(&variable) {
                        upward_uses.insert((variable, block));
                    }
                }
                Event::Definition {
                    variable,
                    definition,
                } => {
                    if !definition_ids.insert((variable, definition)) {
                        return Err(SsaError::new(
                            "SSA.DEFINITION_IDENTITY",
                            Some(block),
                            "one variable-definition identity occurs more than once",
                        ));
                    }
                    locally_defined.insert(variable);
                    definitions.entry(variable).or_default().insert(block);
                }
            }
        }
    }

    let definition_pairs = definitions
        .iter()
        .flat_map(|(&variable, blocks)| blocks.iter().map(move |&block| (variable, block)))
        .collect::<BTreeSet<_>>();
    let mut live_in = upward_uses.clone();
    let mut live_work = upward_uses.into_iter().collect::<VecDeque<_>>();
    while let Some((variable, block)) = live_work.pop_front() {
        for &predecessor in &cfg.predecessors()[block] {
            let pair = (variable, predecessor);
            if !definition_pairs.contains(&pair) && live_in.insert(pair) {
                live_work.push_back(pair);
            }
        }
    }

    let mut phi_pairs = BTreeSet::<(usize, V)>::new();
    for (&variable, original_definitions) in &definitions {
        let mut queued = original_definitions.clone();
        let mut work = original_definitions
            .iter()
            .copied()
            .collect::<VecDeque<_>>();
        while let Some(definition) = work.pop_front() {
            for frontier in cfg.dominance_frontier(definition) {
                if !live_in.contains(&(variable, frontier))
                    || !phi_pairs.insert((frontier, variable))
                {
                    continue;
                }
                if queued.insert(frontier) {
                    work.push_back(frontier);
                }
            }
        }
    }

    let mut phis = Vec::<Phi<V, D>>::with_capacity(phi_pairs.len());
    let mut phis_by_block = vec![Vec::<usize>::new(); blocks];
    for (block, variable) in phi_pairs {
        let phi = phis.len();
        phis.push(Phi {
            variable,
            block,
            version: Version::Phi { variable, block },
            inputs: Vec::with_capacity(cfg.predecessors()[block].len()),
        });
        phis_by_block[block].push(phi);
    }

    enum Action<V, D> {
        Enter(usize),
        Exit(Vec<(V, Option<Version<V, D>>)>),
    }
    let mut current = BTreeMap::<V, Version<V, D>>::new();
    let mut uses = BTreeMap::<U, Version<V, D>>::new();
    let mut actions = vec![Action::Enter(cfg.root())];
    while let Some(action) = actions.pop() {
        let block = match action {
            Action::Exit(changes) => {
                for (variable, previous) in changes.into_iter().rev() {
                    if let Some(previous) = previous {
                        current.insert(variable, previous);
                    } else {
                        current.remove(&variable);
                    }
                }
                continue;
            }
            Action::Enter(block) => block,
        };
        let mut changes = Vec::new();
        for &phi in &phis_by_block[block] {
            let variable = phis[phi].variable;
            changes.push((variable, current.insert(variable, phis[phi].version)));
        }
        for event in &events[block] {
            match *event {
                Event::Use { variable, usage } => {
                    let version = current
                        .get(&variable)
                        .copied()
                        .unwrap_or(Version::Entry(variable));
                    if uses.insert(usage, version).is_some() {
                        return Err(SsaError::new(
                            "SSA.USE_RENAME",
                            Some(block),
                            "dominator rename visited one use more than once",
                        ));
                    }
                }
                Event::Definition {
                    variable,
                    definition,
                } => {
                    let version = Version::Definition {
                        variable,
                        definition,
                    };
                    changes.push((variable, current.insert(variable, version)));
                }
            }
        }
        for &successor in &cfg.successors()[block] {
            for &phi in &phis_by_block[successor] {
                let variable = phis[phi].variable;
                let version = current
                    .get(&variable)
                    .copied()
                    .unwrap_or(Version::Entry(variable));
                phis[phi].inputs.push((block, version));
            }
        }
        actions.push(Action::Exit(changes));
        actions.extend(
            cfg.dominator_children()[block]
                .iter()
                .rev()
                .copied()
                .map(Action::Enter),
        );
    }

    if uses.len() != usage_ids.len() {
        return Err(SsaError::new(
            "SSA.USE_COVERAGE",
            None,
            "dominator rename did not visit every use",
        ));
    }
    for phi in &mut phis {
        phi.inputs
            .sort_unstable_by_key(|(predecessor, _)| *predecessor);
        if phi.inputs.len() != cfg.predecessors()[phi.block].len()
            || phi
                .inputs
                .iter()
                .zip(&cfg.predecessors()[phi.block])
                .any(|((actual, _), expected)| actual != expected)
        {
            return Err(SsaError::new(
                "SSA.PHI_INPUTS",
                Some(phi.block),
                "phi inputs do not cover every CFG predecessor exactly once",
            ));
        }
    }

    Ok(SparseSsa {
        phis,
        phis_by_block,
        uses,
    })
}

/// Construct SSA for one straight-line block without building dominance or
/// liveness state. This is equivalent to [`build`] for a one-block CFG with no
/// successors, but avoids paying the general CFG cost for ordinary continuous
/// assignments and other branch-free procedures.
pub fn build_linear<V, D, U>(events: &[Event<V, D, U>]) -> Result<SparseSsa<V, D, U>, SsaError>
where
    V: Copy + Ord,
    D: Copy + Ord,
    U: Copy + Ord,
{
    let mut current = BTreeMap::<V, Version<V, D>>::new();
    let mut definitions = BTreeSet::<(V, D)>::new();
    let mut uses = BTreeMap::<U, Version<V, D>>::new();
    for event in events {
        match *event {
            Event::Use { variable, usage } => {
                let version = current
                    .get(&variable)
                    .copied()
                    .unwrap_or(Version::Entry(variable));
                if uses.insert(usage, version).is_some() {
                    return Err(SsaError::new(
                        "SSA.USE_IDENTITY",
                        Some(0),
                        "one use identity occurs more than once",
                    ));
                }
            }
            Event::Definition {
                variable,
                definition,
            } => {
                if !definitions.insert((variable, definition)) {
                    return Err(SsaError::new(
                        "SSA.DEFINITION_IDENTITY",
                        Some(0),
                        "one variable-definition identity occurs more than once",
                    ));
                }
                current.insert(
                    variable,
                    Version::Definition {
                        variable,
                        definition,
                    },
                );
            }
        }
    }
    Ok(SparseSsa {
        phis: Vec::new(),
        phis_by_block: vec![Vec::new()],
        uses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_definitions_create_one_live_join_phi() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1, 2], vec![3], vec![3], vec![]], 0).unwrap();
        let events = vec![
            vec![],
            vec![Event::Definition {
                variable: 7,
                definition: 10,
            }],
            vec![Event::Definition {
                variable: 7,
                definition: 20,
            }],
            vec![Event::Use {
                variable: 7,
                usage: 30,
            }],
        ];

        let ssa = build(&cfg, &events).unwrap();

        assert_eq!(ssa.phis.len(), 1);
        assert_eq!(ssa.phis[0].block, 3);
        assert_eq!(ssa.phis[0].inputs.len(), 2);
        assert_eq!(
            ssa.uses[&30],
            Version::Phi {
                variable: 7,
                block: 3
            }
        );
    }

    #[test]
    fn dead_join_does_not_receive_a_phi() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1, 2], vec![3], vec![3], vec![]], 0).unwrap();
        let events = vec![
            vec![],
            vec![Event::Definition {
                variable: 7,
                definition: 10,
            }],
            vec![Event::Definition {
                variable: 7,
                definition: 20,
            }],
            vec![],
        ];

        let ssa = build::<_, _, usize>(&cfg, &events).unwrap();

        assert!(ssa.phis.is_empty());
    }

    #[test]
    fn loop_use_observes_header_phi() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1], vec![2, 3], vec![1], vec![]], 0).unwrap();
        let events = vec![
            vec![Event::Definition {
                variable: 1,
                definition: 0,
            }],
            vec![Event::Use {
                variable: 1,
                usage: 10,
            }],
            vec![Event::Definition {
                variable: 1,
                definition: 20,
            }],
            vec![],
        ];

        let ssa = build(&cfg, &events).unwrap();

        assert_eq!(
            ssa.uses[&10],
            Version::Phi {
                variable: 1,
                block: 1
            }
        );
        assert_eq!(ssa.phis[0].inputs.len(), 2);
    }

    #[test]
    fn same_block_uses_observe_event_order() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let events = vec![vec![
            Event::Use {
                variable: 1,
                usage: 1,
            },
            Event::Definition {
                variable: 1,
                definition: 2,
            },
            Event::Use {
                variable: 1,
                usage: 3,
            },
        ]];

        let ssa = build(&cfg, &events).unwrap();

        assert_eq!(ssa.uses[&1], Version::Entry(1));
        assert_eq!(
            ssa.uses[&3],
            Version::Definition {
                variable: 1,
                definition: 2
            }
        );
    }

    #[test]
    fn linear_builder_matches_general_one_block_ssa() {
        let cfg = ControlFlowGraph::analyze(vec![vec![]], 0).unwrap();
        let events = vec![
            Event::Use {
                variable: 1,
                usage: 10,
            },
            Event::Definition {
                variable: 1,
                definition: 20,
            },
            Event::Use {
                variable: 2,
                usage: 30,
            },
            Event::Definition {
                variable: 2,
                definition: 40,
            },
            Event::Use {
                variable: 1,
                usage: 50,
            },
            Event::Use {
                variable: 2,
                usage: 60,
            },
        ];

        let general = build(&cfg, std::slice::from_ref(&events)).unwrap();
        let linear = build_linear(&events).unwrap();

        assert_eq!(linear, general);
    }
}
