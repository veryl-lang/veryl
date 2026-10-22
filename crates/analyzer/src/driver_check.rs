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
use crate::comb_loop_detect::hierarchy::module_postorder;
use crate::conv::checker::portability::allow_multiple_assign;
use crate::ir::span::{ArraySpan, PackedSpan};
use crate::ir::{Ir, Module, VarId};
use crate::{HashMap, HashSet};
use veryl_parser::token_range::TokenRange;

pub fn check(ir: &Ir) -> Vec<AnalyzerError> {
    let mut reports = Reports::default();
    for module in module_postorder(ir) {
        check_module(module, &mut reports);
    }
    reports.into_errors()
}

/// Conflicts at the same source location found in several specializations
/// are one diagnostic.
#[derive(Default)]
struct Reports {
    order: Vec<(String, TokenRange)>,
    sites: HashMap<(String, TokenRange), Vec<TokenRange>>,
}

impl Reports {
    fn insert(&mut self, identifier: String, sites: Vec<TokenRange>) {
        let key = (identifier, sites[0]);
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
                AnalyzerError::multiple_assignment(&key.0, &key.1, &sites)
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
        for write in writes.writes.iter().filter(|x| x.driving) {
            driven
                .entry(write.id)
                .or_default()
                .entry((process, write.array, write.packed))
                .or_default()
                .push(write.token);
        }
    }

    let mut context = crate::conv::Context::default();
    context.variables = module.variables.clone();
    context.variables.extend(module.interface_members.clone());

    let mut found = Vec::new();
    for (id, regions) in driven {
        if allow_multiple_assign(&context, id) {
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
        if let Some(variable) = context.variables.get(&id)
            && !sites.is_empty()
        {
            found.push((variable.path.to_string(), sites));
        }
    }
    found.sort_by(|a, b| a.1[0].cmp(&b.1[0]));
    for (identifier, sites) in found {
        reports.insert(identifier, sites);
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
