//! `multiple_assignment` on concrete module specializations.
//!
//! A region driven by two processes of a module's [`WriteSummary`] is a
//! conflict. The check walks the same specializations as the
//! combinational-loop detector, so generate blocks are resolved by the actual
//! generic arguments; unevaluable generic templates are only checked through
//! their instantiations.
//!
//! [`WriteSummary`]: crate::ir::WriteSummary

use crate::analyzer_error::AnalyzerError;
use crate::ir::span::{ArraySpan, PackedSpan};
use crate::ir::{Ir, Module, VarId};
use crate::{HashMap, HashSet};
use veryl_parser::token_range::TokenRange;

pub fn check(ir: &Ir) -> Vec<AnalyzerError> {
    let mut reports = Reports::default();
    for module in ir.module_postorder() {
        check_module(module, &mut reports);
    }
    reports.into_errors()
}

/// Conflicts on the same declared variable found in several specializations
/// are one diagnostic.
#[derive(Default)]
struct Reports {
    /// `(declaration, identifier)`
    order: Vec<(TokenRange, String)>,
    sites: HashMap<(TokenRange, String), Vec<TokenRange>>,
}

impl Reports {
    fn insert(&mut self, declaration: TokenRange, identifier: String, sites: Vec<TokenRange>) {
        let key = (declaration, identifier);
        if let Some(existing) = self.sites.get_mut(&key) {
            existing.extend(sites);
            existing.sort();
            existing.dedup();
        } else {
            self.order.push(key.clone());
            self.sites.insert(key, sites);
        }
    }

    fn into_errors(mut self) -> Vec<AnalyzerError> {
        self.order
            .into_iter()
            .map(|key| {
                let sites = self.sites.remove(&key).unwrap();
                AnalyzerError::multiple_assignment(&key.1, &sites[0], &sites)
            })
            .collect()
    }
}

/// `(process, array, packed)`
type ProcessRegion = (usize, ArraySpan, PackedSpan);

struct Write {
    process: usize,
    array: ArraySpan,
    packed: PackedSpan,
    tokens: Vec<TokenRange>,
}

fn check_module(module: &Module, reports: &mut Reports) {
    // Identical regions driven by one process share an entry.
    let mut driven: HashMap<VarId, HashMap<ProcessRegion, Vec<TokenRange>>> = HashMap::default();
    for (process, writes) in module.write_summary.processes.iter().enumerate() {
        // Iteration writes lie within a claimed region of the same process.
        for write in writes
            .writes
            .iter()
            .filter(|x| x.driving && x.extent.is_claimed())
        {
            driven
                .entry(write.id)
                .or_default()
                .entry((process, write.array, write.packed))
                .or_default()
                .push(write.token);
        }
    }

    let mut found = Vec::new();
    for (id, regions) in driven {
        let Some(variable) = module
            .variables
            .get(&id)
            .or_else(|| module.interface_members.get(&id))
        else {
            continue;
        };
        if variable.allow_multiple_assign() {
            continue;
        }
        let mut writes: Vec<_> = regions
            .into_iter()
            .map(|((process, array, packed), tokens)| Write {
                process,
                array,
                packed,
                tokens,
            })
            .collect();
        let sites = conflicting_sites(&mut writes);
        if !sites.is_empty() {
            found.push((variable.token, variable.path.to_string(), sites));
        }
    }
    found.sort_by(|a, b| (a.2[0], &a.1).cmp(&(b.2[0], &b.1)));
    for (declaration, identifier, sites) in found {
        reports.insert(declaration, identifier, sites);
    }
}

/// Sorted tokens of every write overlapping a write of another process.
fn conflicting_sites(writes: &mut [Write]) -> Vec<TokenRange> {
    writes.sort_by_key(|x| (x.array.start, x.packed.start, x.process));

    let mut hit = HashSet::default();
    let mut active: Vec<usize> = Vec::new();
    for i in 0..writes.len() {
        let start = writes[i].array.start;
        active.retain(|&j| writes[j].array.end().is_none_or(|end| end > start));
        for &j in &active {
            if writes[j].process != writes[i].process && writes[j].packed.overlaps(writes[i].packed)
            {
                hit.insert(i);
                hit.insert(j);
            }
        }
        active.push(i);
    }

    let mut sites: Vec<_> = hit
        .into_iter()
        .flat_map(|i| writes[i].tokens.iter().copied())
        .collect();
    sites.sort();
    sites.dedup();
    sites
}
