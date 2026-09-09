//! Share equivalent dependency nodes after statement-ordered SSA evaluation.

use super::*;

type MappedBindings<K> = Rc<Vec<(K, Vec<(usize, PositionRelation)>)>>;

#[derive(PartialEq, Eq, Hash)]
struct InvocationKey<K> {
    graph: usize,
    bindings: MappedBindings<K>,
    branches: Vec<(BranchId, BranchId)>,
}

struct Invocation<K> {
    bindings: MappedBindings<K>,
    mapped: HashMap<usize, usize>,
}

pub(super) struct Imports<K> {
    incoming: HashMap<usize, Vec<Vec<usize>>>,
    identities: HashMap<(usize, usize, usize), usize>,
    equivalent: HashMap<InvocationKey<K>, usize>,
    invocations: Vec<Invocation<K>>,
}

impl<K> Default for Imports<K> {
    fn default() -> Self {
        Self {
            incoming: HashMap::default(),
            identities: HashMap::default(),
            equivalent: HashMap::default(),
            invocations: Vec::new(),
        }
    }
}

impl<K: Copy + Eq + Hash + Ord> Imports<K> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn inline(
        &mut self,
        graph: &Rc<DependencyDag<K>>,
        root: Option<usize>,
        bindings: &Rc<HashMap<K, Vec<(VersionId, PositionRelation)>>>,
        branches: &Rc<HashMap<BranchId, BranchId>>,
        parent: &HashMap<(VersionId, bool), Option<usize>>,
        builder: &mut Builder<K>,
        work: &mut usize,
    ) -> Option<Option<usize>> {
        let Some(root) = root else {
            return Some(None);
        };
        let graph_id = Rc::as_ptr(graph) as usize;
        let identity = (
            graph_id,
            Rc::as_ptr(bindings) as usize,
            Rc::as_ptr(branches) as usize,
        );
        let invocation = if let Some(&invocation) = self.identities.get(&identity) {
            invocation
        } else {
            // Normalize actuals once per call, not once per result region.
            // Still recognize equivalent calls after SSA aliases collapse.
            *work = work.checked_sub(bindings.len().saturating_add(branches.len()))?;
            let mut mapped_bindings = Vec::with_capacity(bindings.len());
            for (&key, sources) in bindings.iter() {
                *work = work.checked_sub(sources.len())?;
                let mut sources = sources
                    .iter()
                    .filter_map(|(source, relation)| {
                        parent[&(*source, true)].map(|source| (source, *relation))
                    })
                    .collect::<Vec<_>>();
                sources.sort_unstable();
                sources.dedup();
                mapped_bindings.push((key, sources));
            }
            mapped_bindings.sort_unstable_by_key(|(key, _)| *key);
            let mut mapped_branches = branches
                .iter()
                .map(|(&source, &destination)| (source, destination))
                .collect::<Vec<_>>();
            mapped_branches.sort_unstable();
            let key = InvocationKey {
                graph: graph_id,
                bindings: Rc::new(mapped_bindings),
                branches: mapped_branches,
            };
            let invocation = *self.equivalent.entry(key).or_insert_with_key(|key| {
                let invocation = self.invocations.len();
                self.invocations.push(Invocation {
                    bindings: key.bindings.clone(),
                    mapped: HashMap::default(),
                });
                invocation
            });
            self.identities.insert(identity, invocation);
            invocation
        };
        let invocation = &mut self.invocations[invocation];
        if let Some(&node) = invocation.mapped.get(&root) {
            return Some(Some(node));
        }

        let incoming = match self.incoming.entry(graph_id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                *work = work.checked_sub(graph.nodes.len().saturating_add(graph.edges.len()))?;
                let mut incoming = vec![Vec::new(); graph.nodes.len()];
                for (index, edge) in graph.edges.iter().enumerate() {
                    incoming[edge.destination].push(index);
                }
                entry.insert(incoming)
            }
        };
        let mut retained = HashSet::default();
        let mut queue = VecDeque::from([root]);
        retained.insert(root);
        while let Some(node) = queue.pop_front() {
            // Charge only newly reached nodes, but include their guard and
            // domain payloads before cloning. Other roots reuse this import.
            *work = work.checked_sub(graph.domains[node].len().saturating_add(1))?;
            for &index in &incoming[node] {
                let edge = &graph.edges[index];
                *work = work.checked_sub(edge.condition.branch_count().saturating_add(1))?;
                if !invocation.mapped.contains_key(&edge.source) && retained.insert(edge.source) {
                    queue.push_back(edge.source);
                }
            }
        }
        let mut retained = retained.into_iter().collect::<Vec<_>>();
        retained.sort_unstable();
        // Exported children are topologically ordered. Map each once across
        // all roots, including shared predecessors and diagnostic sites.
        for child in retained {
            let mut inputs = incoming[child]
                .iter()
                .map(|&index| {
                    let edge = &graph.edges[index];
                    debug_assert!(edge.source < child);
                    (
                        invocation.mapped[&edge.source],
                        edge.relation,
                        edge.condition.remapped(branches),
                    )
                })
                .collect::<Vec<_>>();
            if let DependencyDagNode::External(key) = &graph.nodes[child]
                && let Ok(index) = invocation
                    .bindings
                    .binary_search_by_key(key, |(key, _)| *key)
            {
                let sources = &invocation.bindings[index].1;
                *work = work.checked_sub(sources.len())?;
                inputs.extend(
                    sources
                        .iter()
                        .map(|&(source, relation)| (source, relation, PathCondition::default())),
                );
            }
            let site = graph.sites.get(&child).map(|site| DefinitionSite {
                token: site.token,
                data_inputs: site
                    .data_inputs
                    .iter()
                    .filter_map(|input| invocation.mapped.get(input).copied())
                    .collect(),
            });
            let node = if let DependencyDagNode::Replicated { stride } = graph.nodes[child] {
                builder.replicated(inputs, graph.domains[child].clone(), site, stride)
            } else {
                builder.internal(inputs, graph.domains[child].clone(), site)
            };
            invocation.mapped.insert(child, node);
        }
        Some(invocation.mapped.get(&root).copied())
    }
}

#[derive(PartialEq, Eq, Hash)]
struct InternalNode {
    inputs: Vec<(usize, PositionRelation, PathCondition)>,
    domains: Vec<PositionDomain>,
    site: Option<DefinitionSite<usize>>,
    replication: Option<isize>,
}

pub(super) struct Builder<K> {
    pub(super) graph: DependencyDag<K>,
    interned: HashMap<InternalNode, usize>,
    replicated_sources: HashMap<usize, usize>,
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
            replicated_sources: HashMap::default(),
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
        inputs: Vec<(usize, PositionRelation, PathCondition)>,
        domains: Vec<PositionDomain>,
        site: Option<DefinitionSite<usize>>,
    ) -> usize {
        self.operation(inputs, domains, site, None)
    }

    pub(super) fn replicated(
        &mut self,
        inputs: Vec<(usize, PositionRelation, PathCondition)>,
        domains: Vec<PositionDomain>,
        site: Option<DefinitionSite<usize>>,
        stride: isize,
    ) -> usize {
        self.operation(inputs, domains, site, Some(stride))
    }

    fn operation(
        &mut self,
        mut inputs: Vec<(usize, PositionRelation, PathCondition)>,
        mut domains: Vec<PositionDomain>,
        mut site: Option<DefinitionSite<usize>>,
        mut replication: Option<isize>,
    ) -> usize {
        inputs.sort_unstable();
        inputs.dedup();
        domains.sort_unstable();
        domains.dedup();
        if let Some(site) = &mut site {
            site.data_inputs.sort_unstable();
            site.data_inputs.dedup();
        }

        // An unconditional identity with no additional position boundary
        // adds no dependency. Removing these aliases also lets imports share
        // subgraphs reached through different numbers of call frames.
        // Keep diagnostic sites so sharing cannot select an unrelated write.
        if replication.is_none()
            && site.is_none()
            && let [(source, relation, condition)] = inputs.as_slice()
            && *relation == PositionRelation::default()
            && condition.is_unconditional()
            && (domains.is_empty() || domains == self.graph.domains[*source])
        {
            return *source;
        }

        // Repeating complete adjacent blocks of a repetition is one larger
        // repetition. Retain clipping, array coordinates and diagnostic sites;
        // use the recorded seed instead of scanning predecessor paths.
        if let Some(stride) = replication
            && site.is_none()
            && let [(source, relation, condition)] = inputs.as_slice()
            && *relation == PositionRelation::default()
            && condition.is_unconditional()
            && let Some(&seed) = self.replicated_sources.get(source)
            && !self.graph.sites.contains_key(source)
            && let DependencyDagNode::Replicated { stride: inner } = self.graph.nodes[*source]
            && !self.graph.domains[seed].is_empty()
            && self.graph.domains[seed].iter().all(|domain| {
                domain
                    .packed_start
                    .checked_add(domain.packed_length)
                    .is_some_and(|end| end <= inner as usize)
            })
            && let [outer_domain] = domains.as_slice()
            && let [inner_domain] = self.graph.domains[*source].as_slice()
            && inner_domain.array_start == outer_domain.array_start
            && inner_domain.array_length == outer_domain.array_length
            && inner_domain.packed_start == 0
            && outer_domain.packed_start == 0
            && inner_domain.packed_length == stride as usize
            && inner_domain.packed_length.is_multiple_of(inner as usize)
        {
            inputs[0].0 = seed;
            replication = Some(inner);
        }

        let key = InternalNode {
            inputs,
            domains,
            site,
            replication,
        };
        if let Some(&node) = self.interned.get(&key) {
            return node;
        }
        let node = self.graph.nodes.len();
        self.graph.nodes.push(match replication {
            Some(stride) => DependencyDagNode::Replicated { stride },
            None => DependencyDagNode::Internal,
        });
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
        if replication.is_some()
            && let [(source, relation, condition)] = key.inputs.as_slice()
            && *relation == PositionRelation::default()
            && condition.is_unconditional()
        {
            self.replicated_sources.insert(node, *source);
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
                    if let DependencyDagNode::Replicated { stride } = kind
                        && let Some(previous) = bit.checked_sub(*stride as usize)
                    {
                        let sources = values[node][array * 4 + previous].clone();
                        values[node][array * 4 + bit].extend(sources);
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
                raw.nodes.push(if stage % 4 == 0 {
                    DependencyDagNode::Replicated {
                        stride: 1 + (stage / 4 % 2) as isize,
                    }
                } else {
                    DependencyDagNode::Internal
                });
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
                            .collect::<HashMap<_, _>>()
                            .into(),
                        Rc::new(if swapped {
                            branch_map.clone()
                        } else {
                            HashMap::default()
                        }),
                    ),
                );
            }
        }
        let shared = caller.dependency_dag(&roots, |key| [0, 1].contains(key));
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

    #[test]
    fn nested_replication_matches_individual_bits_with_clipped_seeds() {
        for seed_start in 0usize..3 {
            for stride in 1..=2 {
                let mut builder = Builder::new();
                let input = builder.external(0u8);
                let identity = |source| {
                    vec![(
                        source,
                        PositionRelation::default(),
                        PathCondition::default(),
                    )]
                };
                let domain = |start, length| {
                    vec![PositionDomain {
                        array_start: 1,
                        array_length: 1,
                        packed_start: start,
                        packed_length: length,
                    }]
                };
                let seed = builder.internal(identity(input), domain(seed_start, 1), None);
                let inner = builder.replicated(identity(seed), domain(0, 2), None, stride);
                let alias = builder.internal(identity(inner), domain(0, 2), None);
                let outer = builder.replicated(identity(alias), domain(0, 4), None, 2);
                assert_eq!(alias, inner);
                assert_eq!(
                    outer,
                    builder.replicated(identity(alias), domain(0, 4), None, 2)
                );
                builder.graph.roots.push(Some(outer));
                let actual = expanded_sources(&builder.graph, &PathCondition::default());
                for array in 0..2 {
                    for bit in 0usize..4 {
                        let expected = if array == 1
                            && seed_start < 2
                            && (bit % 2)
                                .checked_sub(seed_start)
                                .is_some_and(|offset| offset.is_multiple_of(stride as usize))
                        {
                            HashSet::from_iter([(0, 1, seed_start)])
                        } else {
                            HashSet::default()
                        };
                        assert_eq!(
                            actual[0][array * 4 + bit],
                            expected,
                            "seed={seed_start}, stride={stride}, array={array}, bit={bit}"
                        );
                    }
                }
            }
        }
    }
}
