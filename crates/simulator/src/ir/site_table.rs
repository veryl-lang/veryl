//! Per-Ir FF write site table.
//!
//! Compile-time metadata identifying every FF write site in the Ir.  Each site
//! is assigned a unique `site_id` (= index in `sites`) and stores the FF
//! current_offset, width, and write kind.  This metadata is the foundation for
//! future passes that want to reason about FF writes statically:
//!
//! - Write-log buffer sizing (= `sites.len()` upper bound)
//! - NBA invariant `debug_assert` (same site writing twice in a cycle)
//! - Per-site activity profiling (future)
//! - IR-level DCE on never-fired sites (future)
//!
//! The table participates in no runtime hot path; it is built at Ir
//! construction time and consulted by `write_log_capacity` (for
//! WriteLogBuffer sizing) and by env-gated diag checks.

use super::ProtoStatement;
use super::variable::native_bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    /// `ProtoAssignStatement::dst` with `VarOffset::Ff(..)`.
    Static,
    /// `ProtoAssignDynamicStatement::dst_base` with `VarOffset::Ff(..)`.
    Dynamic,
    /// FF write with bit width > 64.
    Wide,
}

#[derive(Debug, Clone)]
pub struct SiteInfo {
    /// FF current_offset (= absolute byte offset in the Ir's `ff_values`).
    pub current_offset: u32,
    /// FF bit width.
    pub width_bits: u32,
    /// Storage native bytes (1, 2, 4, 8, or 16).
    pub native_bytes: u8,
    pub kind: SiteKind,
}

#[derive(Debug, Default, Clone)]
pub struct SiteTable {
    pub sites: Vec<SiteInfo>,
}

impl SiteTable {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, info: SiteInfo) -> u32 {
        let id = self.sites.len() as u32;
        self.sites.push(info);
        id
    }

    pub fn add_static(&mut self, current_offset: u32, width_bits: u32, native_bytes: u8) -> u32 {
        self.push(SiteInfo {
            current_offset,
            width_bits,
            native_bytes,
            kind: SiteKind::Static,
        })
    }

    pub fn add_dynamic(
        &mut self,
        base_current_offset: u32,
        width_bits: u32,
        native_bytes: u8,
    ) -> u32 {
        self.push(SiteInfo {
            current_offset: base_current_offset,
            width_bits,
            native_bytes,
            kind: SiteKind::Dynamic,
        })
    }

    pub fn add_wide(&mut self, current_offset: u32, width_bits: u32, native_bytes: u8) -> u32 {
        self.push(SiteInfo {
            current_offset,
            width_bits,
            native_bytes,
            kind: SiteKind::Wide,
        })
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}

impl SiteTable {
    /// Walk pre-JIT `ProtoStatement`s and append a `SiteInfo` for every FF
    /// write site.  Recurses through If / For / SequentialBlock bodies to
    /// surface conditional / loop-internal sites.  Does not descend into
    /// `CompiledBlock` because the parent's pre-JIT list already contains
    /// the originals.
    pub fn extend_from_protos(&mut self, protos: &[ProtoStatement]) {
        for s in protos {
            visit(s, self);
        }
    }
}

fn visit(stmt: &ProtoStatement, table: &mut SiteTable) {
    match stmt {
        ProtoStatement::Assign(a) if a.dst.is_ff() => {
            let nb = native_bytes(a.dst_width);
            let width = a.dst_width as u32;
            let cur = a.dst_ff_current_offset as u32;
            if a.dst_width > 64 {
                table.add_wide(cur, width, nb as u8);
            } else {
                table.add_static(cur, width, nb as u8);
            }
        }
        ProtoStatement::AssignDynamic(a) if a.dst_base.is_ff() => {
            let nb = native_bytes(a.dst_width);
            let width = a.dst_width as u32;
            let base = a.dst_ff_current_base_offset as u32;
            table.add_dynamic(base, width, nb as u8);
        }
        ProtoStatement::If(i) => {
            for s in &i.true_side {
                visit(s, table);
            }
            for s in &i.false_side {
                visit(s, table);
            }
        }
        ProtoStatement::For(f) => {
            for s in &f.body {
                visit(s, table);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                visit(s, table);
            }
        }
        // CompiledBlock / SystemFunctionCall / TbMethodCall / Break / non-FF
        // Assign(Dynamic) have no FF writes for this metadata table.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_table() {
        let t = SiteTable::new();
        assert_eq!(t.len(), 0);
        assert!(t.is_empty());
    }

    #[test]
    fn add_static_assigns_increasing_ids() {
        let mut t = SiteTable::new();
        assert_eq!(t.add_static(0, 32, 4), 0);
        assert_eq!(t.add_static(8, 64, 8), 1);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn add_mixed_kinds() {
        let mut t = SiteTable::new();
        let s0 = t.add_static(0, 32, 4);
        let s1 = t.add_dynamic(8, 64, 8);
        let s2 = t.add_wide(16, 128, 16);
        assert_eq!(s0, 0);
        assert_eq!(s1, 1);
        assert_eq!(s2, 2);
        assert_eq!(t.sites[0].kind, SiteKind::Static);
        assert_eq!(t.sites[1].kind, SiteKind::Dynamic);
        assert_eq!(t.sites[2].kind, SiteKind::Wide);
    }

    #[test]
    fn extend_from_empty_proto_list() {
        let mut t = SiteTable::new();
        t.extend_from_protos(&[]);
        assert!(t.is_empty());
    }
}
