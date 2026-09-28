//! Definite-assignment queries over lowered procedural SSA.
//!
//! Assignment effects are independent of data dependencies: an explicit
//! self-read is still a write, while a skipped write retains its entry effect.
//! Regions are sparse partition atoms, never individual declared array items.

use super::ssa::SsaStore;
use super::{SsaAspect, SsaKey, SsaOutput, UncoveredAssignment};

pub(super) fn uncovered(
    ssa: &mut SsaStore<SsaKey>,
    outputs: &[SsaOutput],
    work: &mut usize,
) -> Option<Vec<UncoveredAssignment>> {
    let mut uncovered = Vec::new();
    for output in outputs {
        // An effect query observes entry effects even without a value read.
        let root = ssa.definition(vec![output.assignment]);
        let sources = ssa.try_root_source_keys_guarded(root, work)?;
        if sources
            .iter()
            .any(|(source, _)| source.aspect == SsaAspect::Assignment)
        {
            uncovered.push(UncoveredAssignment {
                id: output.key.0,
                sites: output.sites.clone(),
            });
        }
    }
    Some(uncovered)
}
