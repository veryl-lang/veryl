//! Share equivalent dependency nodes after statement-ordered SSA evaluation.

use super::*;

#[derive(PartialEq, Eq, Hash)]
struct InternalNode {
    inputs: Vec<(usize, PositionRelation, PathCondition)>,
    domains: Vec<PositionDomain>,
    site: Option<DefinitionSite<usize>>,
}

pub(super) struct Builder<K> {
    pub(super) graph: DependencyDag<K>,
    interned: HashMap<InternalNode, usize>,
}

impl<K> Builder<K> {
    pub(super) fn new() -> Self {
        Self {
            graph: DependencyDag {
                nodes: Vec::new(),
                edges: Vec::new(),
                roots: Vec::new(),
                domains: Vec::new(),
                sites: HashMap::default(),
            },
            interned: HashMap::default(),
        }
    }

    pub(super) fn external(&mut self, key: K) -> usize {
        let node = self.graph.nodes.len();
        self.graph.nodes.push(DependencyDagNode::External(key));
        self.graph.domains.push(Vec::new());
        node
    }

    pub(super) fn internal(
        &mut self,
        mut inputs: Vec<(usize, PositionRelation, PathCondition)>,
        mut domains: Vec<PositionDomain>,
        mut site: Option<DefinitionSite<usize>>,
    ) -> usize {
        inputs.sort_unstable();
        inputs.dedup();
        domains.sort_unstable();
        domains.dedup();
        if let Some(site) = &mut site {
            site.data_inputs.sort_unstable();
            site.data_inputs.dedup();
        }

        // An unrestricted, unconditional identity adds neither a dependency
        // nor a position boundary. Removing these aliases also lets imports
        // share subgraphs reached through different numbers of call frames.
        // Keep diagnostic sites so sharing cannot select an unrelated write.
        if domains.is_empty()
            && site.is_none()
            && let [(source, relation, condition)] = inputs.as_slice()
            && *relation == PositionRelation::default()
            && condition.is_unconditional()
        {
            return *source;
        }

        let key = InternalNode {
            inputs,
            domains,
            site,
        };
        if let Some(&node) = self.interned.get(&key) {
            return node;
        }
        let node = self.graph.nodes.len();
        self.graph.nodes.push(DependencyDagNode::Internal);
        self.graph.domains.push(key.domains.clone());
        self.graph
            .edges
            .extend(
                key.inputs
                    .iter()
                    .map(|(source, relation, condition)| DependencyDagEdge {
                        source: *source,
                        destination: node,
                        relation: *relation,
                        condition: condition.clone(),
                    }),
            );
        if let Some(site) = &key.site {
            self.graph.sites.insert(node, site.clone());
        }
        self.interned.insert(key, node);
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Point = (u8, usize, usize);

    // Independent finite reference: propagate individual source coordinates
    // through every edge for one complete valuation of the branch choices.
    fn expanded_sources(
        graph: &DependencyDag<u8>,
        choices: &PathCondition,
    ) -> Vec<Vec<HashSet<Point>>> {
        let mut values = vec![vec![HashSet::default(); 8]; graph.nodes.len()];
        for (node, kind) in graph.nodes.iter().enumerate() {
            for array in 0..2 {
                for bit in 0..4 {
                    if !graph.domains[node].is_empty()
                        && !graph.domains[node].iter().any(|domain| {
                            (domain.array_start..domain.array_start + domain.array_length)
                                .contains(&array)
                                && (domain.packed_start..domain.packed_start + domain.packed_length)
                                    .contains(&bit)
                        })
                    {
                        continue;
                    }
                    if let DependencyDagNode::External(key) = kind {
                        values[node][array * 4 + bit].insert((*key, array, bit));
                    }
                    for edge in graph.edges.iter().filter(|edge| edge.destination == node) {
                        if edge.condition.conjoin_if_compatible(choices).is_none() {
                            continue;
                        }
                        assert!(edge.source < node);
                        for source_array in 0..2 {
                            for source_bit in 0..4 {
                                if edge.relation.array.is_none_or(|offset| {
                                    source_array as isize + offset == array as isize
                                }) && edge.relation.packed.is_none_or(|offset| {
                                    source_bit as isize + offset == bit as isize
                                }) {
                                    let sources =
                                        values[edge.source][source_array * 4 + source_bit].clone();
                                    values[node][array * 4 + bit].extend(sources);
                                }
                            }
                        }
                    }
                }
            }
        }
        graph
            .roots
            .iter()
            .map(|root| values[root.unwrap()].clone())
            .collect()
    }

    #[test]
    fn shared_imports_match_expanded_guarded_and_projected_dependencies() {
        let branches = [BranchId::new(0, 0, 2), BranchId::new(0, 1, 2)];
        let mut raw = DependencyDag {
            nodes: vec![
                DependencyDagNode::External(0),
                DependencyDagNode::External(1),
            ],
            edges: Vec::new(),
            roots: Vec::new(),
            domains: vec![Vec::new(), Vec::new()],
            sites: HashMap::default(),
        };
        for stage in 0..18 {
            let source = if stage % 6 == 0 {
                0
            } else {
                raw.nodes.len() - 1
            };
            let condition = if stage % 3 == 0 {
                PathCondition::default()
            } else {
                PathCondition::default().with_choice(branches[stage % 2], stage % 2)
            };
            // Each pair is equivalent, but adjacent stages vary the input,
            // offset, guard or domain. Neither branch nor position restrictions
            // can be dropped merely to increase the amount of sharing.
            for _ in 0..2 {
                let node = raw.nodes.len();
                raw.nodes.push(DependencyDagNode::Internal);
                raw.domains.push(vec![PositionDomain {
                    array_start: 0,
                    array_length: 1 + stage % 2,
                    packed_start: stage % 3,
                    packed_length: 4 - stage % 3,
                }]);
                raw.edges.push(DependencyDagEdge {
                    source,
                    destination: node,
                    relation: PositionRelation {
                        array: Some(0),
                        packed: Some((stage % 3) as isize - 1),
                    },
                    condition: condition.clone(),
                });
                raw.edges.push(DependencyDagEdge {
                    source: 1,
                    destination: node,
                    relation: PositionRelation {
                        array: Some(0),
                        packed: None,
                    },
                    condition: PathCondition::default(),
                });
                raw.roots.push(Some(node));
            }
        }
        let remapped = [BranchId::new(1, 0, 2), BranchId::new(1, 1, 2)];
        let branch_map: HashMap<_, _> = branches.into_iter().zip(remapped).collect();
        let mut second = raw.clone();
        second.nodes[0] = DependencyDagNode::External(1);
        second.nodes[1] = DependencyDagNode::External(0);
        for edge in &mut second.edges {
            edge.condition = edge.condition.remapped(&branch_map);
        }

        let raw = Rc::new(raw);
        let mut caller = SsaStore::default();
        let inputs = [caller.read(0), caller.read(1)];
        let mut roots = Vec::new();
        for swapped in [false, true] {
            for &root in &raw.roots {
                roots.push(
                    caller.imported(
                        raw.clone(),
                        root,
                        (0..2)
                            .map(|key| {
                                (
                                    key as u8,
                                    vec![(
                                        inputs[key ^ usize::from(swapped)],
                                        PositionRelation::default(),
                                    )],
                                )
                            })
                            .collect(),
                        if swapped {
                            branch_map.clone()
                        } else {
                            HashMap::default()
                        },
                    ),
                );
            }
        }
        let shared = caller.dependency_dag(&roots, &[0, 1].into_iter().collect());
        assert!(shared.nodes.len() < raw.nodes.len() * 2);
        for valuation in 0..16 {
            let choices = branches
                .into_iter()
                .chain(remapped)
                .enumerate()
                .fold(PathCondition::default(), |choices, (index, branch)| {
                    choices.with_choice(branch, (valuation >> index) & 1)
                });
            let mut expected = expanded_sources(&raw, &choices);
            expected.extend(expanded_sources(&second, &choices));
            assert_eq!(
                expanded_sources(&shared, &choices),
                expected,
                "valuation={valuation}"
            );
        }
    }
}
