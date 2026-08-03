//! Deterministic control-flow analysis over dense, IR-independent block IDs.

use std::collections::BTreeSet;
use std::fmt;

/// A malformed dense control-flow graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfgError {
    Empty,
    InvalidRoot {
        root: usize,
        blocks: usize,
    },
    EdgeOutOfRange {
        source: usize,
        target: usize,
        blocks: usize,
    },
    Unreachable(Vec<usize>),
    InvalidGraph(&'static str),
}

impl fmt::Display for CfgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("control-flow graph is empty"),
            Self::InvalidRoot { root, blocks } => {
                write!(formatter, "CFG root {root} is outside {blocks} blocks")
            }
            Self::EdgeOutOfRange {
                source,
                target,
                blocks,
            } => write!(
                formatter,
                "CFG edge {source} -> {target} is outside {blocks} blocks"
            ),
            Self::Unreachable(blocks) => {
                formatter.write_str("CFG contains unreachable blocks:")?;
                for block in blocks {
                    write!(formatter, " {block}")?;
                }
                Ok(())
            }
            Self::InvalidGraph(message) => write!(formatter, "invalid CFG: {message}"),
        }
    }
}

impl std::error::Error for CfgError {}

/// Immediate dominators and constant-time dominance intervals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DominatorTree {
    pub idom: Vec<Option<usize>>,
    pub children: Vec<Vec<usize>>,
    enter: Vec<usize>,
    exit: Vec<usize>,
    depth: Vec<usize>,
}

impl DominatorTree {
    fn compute(successors: &[Vec<usize>], root: usize) -> Result<Self, CfgError> {
        let idom = lengauer_tarjan(successors, root)?;
        Self::from_idom(idom, root)
    }

    /// Construct dominance intervals from an independently supplied idom tree.
    pub fn from_idom(idom: Vec<Option<usize>>, root: usize) -> Result<Self, CfgError> {
        if root >= idom.len() {
            return Err(CfgError::InvalidRoot {
                root,
                blocks: idom.len(),
            });
        }
        let mut children = vec![Vec::new(); idom.len()];
        for (block, parent) in idom.iter().copied().enumerate() {
            if block == root {
                if parent.is_some() {
                    return Err(CfgError::InvalidGraph(
                        "dominator root has an immediate dominator",
                    ));
                }
                continue;
            }
            if let Some(parent) = parent {
                let Some(parent_children) = children.get_mut(parent) else {
                    return Err(CfgError::InvalidGraph(
                        "immediate dominator is out of range",
                    ));
                };
                parent_children.push(block);
            }
        }
        for block_children in &mut children {
            block_children.sort_unstable();
        }

        enum Event {
            Enter(usize, usize),
            Exit(usize),
        }
        let mut enter = vec![usize::MAX; idom.len()];
        let mut exit = vec![usize::MAX; idom.len()];
        let mut depth = vec![usize::MAX; idom.len()];
        let mut time = 0usize;
        let mut events = vec![Event::Enter(root, 0)];
        while let Some(event) = events.pop() {
            match event {
                Event::Enter(block, block_depth) => {
                    if enter[block] != usize::MAX {
                        return Err(CfgError::InvalidGraph(
                            "immediate-dominator links contain a cycle",
                        ));
                    }
                    enter[block] = time;
                    depth[block] = block_depth;
                    time += 1;
                    events.push(Event::Exit(block));
                    events.extend(
                        children[block]
                            .iter()
                            .rev()
                            .copied()
                            .map(|child| Event::Enter(child, block_depth + 1)),
                    );
                }
                Event::Exit(block) => {
                    exit[block] = time;
                    time += 1;
                }
            }
        }
        Ok(Self {
            idom,
            children,
            enter,
            exit,
            depth,
        })
    }

    #[must_use]
    pub fn dominates(&self, dominator: usize, block: usize) -> bool {
        let (Some(&dominator_enter), Some(&dominator_exit), Some(&block_enter), Some(&block_exit)) = (
            self.enter.get(dominator),
            self.exit.get(dominator),
            self.enter.get(block),
            self.exit.get(block),
        ) else {
            return false;
        };
        dominator_enter != usize::MAX
            && block_enter != usize::MAX
            && dominator_enter <= block_enter
            && block_exit <= dominator_exit
    }

    #[must_use]
    pub fn lca(&self, left: usize, right: usize) -> Option<usize> {
        let (Some(&left_depth), Some(&right_depth)) = (self.depth.get(left), self.depth.get(right))
        else {
            return None;
        };
        if left_depth == usize::MAX || right_depth == usize::MAX {
            return None;
        }
        let mut left = left;
        let mut right = right;
        while self.depth[left] > self.depth[right] {
            left = self.idom[left]?;
        }
        while self.depth[right] > self.depth[left] {
            right = self.idom[right]?;
        }
        while left != right {
            left = self.idom[left]?;
            right = self.idom[right]?;
        }
        Some(left)
    }
}

/// Post-dominance tree rooted at a synthetic common exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostDominatorTree {
    tree: DominatorTree,
    virtual_exit: usize,
    original_blocks: usize,
}

impl PostDominatorTree {
    #[must_use]
    pub fn postdominates(&self, postdominator: usize, block: usize) -> bool {
        postdominator < self.original_blocks
            && block < self.original_blocks
            && self.tree.dominates(postdominator, block)
    }

    #[must_use]
    pub fn common_postdominator(&self, left: usize, right: usize) -> Option<usize> {
        let candidate = self.tree.lca(left, right)?;
        (candidate != self.virtual_exit && candidate < self.original_blocks).then_some(candidate)
    }

    #[must_use]
    pub fn immediate_postdominator(&self, block: usize) -> Option<usize> {
        let parent = *self.tree.idom.get(block)?.as_ref()?;
        (parent != self.virtual_exit && parent < self.original_blocks).then_some(parent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StronglyConnectedRegion {
    pub blocks: Vec<usize>,
    pub entries: Vec<usize>,
    pub cyclic: bool,
    pub reducible_header: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NaturalLoop {
    pub header: usize,
    pub blocks: BTreeSet<usize>,
    pub parent: Option<usize>,
}

/// Complete analysis of one reachable dense CFG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFlowGraph {
    pub root: usize,
    pub predecessors: Vec<Vec<usize>>,
    pub successors: Vec<Vec<usize>>,
    pub dominators: DominatorTree,
    pub dominance_frontier: Vec<Vec<usize>>,
    pub postdominators: PostDominatorTree,
    pub postdominance_frontier: Vec<Vec<usize>>,
    pub controllers: Vec<Vec<usize>>,
    pub control_dependents: Vec<Vec<usize>>,
    pub sccs: Vec<StronglyConnectedRegion>,
    pub scc_for_block: Vec<usize>,
    pub loops: Vec<NaturalLoop>,
}

/// Forward-only CFG analysis used by SSA construction and machine backends.
///
/// Unlike [`ControlFlowGraph`], this does not construct postdominators or
/// control dependence. SCC membership is retained because loop-sensitive
/// placement needs to reject irreducible cycles without materializing the
/// potentially dense reverse/control-dependence graphs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardControlFlowGraph {
    pub root: usize,
    pub predecessors: Vec<Vec<usize>>,
    pub successors: Vec<Vec<usize>>,
    pub dominators: DominatorTree,
    pub dominance_frontier: Vec<Vec<usize>>,
    pub sccs: Vec<StronglyConnectedRegion>,
    pub scc_for_block: Vec<usize>,
    pub loops: Vec<NaturalLoop>,
}

impl ForwardControlFlowGraph {
    /// Analyze the forward properties of a graph without changing block IDs.
    pub fn analyze(successors: Vec<Vec<usize>>, root: usize) -> Result<Self, CfgError> {
        Self::analyze_impl(successors, root, true)
    }

    /// Analyze dominance, loops, and SCCs without constructing a dominance
    /// frontier.
    ///
    /// This is for placement clients which issue dominance queries but do not
    /// construct SSA. The returned frontier table has one empty entry per
    /// block so accidental frontier use is explicit in tests and diagnostics.
    pub fn analyze_structure(successors: Vec<Vec<usize>>, root: usize) -> Result<Self, CfgError> {
        Self::analyze_impl(successors, root, false)
    }

    fn analyze_impl(
        mut successors: Vec<Vec<usize>>,
        root: usize,
        include_frontier: bool,
    ) -> Result<Self, CfgError> {
        validate_edges(&successors, root)?;
        for outgoing in &mut successors {
            let mut seen = BTreeSet::new();
            outgoing.retain(|successor| seen.insert(*successor));
        }
        let order = reverse_postorder(&successors, root)?;
        if order.len() != successors.len() {
            let reached = order.into_iter().collect::<BTreeSet<_>>();
            return Err(CfgError::Unreachable(
                (0..successors.len())
                    .filter(|block| !reached.contains(block))
                    .collect(),
            ));
        }

        let mut predecessors = vec![Vec::new(); successors.len()];
        for (block, outgoing) in successors.iter().enumerate() {
            for &successor in outgoing {
                predecessors[successor].push(block);
            }
        }
        for incoming in &mut predecessors {
            incoming.sort_unstable();
            incoming.dedup();
        }

        let dominators = DominatorTree::compute(&successors, root)?;
        if dominators
            .idom
            .iter()
            .enumerate()
            .any(|(block, parent)| block != root && parent.is_none())
        {
            return Err(CfgError::InvalidGraph(
                "reachable block has no immediate dominator",
            ));
        }
        let dominance_frontier = if include_frontier {
            dominance_frontiers(&successors, &dominators, root)
        } else {
            vec![Vec::new(); successors.len()]
        };
        let loops = natural_loops(&predecessors, &successors, &dominators)?;
        let (sccs, scc_for_block) =
            strongly_connected_regions(&predecessors, &successors, &dominators, root);

        Ok(Self {
            root,
            predecessors,
            successors,
            dominators,
            dominance_frontier,
            sccs,
            scc_for_block,
            loops,
        })
    }
}

impl ControlFlowGraph {
    /// Analyze a graph without changing the caller's block numbering.
    pub fn analyze(successors: Vec<Vec<usize>>, root: usize) -> Result<Self, CfgError> {
        let forward = ForwardControlFlowGraph::analyze(successors, root)?;
        Self::finish(forward, true)
    }

    /// Analyze dominance, post-dominance, loops, and SCCs without constructing
    /// either dominance frontier or the potentially dense control-dependence
    /// relation.
    ///
    /// Placement clients which only need legal region boundaries should use
    /// this mode. Its graph tables remain linear in the input CFG size.
    pub fn analyze_structure(successors: Vec<Vec<usize>>, root: usize) -> Result<Self, CfgError> {
        let forward = ForwardControlFlowGraph::analyze_structure(successors, root)?;
        Self::finish(forward, false)
    }

    fn finish(forward: ForwardControlFlowGraph, include_frontiers: bool) -> Result<Self, CfgError> {
        let (postdominators, postdominance_frontier) = build_postdominators(
            &forward.predecessors,
            &forward.successors,
            include_frontiers,
        )?;
        let controllers = postdominance_frontier.clone();
        let mut control_dependents = vec![Vec::new(); forward.successors.len()];
        if include_frontiers {
            for (dependent, dependent_controllers) in controllers.iter().enumerate() {
                for &controller in dependent_controllers {
                    control_dependents[controller].push(dependent);
                }
            }
            for dependents in &mut control_dependents {
                dependents.sort_unstable();
                dependents.dedup();
            }
        }
        Ok(Self {
            root: forward.root,
            predecessors: forward.predecessors,
            successors: forward.successors,
            dominators: forward.dominators,
            dominance_frontier: forward.dominance_frontier,
            postdominators,
            postdominance_frontier,
            controllers,
            control_dependents,
            sccs: forward.sccs,
            scc_for_block: forward.scc_for_block,
            loops: forward.loops,
        })
    }
}

fn validate_edges(successors: &[Vec<usize>], root: usize) -> Result<(), CfgError> {
    if successors.is_empty() {
        return Err(CfgError::Empty);
    }
    if root >= successors.len() {
        return Err(CfgError::InvalidRoot {
            root,
            blocks: successors.len(),
        });
    }
    for (source, outgoing) in successors.iter().enumerate() {
        if let Some(&target) = outgoing.iter().find(|&&target| target >= successors.len()) {
            return Err(CfgError::EdgeOutOfRange {
                source,
                target,
                blocks: successors.len(),
            });
        }
    }
    Ok(())
}

/// Iterative DFS reverse postorder in the caller's block-number domain.
pub fn reverse_postorder(successors: &[Vec<usize>], root: usize) -> Result<Vec<usize>, CfgError> {
    validate_edges(successors, root)?;
    let mut visited = vec![false; successors.len()];
    let mut postorder = Vec::with_capacity(successors.len());
    visited[root] = true;
    let mut stack = vec![(root, 0usize)];
    while let Some((block, next_successor)) = stack.last_mut() {
        if *next_successor == successors[*block].len() {
            postorder.push(*block);
            stack.pop();
            continue;
        }
        let successor = successors[*block][*next_successor];
        *next_successor += 1;
        if !visited[successor] {
            visited[successor] = true;
            stack.push((successor, 0));
        }
    }
    postorder.reverse();
    Ok(postorder)
}

/// Lengauer--Tarjan immediate dominators over a dense graph.
fn lengauer_tarjan(successors: &[Vec<usize>], root: usize) -> Result<Vec<Option<usize>>, CfgError> {
    validate_edges(successors, root)?;
    let mut dfs_number = vec![0usize; successors.len()];
    let mut vertex = vec![usize::MAX];
    let mut parent = vec![0usize; successors.len() + 1];
    dfs_number[root] = 1;
    vertex.push(root);
    let mut stack = vec![(root, 0usize)];
    while let Some((block, next_successor)) = stack.last_mut() {
        if *next_successor == successors[*block].len() {
            stack.pop();
            continue;
        }
        let successor = successors[*block][*next_successor];
        *next_successor += 1;
        if dfs_number[successor] == 0 {
            let number = vertex.len();
            dfs_number[successor] = number;
            vertex.push(successor);
            parent[number] = dfs_number[*block];
            stack.push((successor, 0));
        }
    }

    let reachable = vertex.len() - 1;
    let mut predecessors = vec![Vec::new(); reachable + 1];
    for (source, outgoing) in successors.iter().enumerate() {
        let source_number = dfs_number[source];
        if source_number == 0 {
            continue;
        }
        for &target in outgoing {
            let target_number = dfs_number[target];
            if target_number != 0 {
                predecessors[target_number].push(source_number);
            }
        }
    }

    let mut semi = (0..=reachable).collect::<Vec<_>>();
    let mut idom_number = vec![0usize; reachable + 1];
    let mut ancestor = vec![0usize; reachable + 1];
    let mut label = (0..=reachable).collect::<Vec<_>>();
    let mut bucket = vec![Vec::<usize>::new(); reachable + 1];

    fn eval(value: usize, ancestor: &mut [usize], label: &mut [usize], semi: &[usize]) -> usize {
        if ancestor[value] == 0 {
            return label[value];
        }
        let mut path = Vec::new();
        let mut current = value;
        while ancestor[current] != 0 && ancestor[ancestor[current]] != 0 {
            path.push(current);
            current = ancestor[current];
        }
        for node in path.into_iter().rev() {
            let parent = ancestor[node];
            if semi[label[parent]] < semi[label[node]] {
                label[node] = label[parent];
            }
            ancestor[node] = ancestor[parent];
        }
        label[value]
    }

    for block in (2..=reachable).rev() {
        for &predecessor in &predecessors[block] {
            let representative = eval(predecessor, &mut ancestor, &mut label, &semi);
            semi[block] = semi[block].min(semi[representative]);
        }
        bucket[semi[block]].push(block);
        let block_parent = parent[block];
        if block_parent == 0 {
            return Err(CfgError::InvalidGraph("non-root DFS node has no parent"));
        }
        ancestor[block] = block_parent;
        let pending = std::mem::take(&mut bucket[block_parent]);
        for candidate in pending {
            let representative = eval(candidate, &mut ancestor, &mut label, &semi);
            idom_number[candidate] = if semi[representative] < semi[candidate] {
                representative
            } else {
                block_parent
            };
        }
    }
    for block in 2..=reachable {
        if idom_number[block] != semi[block] {
            let parent = idom_number[block];
            if parent == 0 {
                return Err(CfgError::InvalidGraph(
                    "dominator correction references no parent",
                ));
            }
            idom_number[block] = idom_number[parent];
        }
    }

    let mut result = vec![None; successors.len()];
    for block in 2..=reachable {
        let parent = idom_number[block];
        if parent == 0 || parent >= vertex.len() {
            return Err(CfgError::InvalidGraph(
                "computed immediate dominator is out of range",
            ));
        }
        result[vertex[block]] = Some(vertex[parent]);
    }
    Ok(result)
}

fn dominance_frontiers(
    successors: &[Vec<usize>],
    dominators: &DominatorTree,
    root: usize,
) -> Vec<Vec<usize>> {
    let mut frontiers = vec![BTreeSet::<usize>::new(); successors.len()];
    let mut tree_postorder = Vec::with_capacity(successors.len());
    let mut stack = vec![(root, false)];
    while let Some((block, expanded)) = stack.pop() {
        if expanded {
            tree_postorder.push(block);
            continue;
        }
        stack.push((block, true));
        stack.extend(
            dominators.children[block]
                .iter()
                .rev()
                .copied()
                .map(|child| (child, false)),
        );
    }
    for block in tree_postorder {
        for &successor in &successors[block] {
            if dominators.idom[successor] != Some(block) {
                frontiers[block].insert(successor);
            }
        }
        for &child in &dominators.children[block] {
            let child_frontier = frontiers[child].iter().copied().collect::<Vec<_>>();
            for member in child_frontier {
                if dominators.idom[member] != Some(block) {
                    frontiers[block].insert(member);
                }
            }
        }
    }
    frontiers
        .into_iter()
        .map(|frontier| frontier.into_iter().collect())
        .collect()
}

fn build_postdominators(
    predecessors: &[Vec<usize>],
    successors: &[Vec<usize>],
    include_frontier: bool,
) -> Result<(PostDominatorTree, Vec<Vec<usize>>), CfgError> {
    let original_blocks = successors.len();
    let virtual_exit = original_blocks;
    let mut reverse_successors = vec![Vec::new(); original_blocks + 1];
    reverse_successors[virtual_exit] = successors
        .iter()
        .enumerate()
        .filter_map(|(block, outgoing)| outgoing.is_empty().then_some(block))
        .collect();
    for (block, incoming) in predecessors.iter().enumerate() {
        reverse_successors[block] = incoming.clone();
    }
    let tree = DominatorTree::compute(&reverse_successors, virtual_exit)?;
    let frontiers = if include_frontier {
        let mut frontiers = dominance_frontiers(&reverse_successors, &tree, virtual_exit);
        frontiers.truncate(original_blocks);
        for frontier in &mut frontiers {
            frontier.retain(|block| *block < original_blocks);
        }
        frontiers
    } else {
        vec![Vec::new(); original_blocks]
    };
    Ok((
        PostDominatorTree {
            tree,
            virtual_exit,
            original_blocks,
        },
        frontiers,
    ))
}

fn strongly_connected_regions(
    predecessors: &[Vec<usize>],
    successors: &[Vec<usize>],
    dominators: &DominatorTree,
    root: usize,
) -> (Vec<StronglyConnectedRegion>, Vec<usize>) {
    let mut visited = vec![false; successors.len()];
    let mut postorder = Vec::with_capacity(successors.len());
    for seed in std::iter::once(root).chain((0..successors.len()).filter(|block| *block != root)) {
        if visited[seed] {
            continue;
        }
        visited[seed] = true;
        let mut stack = vec![(seed, 0usize)];
        while let Some((block, next_successor)) = stack.last_mut() {
            if *next_successor == successors[*block].len() {
                postorder.push(*block);
                stack.pop();
                continue;
            }
            let successor = successors[*block][*next_successor];
            *next_successor += 1;
            if !visited[successor] {
                visited[successor] = true;
                stack.push((successor, 0));
            }
        }
    }

    let mut component = vec![usize::MAX; successors.len()];
    let mut raw_components = Vec::<Vec<usize>>::new();
    for seed in postorder.into_iter().rev() {
        if component[seed] != usize::MAX {
            continue;
        }
        let component_id = raw_components.len();
        component[seed] = component_id;
        let mut members = Vec::new();
        let mut stack = vec![seed];
        while let Some(block) = stack.pop() {
            members.push(block);
            for &predecessor in predecessors[block].iter().rev() {
                if component[predecessor] == usize::MAX {
                    component[predecessor] = component_id;
                    stack.push(predecessor);
                }
            }
        }
        members.sort_unstable();
        raw_components.push(members);
    }

    let regions = raw_components
        .iter()
        .enumerate()
        .map(|(component_id, members)| {
            let mut entries = BTreeSet::new();
            for &block in members {
                if block == root
                    || predecessors[block]
                        .iter()
                        .any(|predecessor| component[*predecessor] != component_id)
                {
                    entries.insert(block);
                }
            }
            let cyclic = members.len() > 1
                || members
                    .first()
                    .is_some_and(|block| successors[*block].contains(block));
            let reducible_header = if entries.len() == 1 {
                entries.iter().next().copied().filter(|header| {
                    members
                        .iter()
                        .all(|block| dominators.dominates(*header, *block))
                })
            } else {
                None
            };
            StronglyConnectedRegion {
                blocks: members.clone(),
                entries: entries.into_iter().collect(),
                cyclic,
                reducible_header,
            }
        })
        .collect();
    (regions, component)
}

fn natural_loops(
    predecessors: &[Vec<usize>],
    successors: &[Vec<usize>],
    dominators: &DominatorTree,
) -> Result<Vec<NaturalLoop>, CfgError> {
    let mut by_header = vec![None::<BTreeSet<usize>>; successors.len()];
    for (tail, outgoing) in successors.iter().enumerate() {
        for &header in outgoing {
            if !dominators.dominates(header, tail) {
                continue;
            }
            let blocks = by_header[header].get_or_insert_with(BTreeSet::new);
            blocks.insert(header);
            let mut stack = vec![tail];
            while let Some(block) = stack.pop() {
                if blocks.insert(block) {
                    stack.extend(predecessors[block].iter().copied());
                }
            }
        }
    }
    let mut loops = by_header
        .into_iter()
        .enumerate()
        .filter_map(|(header, blocks)| {
            blocks.map(|blocks| NaturalLoop {
                header,
                blocks,
                parent: None,
            })
        })
        .collect::<Vec<_>>();
    loops.sort_by_key(|natural_loop| (natural_loop.blocks.len(), natural_loop.header));

    let mut innermost_for_block = vec![None::<usize>; successors.len()];
    for child in (0..loops.len()).rev() {
        let parent = innermost_for_block[loops[child].header];
        if parent.is_some_and(|parent| !loops[parent].blocks.is_superset(&loops[child].blocks)) {
            return Err(CfgError::InvalidGraph(
                "natural loops overlap without nesting",
            ));
        }
        loops[child].parent = parent;
        for &block in &loops[child].blocks {
            innermost_for_block[block] = Some(child);
        }
    }
    Ok(loops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_graph() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1], vec![2], vec![]], 0).unwrap();
        assert_eq!(cfg.dominators.idom, vec![None, Some(0), Some(1)]);
        assert!(cfg.dominators.dominates(0, 2));
        assert!(cfg.postdominators.postdominates(2, 0));
    }

    #[test]
    fn diamond_frontier_and_control_dependence() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1, 2], vec![3], vec![3], vec![]], 0).unwrap();
        assert_eq!(cfg.dominance_frontier[1], vec![3]);
        assert_eq!(cfg.dominance_frontier[2], vec![3]);
        assert_eq!(cfg.controllers[1], vec![0]);
        assert_eq!(cfg.controllers[2], vec![0]);
        assert_eq!(cfg.control_dependents[0], vec![1, 2]);
    }

    #[test]
    fn multiple_exits_use_virtual_postdominator() {
        let cfg = ControlFlowGraph::analyze(vec![vec![1, 2], vec![], vec![]], 0).unwrap();
        assert_eq!(cfg.postdominators.common_postdominator(1, 2), None);
        assert!(!cfg.postdominators.postdominates(1, 0));
    }

    #[test]
    fn natural_and_irreducible_regions() {
        let natural =
            ControlFlowGraph::analyze(vec![vec![1], vec![2, 3], vec![1], vec![]], 0).unwrap();
        assert_eq!(natural.loops.len(), 1);
        assert_eq!(natural.loops[0].header, 1);

        let irreducible =
            ControlFlowGraph::analyze(vec![vec![1, 2], vec![2], vec![1, 3], vec![]], 0).unwrap();
        let region = irreducible
            .sccs
            .iter()
            .find(|region| region.cyclic && region.blocks.len() == 2)
            .unwrap();
        assert_eq!(region.entries, vec![1, 2]);
        assert_eq!(region.reducible_header, None);
    }

    #[test]
    fn forward_analysis_retains_sccs_without_control_dependence() {
        let successors = vec![vec![1, 2], vec![2], vec![1, 3], vec![]];
        let full = ControlFlowGraph::analyze(successors.clone(), 0).unwrap();
        let forward = ForwardControlFlowGraph::analyze(successors.clone(), 0).unwrap();
        let structure = ForwardControlFlowGraph::analyze_structure(successors, 0).unwrap();
        assert_eq!(forward.dominators, full.dominators);
        assert_eq!(forward.dominance_frontier, full.dominance_frontier);
        assert_eq!(forward.sccs, full.sccs);
        assert_eq!(forward.scc_for_block, full.scc_for_block);
        assert_eq!(forward.loops, full.loops);
        assert_eq!(structure.dominators, full.dominators);
        assert_eq!(structure.sccs, full.sccs);
        assert!(structure.dominance_frontier.iter().all(Vec::is_empty));
    }

    #[test]
    fn bidirectional_structure_retains_postdominators_without_control_dependence() {
        let successors = vec![vec![1, 2], vec![3], vec![3], vec![]];
        let full = ControlFlowGraph::analyze(successors.clone(), 0).unwrap();
        let structure = ControlFlowGraph::analyze_structure(successors, 0).unwrap();
        assert_eq!(structure.dominators, full.dominators);
        assert_eq!(structure.postdominators, full.postdominators);
        assert_eq!(structure.sccs, full.sccs);
        assert!(structure.dominance_frontier.iter().all(Vec::is_empty));
        assert!(structure.postdominance_frontier.iter().all(Vec::is_empty));
        assert!(structure.controllers.iter().all(Vec::is_empty));
        assert!(structure.control_dependents.iter().all(Vec::is_empty));
    }

    #[test]
    fn multiple_backedges_to_one_header_form_their_union() {
        let cfg =
            ControlFlowGraph::analyze(vec![vec![1], vec![2, 3], vec![1], vec![1]], 0).unwrap();

        assert_eq!(cfg.loops.len(), 1);
        assert_eq!(cfg.loops[0].header, 1);
        assert_eq!(cfg.loops[0].blocks, BTreeSet::from([1, 2, 3]));
        assert_eq!(cfg.loops[0].parent, None);
    }

    #[test]
    fn deeply_nested_loop_forest_has_direct_parents() {
        const DEPTH: usize = 128;
        const BLOCKS: usize = DEPTH + 1;
        let mut successors = vec![Vec::new(); BLOCKS];
        for (block, outgoing) in successors.iter_mut().enumerate().take(DEPTH) {
            outgoing.push(block + 1);
        }
        for header in 1..=DEPTH {
            successors[DEPTH].push(header);
        }

        let cfg = ControlFlowGraph::analyze(successors, 0).unwrap();

        assert_eq!(cfg.loops.len(), DEPTH);
        for (child, loop_info) in cfg.loops.iter().enumerate().take(DEPTH - 1) {
            assert_eq!(loop_info.parent, Some(child + 1));
            assert_eq!(loop_info.header, DEPTH - child);
        }
        assert_eq!(cfg.loops[DEPTH - 1].header, 1);
        assert_eq!(cfg.loops[DEPTH - 1].parent, None);
    }

    #[test]
    fn reports_bad_edges_and_unreachable_blocks() {
        assert_eq!(
            ControlFlowGraph::analyze(vec![vec![2], vec![]], 0).unwrap_err(),
            CfgError::EdgeOutOfRange {
                source: 0,
                target: 2,
                blocks: 2,
            }
        );
        assert_eq!(
            ControlFlowGraph::analyze(vec![vec![], vec![]], 0).unwrap_err(),
            CfgError::Unreachable(vec![1])
        );
    }

    #[test]
    fn deep_graph_is_iterative() {
        const BLOCKS: usize = 20_000;
        let mut successors = vec![Vec::new(); BLOCKS];
        for (block, outgoing) in successors.iter_mut().enumerate().take(BLOCKS - 1) {
            outgoing.push(block + 1);
        }
        let cfg = ControlFlowGraph::analyze(successors, 0).unwrap();
        assert!(cfg.dominators.dominates(0, BLOCKS - 1));
        assert!(cfg.postdominators.postdominates(BLOCKS - 1, 0));
    }
}
