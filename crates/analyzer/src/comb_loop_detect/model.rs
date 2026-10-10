//! Shared dependency and module-summary data model.

use super::region::{ArraySpan, PackedSpan};
use super::ssa::PathCondition;
use super::ssa::PositionDomain;
use crate::ir::VarId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct SummaryRegion {
    pub(super) id: VarId,
    pub(super) array: ArraySpan,
    pub(super) packed: PackedSpan,
}

pub(super) use super::position::Relation as BitDependency;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SummaryNodeKind {
    Input,
    Output,
    Interface,
    Internal,
}

#[derive(Clone, Debug)]
pub(super) struct SummaryNode {
    pub(super) region: SummaryRegion,
    pub(super) domains: Vec<PositionDomain>,
    pub(super) kind: SummaryNodeKind,
}

/// Finite dependency graph across a module boundary. Retaining graph structure
/// is essential: taking the transitive closure of a positional cycle such as
/// `x = x << 1` would otherwise enumerate one offset per declared bit.
#[derive(Clone, Debug, Default)]
pub(super) struct ModuleCombSummary {
    pub(super) nodes: Vec<SummaryNode>,
    pub(super) edges: Vec<SummaryDependency>,
    pub(super) complete: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SummaryDependency {
    pub(super) source: usize,
    pub(super) destination: usize,
    pub(super) kind: BitDependency,
    pub(super) condition: PathCondition,
}
