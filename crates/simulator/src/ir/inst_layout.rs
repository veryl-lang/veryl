//! Per-top-level-Inst FF byte range metadata.
//!
//! For each top-level child of the top ProtoModule we record the minimum/
//! maximum FF byte offset reachable in that subtree.  Metadata only — no
//! runtime path is changed.  Future work may consume the ranges to apply
//! cache-line aligned padding between Inst FF blocks or to drive per-Inst
//! independent commit dispatch.

use super::variable::ModuleVariableMeta;
use veryl_parser::resource_table::StrId;

#[derive(Debug, Clone)]
pub struct InstFfRange {
    pub name: StrId,
    /// Inclusive start byte offset within Ir.ff_values.
    pub ff_start: u32,
    /// Exclusive end byte offset within Ir.ff_values.
    pub ff_end: u32,
}

#[derive(Debug, Default, Clone)]
pub struct InstLayout {
    pub ranges: Vec<InstFfRange>,
}

impl InstLayout {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the layout from the top ProtoModule's variable meta tree.
    /// One `InstFfRange` is produced per top-level child whose subtree
    /// contains at least one FF VariableElement.
    pub fn build_from_top(top: &ModuleVariableMeta) -> Self {
        let mut ranges = Vec::with_capacity(top.children.len());
        for child in &top.children {
            if let Some((s, e)) = compute_ff_extent(child) {
                ranges.push(InstFfRange {
                    name: child.name,
                    ff_start: s,
                    ff_end: e,
                });
            }
        }
        Self { ranges }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// True iff no two ranges overlap.  Always expected to hold for
    /// well-formed designs; race-free per-Inst commit relies on it.
    pub fn ranges_disjoint(&self) -> bool {
        let mut sorted: Vec<&InstFfRange> = self.ranges.iter().collect();
        sorted.sort_by_key(|r| r.ff_start);
        for w in sorted.windows(2) {
            if w[0].ff_end > w[1].ff_start {
                return false;
            }
        }
        true
    }
}

fn compute_ff_extent(meta: &ModuleVariableMeta) -> Option<(u32, u32)> {
    let mut min: Option<u32> = None;
    let mut max: Option<u32> = None;
    accumulate(meta, &mut min, &mut max);
    match (min, max) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    }
}

fn accumulate(meta: &ModuleVariableMeta, min: &mut Option<u32>, max: &mut Option<u32>) {
    for vm in meta.variable_meta.values() {
        for el in &vm.elements {
            if el.is_ff() {
                let start = el.current_offset() as u32;
                let end = start + el.native_bytes as u32;
                *min = Some(min.map(|m| m.min(start)).unwrap_or(start));
                *max = Some(max.map(|m| m.max(end)).unwrap_or(end));
            }
        }
    }
    for child in &meta.children {
        accumulate(child, min, max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_layout() {
        let l = InstLayout::new();
        assert!(l.is_empty());
        assert_eq!(l.len(), 0);
        assert!(l.ranges_disjoint());
    }

    #[test]
    fn disjoint_passes() {
        let l = InstLayout {
            ranges: vec![
                InstFfRange {
                    name: StrId::default(),
                    ff_start: 0,
                    ff_end: 100,
                },
                InstFfRange {
                    name: StrId::default(),
                    ff_start: 100,
                    ff_end: 200,
                },
            ],
        };
        assert!(l.ranges_disjoint());
    }

    #[test]
    fn overlapping_fails() {
        let l = InstLayout {
            ranges: vec![
                InstFfRange {
                    name: StrId::default(),
                    ff_start: 0,
                    ff_end: 100,
                },
                InstFfRange {
                    name: StrId::default(),
                    ff_start: 50,
                    ff_end: 150,
                },
            ],
        };
        assert!(!l.ranges_disjoint());
    }
}
