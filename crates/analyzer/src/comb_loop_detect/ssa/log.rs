//! Indexed write history of one storage key.
//!
//! A key's SSA value is an `Overlay` chain, which is exact but must be walked
//! from the newest write. A read of a small region would then visit every
//! earlier write. This log records the same writes as a persistent stack with
//! skip pointers whose bounding boxes let a regional read pass over writes
//! that cannot reach it, so it visits only the writes that overlap it.

use super::{PositionDomain, VersionId, complement};
use std::rc::Rc;

pub(super) enum Layer {
    Write {
        region: PositionDomain,
        version: VersionId,
        strong: bool,
    },
    /// The join of branches that all started from `common`. A position keeps
    /// its value from `common` unless every branch overwrote it.
    Merge {
        common: Rc<LogNode>,
        branches: Vec<Rc<LogNode>>,
        bounds: Option<PositionDomain>,
    },
}

impl Layer {
    fn bounds(&self) -> Option<PositionDomain> {
        match self {
            Self::Write { region, .. } => Some(*region),
            Self::Merge { bounds, .. } => *bounds,
        }
    }
}

pub(super) struct LogNode {
    prev: Option<Rc<LogNode>>,
    // An ancestor reached by skipping the layers in `bounds`.
    jump: Option<Rc<LogNode>>,
    layer: Option<Layer>,
    base: VersionId,
    len: usize,
    bounds: Option<PositionDomain>,
}

impl Drop for LogNode {
    // Long write sequences would otherwise drop recursively.
    fn drop(&mut self) {
        let mut stack: Vec<Rc<LogNode>> = self
            .prev
            .take()
            .into_iter()
            .chain(self.jump.take())
            .collect();
        let mut layer = self.layer.take();
        loop {
            if let Some(Layer::Merge {
                common, branches, ..
            }) = layer.take()
            {
                stack.push(common);
                stack.extend(branches);
            }
            let Some(node) = stack.pop() else {
                break;
            };
            if let Ok(mut node) = Rc::try_unwrap(node) {
                stack.extend(node.prev.take());
                stack.extend(node.jump.take());
                layer = node.layer.take();
            }
        }
    }
}

/// A position range of a read and the version that supplies it.
pub(super) struct Piece {
    pub(super) version: VersionId,
    pub(super) domain: PositionDomain,
}

impl LogNode {
    pub(super) fn root(base: VersionId) -> Rc<Self> {
        Rc::new(Self {
            prev: None,
            jump: None,
            layer: None,
            base,
            len: 0,
            bounds: None,
        })
    }

    pub(super) fn base(&self) -> VersionId {
        self.base
    }

    pub(super) fn push(self: &Rc<Self>, layer: Layer) -> Rc<Self> {
        let own = layer.bounds();
        // Skew-binary jumps keep every node within a logarithmic number of
        // jumps from any ancestor.
        let (jump, bounds) = match self.jump.as_ref() {
            Some(first)
                if first
                    .jump
                    .as_ref()
                    .is_some_and(|second| self.len - first.len == first.len - second.len) =>
            {
                let bounds = hull(hull(own, self.bounds), first.bounds);
                (first.jump.clone(), bounds)
            }
            _ => (Some(Rc::clone(self)), own),
        };
        Rc::new(Self {
            prev: Some(Rc::clone(self)),
            jump,
            layer: Some(layer),
            base: self.base,
            len: self.len + 1,
            bounds,
        })
    }

    /// Join branch logs that all extend `common`.
    pub(super) fn merge(common: &Rc<Self>, branches: Vec<Rc<Self>>) -> Option<Rc<Self>> {
        let mut bounds = None;
        for branch in &branches {
            let mut node = branch;
            while node.len > common.len {
                bounds = hull(bounds, node.layer.as_ref().and_then(Layer::bounds));
                node = node.prev.as_ref()?;
            }
            // Logs started at the same unlogged version share their bottom.
            if !Rc::ptr_eq(node, common) && !(common.len == 0 && node.base == common.base) {
                return None;
            }
        }
        Some(common.push(Layer::Merge {
            common: Rc::clone(common),
            branches,
            bounds,
        }))
    }

    /// The writes that supply `region`, including the version below the log.
    pub(super) fn resolve(
        self: &Rc<Self>,
        region: PositionDomain,
        work: &mut usize,
    ) -> Option<Vec<Piece>> {
        let mut pieces = Vec::new();
        let mut remaining = Fragments::default();
        remaining.insert(region);
        let remaining = walk(self, 0, remaining, &mut pieces, work)?;
        pieces.extend(remaining.into_domains().map(|domain| Piece {
            version: self.base,
            domain,
        }));
        Some(pieces)
    }
}

/// Disjoint position boxes ordered by their array start, then their packed
/// start. A query visits only boxes whose array start lies within the longest
/// array length of the query. Boxes that share an array start overlap on the
/// array axis, so they are disjoint on the packed one: of those starting
/// below the query, only the last can meet it. Scattered writes into many
/// small fragments, of elements or of bits, do not rescan all of them.
#[derive(Clone, Default)]
pub(super) struct Fragments {
    boxes: std::collections::BTreeSet<(usize, usize, usize, usize)>,
    // Array lengths of the boxes with their multiplicities.
    lengths: std::collections::BTreeMap<usize, usize>,
}

impl Fragments {
    fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    pub(super) fn insert(&mut self, domain: PositionDomain) {
        if self.boxes.insert((
            domain.array_start,
            domain.packed_start,
            domain.array_length,
            domain.packed_length,
        )) {
            *self.lengths.entry(domain.array_length).or_default() += 1;
        }
    }

    fn remove(&mut self, domain: PositionDomain) {
        if self.boxes.remove(&(
            domain.array_start,
            domain.packed_start,
            domain.array_length,
            domain.packed_length,
        )) && let Some(count) = self.lengths.get_mut(&domain.array_length)
        {
            *count -= 1;
            if *count == 0 {
                self.lengths.remove(&domain.array_length);
            }
        }
    }

    fn longest(&self) -> usize {
        self.lengths.keys().next_back().copied().unwrap_or(0)
    }

    /// Visit the boxes whose starts lie close enough to meet `region` until
    /// `visit` returns true, charging each visited box and each array start
    /// passed. Whether some visit returned true.
    fn visit_candidates(
        &self,
        region: PositionDomain,
        work: &mut usize,
        mut visit: impl FnMut(PositionDomain) -> bool,
    ) -> Option<bool> {
        let high = region.array_start.saturating_add(region.array_length);
        let packed_low = region.packed_start;
        let packed_high = region.packed_start.saturating_add(region.packed_length);
        let mut array = region
            .array_start
            .saturating_sub(self.longest().saturating_sub(1));
        while array < high {
            *work = work.checked_sub(1)?;
            // The first box at or after this array start.
            let Some(&(next, ..)) = self.boxes.range((array, 0, 0, 0)..).next() else {
                break;
            };
            if next >= high {
                break;
            }
            let below = self
                .boxes
                .range((next, 0, 0, 0)..(next, packed_low, 0, 0))
                .next_back();
            let within = self
                .boxes
                .range((next, packed_low, 0, 0)..(next, packed_high, 0, 0));
            for &(array_start, packed_start, array_length, packed_length) in
                below.into_iter().chain(within)
            {
                *work = work.checked_sub(1)?;
                if visit(PositionDomain {
                    array_start,
                    array_length,
                    packed_start,
                    packed_length,
                }) {
                    return Some(true);
                }
            }
            let Some(after) = next.checked_add(1) else {
                break;
            };
            array = after;
        }
        Some(false)
    }

    /// The boxes overlapping `region`, charging each visited candidate.
    pub(super) fn overlapping(
        &self,
        region: PositionDomain,
        work: &mut usize,
    ) -> Option<Vec<PositionDomain>> {
        let mut found = Vec::new();
        self.visit_candidates(region, work, |fragment| {
            if intersection(fragment, region).is_some() {
                found.push(fragment);
            }
            false
        })?;
        Some(found)
    }

    /// Whether any box overlaps `region`. Stops at the first one: a jump's
    /// bounds usually meet many fragments, and collecting them all would make
    /// every skip test linear in the fragments.
    fn overlaps(&self, region: Option<PositionDomain>, work: &mut usize) -> Option<bool> {
        let Some(region) = region else {
            return Some(false);
        };
        self.visit_candidates(region, work, |fragment| {
            intersection(fragment, region).is_some()
        })
    }

    /// Remove the positions of `region`.
    fn subtract(&mut self, region: PositionDomain, work: &mut usize) -> Option<()> {
        for fragment in self.overlapping(region, work)? {
            self.remove(fragment);
            for part in complement(fragment, region) {
                *work = work.checked_sub(1)?;
                self.insert(part);
            }
        }
        Some(())
    }

    /// Add the positions of `domain` that are not present yet.
    fn add(&mut self, domain: PositionDomain, work: &mut usize) -> Option<()> {
        let mut parts = vec![domain];
        for existing in self.overlapping(domain, work)? {
            parts = parts
                .into_iter()
                .flat_map(|part| complement(part, existing))
                .collect();
        }
        for part in parts {
            *work = work.checked_sub(1)?;
            self.insert(part);
        }
        Some(())
    }

    /// Move the boxes overlapping `region` into a separate set.
    fn split_off(&mut self, region: Option<PositionDomain>, work: &mut usize) -> Option<Self> {
        let mut inside = Self::default();
        if let Some(region) = region {
            for fragment in self.overlapping(region, work)? {
                self.remove(fragment);
                inside.insert(fragment);
            }
        }
        Some(inside)
    }

    fn into_domains(self) -> impl Iterator<Item = PositionDomain> {
        self.boxes
            .into_iter()
            .map(
                |(array_start, packed_start, array_length, packed_length)| PositionDomain {
                    array_start,
                    array_length,
                    packed_start,
                    packed_length,
                },
            )
    }
}

/// Collect the writes from `tip` down to the ancestor of length `stop` and
/// return the positions they do not definitely overwrite.
/// One history being walked: from `node` down to the layer at `stop`,
/// supplying `remaining`, and the join it is inside of, if any.
struct WalkFrame<'a> {
    node: &'a Rc<LogNode>,
    stop: usize,
    remaining: Fragments,
    join: Option<JoinWalk<'a>>,
}

/// The branches of a join still to walk, from the positions inside it.
struct JoinWalk<'a> {
    branches: &'a [Rc<LogNode>],
    next: usize,
    stop: usize,
    inside: Fragments,
}

/// Walk the history at `tip` down to `stop`, recording the writes that supply
/// positions of `remaining` and returning the positions none supplies. The
/// branches of joins are walked from an explicit stack, so nested joins need
/// no recursion.
fn walk(
    tip: &Rc<LogNode>,
    stop: usize,
    remaining: Fragments,
    pieces: &mut Vec<Piece>,
    work: &mut usize,
) -> Option<Fragments> {
    let mut frames = vec![WalkFrame {
        node: tip,
        stop,
        remaining,
        join: None,
    }];
    let mut returned: Option<Fragments> = None;
    loop {
        let frame = frames.last_mut().expect("a frame is walked until done");
        if let Some(join) = &mut frame.join {
            // Positions a branch left unwritten keep the value below the join.
            if let Some(left) = returned.take() {
                for fragment in left.into_domains() {
                    frame.remaining.add(fragment, work)?;
                }
            }
            if let Some(branch) = join.branches.get(join.next) {
                join.next += 1;
                let child = WalkFrame {
                    node: branch,
                    stop: join.stop,
                    remaining: join.inside.clone(),
                    join: None,
                };
                frames.push(child);
                continue;
            }
            frame.join = None;
            frame.node = frame
                .node
                .prev
                .as_ref()
                .expect("only the root has no layer");
        }
        let mut joined = false;
        while !frame.remaining.is_empty() && frame.node.len > frame.stop {
            *work = work.checked_sub(1)?;
            let node = frame.node;
            if let Some(jump) = &node.jump
                && jump.len >= frame.stop
                && !frame.remaining.overlaps(node.bounds, work)?
            {
                frame.node = jump;
                continue;
            }
            match node.layer.as_ref().expect("only the root has no layer") {
                Layer::Write {
                    region,
                    version,
                    strong,
                } => {
                    for fragment in frame.remaining.overlapping(*region, work)? {
                        if let Some(domain) = intersection(fragment, *region) {
                            pieces.push(Piece {
                                version: *version,
                                domain,
                            });
                        }
                    }
                    if *strong {
                        frame.remaining.subtract(*region, work)?;
                    }
                }
                Layer::Merge {
                    common,
                    branches,
                    bounds,
                } => {
                    let inside = frame.remaining.split_off(*bounds, work)?;
                    if !inside.is_empty() {
                        frame.join = Some(JoinWalk {
                            branches,
                            next: 0,
                            stop: common.len,
                            inside,
                        });
                        joined = true;
                        break;
                    }
                }
            }
            frame.node = node.prev.as_ref().expect("only the root has no layer");
        }
        if joined {
            continue;
        }
        let done = frames.pop().expect("the walked frame is on the stack");
        if frames.is_empty() {
            return Some(done.remaining);
        }
        returned = Some(done.remaining);
    }
}

pub(super) fn intersection(a: PositionDomain, b: PositionDomain) -> Option<PositionDomain> {
    let array_start = a.array_start.max(b.array_start);
    let array_end = a
        .array_start
        .saturating_add(a.array_length)
        .min(b.array_start.saturating_add(b.array_length));
    let packed_start = a.packed_start.max(b.packed_start);
    let packed_end = a
        .packed_start
        .saturating_add(a.packed_length)
        .min(b.packed_start.saturating_add(b.packed_length));
    (array_start < array_end && packed_start < packed_end).then(|| PositionDomain {
        array_start,
        array_length: array_end - array_start,
        packed_start,
        packed_length: packed_end - packed_start,
    })
}

fn hull(a: Option<PositionDomain>, b: Option<PositionDomain>) -> Option<PositionDomain> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.hull(b)),
        (a, None) => a,
        (None, b) => b,
    }
}
