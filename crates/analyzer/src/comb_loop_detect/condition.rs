//! Branch conditions as reduced ordered binary decision diagrams.
//!
//! A path condition is a set of branch valuations: every branch takes one of
//! its arms. Each branch is encoded by the binary digits of its arm index,
//! most significant first, and branches are ordered by first use, which
//! follows statement order. Codes at or above the arm count do not denote an
//! arm. Every stored function is true on such codes ("don't care"), so two
//! conditions with the same valuations are the same node, and the condition
//! allowing every arm of a branch is exactly `true`.
//!
//! Conjunction and disjunction preserve that form. Satisfiability and
//! implication restrict a function to valid codes of the branches it reads
//! before comparing with `false`. All operations are exact, so a disjunction
//! never widens a condition, and equal conditions are found by identity.
//!
//! Nodes live in a per-thread manager that `reset` clears at the start of
//! each analysis; no condition outlives the analysis that created it.

use super::ssa::BranchId;
use crate::HashMap;
use std::cell::RefCell;

const FALSE: u32 = 0;
const TRUE: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::comb_loop_detect) struct PathCondition(u32);

impl Default for PathCondition {
    fn default() -> Self {
        Self(TRUE)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Node {
    variable: u32,
    low: u32,
    high: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Operation {
    And,
    Or,
}

struct BranchVariables {
    first: u32,
    bits: u32,
}

#[derive(Default)]
struct Manager {
    nodes: Vec<Node>,
    unique: HashMap<Node, u32>,
    operations: HashMap<(Operation, u32, u32), u32>,
    negations: HashMap<u32, u32>,
    branches: HashMap<BranchId, BranchVariables>,
    /// The branch and digit of every variable, in variable order.
    variables: Vec<(BranchId, u32)>,
    sizes: HashMap<u32, usize>,
    supports: HashMap<u32, std::rc::Rc<[BranchId]>>,
    valid: HashMap<BranchId, u32>,
}

thread_local! {
    static MANAGER: RefCell<Manager> = RefCell::new(Manager::new());
}

/// Discard every condition. Call only when none can be used again.
pub(in crate::comb_loop_detect) fn reset() {
    MANAGER.with(|manager| *manager.borrow_mut() = Manager::new());
}

fn with<T>(f: impl FnOnce(&mut Manager) -> T) -> T {
    MANAGER.with(|manager| f(&mut manager.borrow_mut()))
}

impl Manager {
    fn new() -> Self {
        let terminal = Node {
            variable: u32::MAX,
            low: 0,
            high: 0,
        };
        Self {
            nodes: vec![
                terminal,
                Node {
                    low: 1,
                    high: 1,
                    ..terminal
                },
            ],
            ..Self::default()
        }
    }

    fn variable(&self, node: u32) -> u32 {
        self.nodes[node as usize].variable
    }

    fn make(&mut self, variable: u32, low: u32, high: u32) -> u32 {
        if low == high {
            return low;
        }
        let node = Node {
            variable,
            low,
            high,
        };
        if let Some(&id) = self.unique.get(&node) {
            return id;
        }
        let id = u32::try_from(self.nodes.len()).expect("condition nodes fit in u32");
        self.nodes.push(node);
        self.unique.insert(node, id);
        id
    }

    fn branch(&mut self, branch: BranchId) -> (u32, u32) {
        if let Some(variables) = self.branches.get(&branch) {
            return (variables.first, variables.bits);
        }
        let arms = branch.arms().max(1);
        let bits = usize::BITS - (arms - 1).leading_zeros();
        let first = u32::try_from(self.variables.len()).expect("condition variables fit in u32");
        for digit in 0..bits {
            self.variables.push((branch, digit));
        }
        self.branches
            .insert(branch, BranchVariables { first, bits });
        (first, bits)
    }

    /// Codes in `[start, end)` or at least `arms`, as digits from `level`.
    fn codes(
        &mut self,
        first: u32,
        bits: u32,
        level: u32,
        base: usize,
        start: usize,
        end: usize,
    ) -> u32 {
        let width = 1usize << (bits - level);
        let (low, high) = (base, base + width);
        if start <= low && high <= end {
            return TRUE;
        }
        if high <= start || end <= low {
            return FALSE;
        }
        let half = width / 2;
        let zero = self.codes(first, bits, level + 1, base, start, end);
        let one = self.codes(first, bits, level + 1, base + half, start, end);
        self.make(first + level, zero, one)
    }

    /// Arms `[start, end)` of `branch`, true on invalid codes.
    fn arms(&mut self, branch: BranchId, start: usize, end: usize) -> u32 {
        let (first, bits) = self.branch(branch);
        if bits == 0 {
            return if start < end { TRUE } else { FALSE };
        }
        let allowed = self.codes(first, bits, 0, 0, start, end);
        let invalid = self.codes(first, bits, 0, 0, branch.arms(), 1 << bits);
        self.apply(Operation::Or, allowed, invalid)
    }

    fn validity(&mut self, branch: BranchId) -> u32 {
        if let Some(&valid) = self.valid.get(&branch) {
            return valid;
        }
        let (first, bits) = self.branch(branch);
        let valid = if bits == 0 {
            TRUE
        } else {
            self.codes(first, bits, 0, 0, 0, branch.arms())
        };
        self.valid.insert(branch, valid);
        valid
    }

    fn apply(&mut self, operation: Operation, left: u32, right: u32) -> u32 {
        match operation {
            Operation::And => {
                if left == FALSE || right == FALSE {
                    return FALSE;
                }
                if left == TRUE {
                    return right;
                }
                if right == TRUE || left == right {
                    return left;
                }
            }
            Operation::Or => {
                if left == TRUE || right == TRUE {
                    return TRUE;
                }
                if left == FALSE {
                    return right;
                }
                if right == FALSE || left == right {
                    return left;
                }
            }
        }
        let key = (operation, left.min(right), left.max(right));
        if let Some(&result) = self.operations.get(&key) {
            return result;
        }
        let (lv, rv) = (self.variable(left), self.variable(right));
        let variable = lv.min(rv);
        let (left_low, left_high) = if lv == variable {
            let node = self.nodes[left as usize];
            (node.low, node.high)
        } else {
            (left, left)
        };
        let (right_low, right_high) = if rv == variable {
            let node = self.nodes[right as usize];
            (node.low, node.high)
        } else {
            (right, right)
        };
        let low = self.apply(operation, left_low, right_low);
        let high = self.apply(operation, left_high, right_high);
        let result = self.make(variable, low, high);
        self.operations.insert(key, result);
        result
    }

    fn negate(&mut self, node: u32) -> u32 {
        match node {
            FALSE => return TRUE,
            TRUE => return FALSE,
            _ => {}
        }
        if let Some(&result) = self.negations.get(&node) {
            return result;
        }
        let Node {
            variable,
            low,
            high,
        } = self.nodes[node as usize];
        let low = self.negate(low);
        let high = self.negate(high);
        let result = self.make(variable, low, high);
        self.negations.insert(node, result);
        result
    }

    /// Remove every variable of `branch` by disjunction over its digits.
    fn exists(&mut self, node: u32, first: u32, bits: u32, cache: &mut HashMap<u32, u32>) -> u32 {
        if node <= TRUE {
            return node;
        }
        if let Some(&result) = cache.get(&node) {
            return result;
        }
        let Node {
            variable,
            low,
            high,
        } = self.nodes[node as usize];
        let low = self.exists(low, first, bits, cache);
        let high = self.exists(high, first, bits, cache);
        let result = if first <= variable && variable < first + bits {
            self.apply(Operation::Or, low, high)
        } else {
            self.make(variable, low, high)
        };
        cache.insert(node, result);
        result
    }

    fn support(&mut self, node: u32) -> std::rc::Rc<[BranchId]> {
        if let Some(support) = self.supports.get(&node) {
            return support.clone();
        }
        let mut stack = vec![node];
        let mut seen = crate::HashSet::default();
        let mut branches = Vec::new();
        while let Some(current) = stack.pop() {
            if current <= TRUE || !seen.insert(current) {
                continue;
            }
            let Node {
                variable,
                low,
                high,
            } = self.nodes[current as usize];
            branches.push(self.variables[variable as usize].0);
            stack.push(low);
            stack.push(high);
        }
        branches.sort_unstable();
        branches.dedup();
        let support: std::rc::Rc<[BranchId]> = branches.into();
        self.supports.insert(node, support.clone());
        support
    }

    fn size(&mut self, node: u32) -> usize {
        if let Some(&size) = self.sizes.get(&node) {
            return size;
        }
        let mut stack = vec![node];
        let mut seen = crate::HashSet::default();
        while let Some(current) = stack.pop() {
            if current <= TRUE || !seen.insert(current) {
                continue;
            }
            let Node { low, high, .. } = self.nodes[current as usize];
            stack.push(low);
            stack.push(high);
        }
        let size = seen.len();
        self.sizes.insert(node, size);
        size
    }

    /// The function restricted to valid codes of every branch it reads.
    fn restricted_to_valid(&mut self, node: u32, others: &[BranchId]) -> u32 {
        let support = self.support(node);
        let mut result = node;
        for &branch in support.iter().chain(others) {
            let valid = self.validity(branch);
            result = self.apply(Operation::And, result, valid);
        }
        result
    }

    fn satisfiable(&mut self, node: u32) -> bool {
        self.restricted_to_valid(node, &[]) != FALSE
    }

    fn rename(
        &mut self,
        node: u32,
        branches: &HashMap<BranchId, BranchId>,
        cache: &mut HashMap<u32, u32>,
    ) -> u32 {
        if node <= TRUE {
            return node;
        }
        if let Some(&result) = cache.get(&node) {
            return result;
        }
        let Node {
            variable,
            low,
            high,
        } = self.nodes[node as usize];
        let low = self.rename(low, branches, cache);
        let high = self.rename(high, branches, cache);
        let (branch, digit) = self.variables[variable as usize];
        let target = branches.get(&branch).copied().unwrap_or(branch);
        debug_assert!(
            target != BranchId::ERASED,
            "erased branches are eliminated first"
        );
        let (first, _) = self.branch(target);
        let literal = self.make(first + digit, FALSE, TRUE);
        let negative = self.negate(literal);
        let when_one = self.apply(Operation::And, literal, high);
        let when_zero = self.apply(Operation::And, negative, low);
        let result = self.apply(Operation::Or, when_one, when_zero);
        cache.insert(node, result);
        result
    }
}

impl PathCondition {
    pub(in crate::comb_loop_detect) fn branch_count(&self) -> usize {
        with(|manager| manager.support(self.0).len())
    }

    /// Nodes of the diagram, a bound on the work of every operation on it.
    pub(in crate::comb_loop_detect) fn work_size(&self) -> usize {
        with(|manager| manager.size(self.0))
    }

    pub(in crate::comb_loop_detect) fn is_unconditional(&self) -> bool {
        self.0 == TRUE
    }

    pub(in crate::comb_loop_detect) fn with_choice(&self, branch: BranchId, arm: usize) -> Self {
        self.with_choice_range(branch, arm, arm.saturating_add(1))
    }

    /// Replace any constraint on `branch` by its arms `[start, end)`.
    pub(in crate::comb_loop_detect) fn with_choice_range(
        &self,
        branch: BranchId,
        start: usize,
        end: usize,
    ) -> Self {
        debug_assert!(start < end && end <= branch.arms());
        with(|manager| {
            let (first, bits) = manager.branch(branch);
            let valid = manager.validity(branch);
            let restricted = manager.apply(Operation::And, self.0, valid);
            let free = manager.exists(restricted, first, bits, &mut HashMap::default());
            let arms = manager.arms(branch, start, end);
            Self(manager.apply(Operation::And, free, arms))
        })
    }

    pub(in crate::comb_loop_detect) fn try_disjoin_all<'a>(
        conditions: impl IntoIterator<Item = &'a Self>,
        work: &mut usize,
    ) -> Option<Self> {
        let mut conditions = conditions.into_iter();
        let Some(first) = conditions.next() else {
            return Some(Self::default());
        };
        let mut combined = *first;
        for condition in conditions {
            super::ssa::reserve_guard_work(work, [&combined, condition])?;
            combined = combined.disjoin(condition);
        }
        Some(combined)
    }

    pub(in crate::comb_loop_detect) fn conjoin_if_compatible(&self, other: &Self) -> Option<Self> {
        with(|manager| {
            let conjunction = manager.apply(Operation::And, self.0, other.0);
            manager
                .satisfiable(conjunction)
                .then_some(Self(conjunction))
        })
    }

    /// Every valuation of `other` is a valuation of `self`.
    pub(in crate::comb_loop_detect) fn covers(&self, other: &Self) -> bool {
        if self.0 == TRUE || self.0 == other.0 {
            return true;
        }
        with(|manager| {
            let negated = manager.negate(self.0);
            let difference = manager.apply(Operation::And, other.0, negated);
            let others = manager.support(self.0);
            manager.restricted_to_valid(difference, &others) == FALSE
        })
    }

    pub(in crate::comb_loop_detect) fn branches(&self) -> impl Iterator<Item = BranchId> {
        with(|manager| manager.support(self.0)).to_vec().into_iter()
    }

    pub(in crate::comb_loop_detect) fn remapped(
        &self,
        branches: &HashMap<BranchId, BranchId>,
    ) -> Self {
        if self.0 <= TRUE || branches.is_empty() {
            return *self;
        }
        with(|manager| {
            // An erased branch is eliminated existentially over its valid
            // arms; its invalid codes, true by convention, admit nothing.
            let mut node = self.0;
            for (branch, target) in branches {
                if *target != BranchId::ERASED || !manager.branches.contains_key(branch) {
                    continue;
                }
                let (first, bits) = manager.branch(*branch);
                let valid = manager.validity(*branch);
                let restricted = manager.apply(Operation::And, node, valid);
                node = manager.exists(restricted, first, bits, &mut HashMap::default());
            }
            Self(manager.rename(node, branches, &mut HashMap::default()))
        })
    }

    pub(in crate::comb_loop_detect) fn disjoin(&self, other: &Self) -> Self {
        with(|manager| Self(manager.apply(Operation::Or, self.0, other.0)))
    }

    /// Disjunction is exact, so it always succeeds.
    pub(in crate::comb_loop_detect) fn disjoin_exact(&self, other: &Self) -> Option<Self> {
        Some(self.disjoin(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branches() -> Vec<BranchId> {
        (0..3)
            .map(|local| BranchId::new(999, local, [2, 3, 5][local]))
            .collect()
    }

    fn valuations(branches: &[BranchId]) -> Vec<Vec<usize>> {
        let mut result = vec![Vec::new()];
        for branch in branches {
            result = result
                .into_iter()
                .flat_map(|prefix| {
                    (0..branch.arms()).map(move |arm| {
                        let mut next = prefix.clone();
                        next.push(arm);
                        next
                    })
                })
                .collect();
        }
        result
    }

    /// The valuations satisfying a condition, by building the same condition
    /// for a single valuation and testing implication.
    fn models(condition: PathCondition, branches: &[BranchId]) -> Vec<Vec<usize>> {
        valuations(branches)
            .into_iter()
            .filter(|valuation| {
                let point = branches
                    .iter()
                    .zip(valuation)
                    .fold(PathCondition::default(), |condition, (branch, arm)| {
                        condition.with_choice(*branch, *arm)
                    });
                point.conjoin_if_compatible(&condition).is_some()
            })
            .collect()
    }

    #[test]
    fn operations_match_valuation_sets() {
        reset();
        let branches = branches();
        let mut conditions = vec![PathCondition::default()];
        for &branch in &branches {
            for start in 0..branch.arms() {
                for end in start + 1..=branch.arms() {
                    conditions.push(PathCondition::default().with_choice_range(branch, start, end));
                }
            }
        }
        let mut seed = 5u64;
        let mut pick = |len: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as usize % len
        };
        for _ in 0..200 {
            let a = conditions[pick(conditions.len())];
            let b = conditions[pick(conditions.len())];
            let combined = if pick(2) == 0 {
                a.conjoin_if_compatible(&b).unwrap_or(a)
            } else {
                a.disjoin(&b)
            };
            conditions.push(combined);
        }
        let all = valuations(&branches);
        for &a in conditions.iter().take(60) {
            let left = models(a, &branches);
            assert_eq!(a.is_unconditional(), left.len() == all.len());
            for &b in conditions.iter().take(60) {
                let right = models(b, &branches);
                let union = models(a.disjoin(&b), &branches);
                let expected_union = all
                    .iter()
                    .filter(|v| left.contains(v) || right.contains(v))
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(union, expected_union);
                let intersection = left.iter().filter(|v| right.contains(v)).count();
                assert_eq!(a.conjoin_if_compatible(&b).is_some(), intersection > 0);
                assert_eq!(a.covers(&b), right.iter().all(|v| left.contains(v)));
                assert_eq!(a == b, left == right, "conditions are canonical");
            }
        }
    }

    #[test]
    fn choosing_an_arm_replaces_the_previous_choice() {
        reset();
        let branch = branches()[1];
        let first = PathCondition::default().with_choice(branch, 0);
        let second = first.with_choice(branch, 2);
        assert_eq!(second, PathCondition::default().with_choice(branch, 2));
        let all = PathCondition::default()
            .with_choice(branch, 0)
            .disjoin(&PathCondition::default().with_choice_range(branch, 1, 3));
        assert!(all.is_unconditional());
    }

    #[test]
    fn erasing_a_branch_removes_its_constraints() {
        reset();
        let [a, b, _] = branches()[..] else {
            unreachable!()
        };
        let condition = PathCondition::default().with_choice(a, 1).with_choice(b, 2);
        let erased = condition.remapped(&HashMap::from_iter([(a, BranchId::ERASED)]));
        assert_eq!(erased, PathCondition::default().with_choice(b, 2));
    }

    #[test]
    fn erasing_a_branch_keeps_the_constraints_of_others() {
        reset();
        let [_, b, c] = branches()[..] else {
            unreachable!()
        };
        // `b` has 3 arms, so code 3 is invalid and true by convention. It
        // must not admit valuations of `c` that no arm of `b` admits.
        let condition = PathCondition::default().with_choice(b, 0).with_choice(c, 4);
        let erased = condition.remapped(&HashMap::from_iter([(b, BranchId::ERASED)]));
        assert_eq!(erased, PathCondition::default().with_choice(c, 4));
        let either = PathCondition::default()
            .with_choice(b, 0)
            .with_choice(c, 1)
            .disjoin(&PathCondition::default().with_choice(b, 2).with_choice(c, 3));
        let erased = either.remapped(&HashMap::from_iter([(b, BranchId::ERASED)]));
        assert_eq!(
            erased,
            PathCondition::default()
                .with_choice_range(c, 1, 2)
                .disjoin(&PathCondition::default().with_choice(c, 3))
        );
    }

    #[test]
    fn remapping_moves_constraints_to_other_branches() {
        reset();
        let [a, b, _] = branches()[..] else {
            unreachable!()
        };
        let target = BranchId::new(1000, 0, a.arms());
        let condition = PathCondition::default().with_choice(a, 1).with_choice(b, 2);
        let remapped = condition.remapped(&HashMap::from_iter([(a, target)]));
        assert_eq!(
            remapped,
            PathCondition::default()
                .with_choice(target, 1)
                .with_choice(b, 2)
        );
    }
}
