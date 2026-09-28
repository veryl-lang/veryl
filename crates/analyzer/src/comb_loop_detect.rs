//! Combinational cycle checking, using the shared IR analysis pipeline.

pub(crate) mod diagnostics;
pub(crate) mod graph;
pub(crate) mod hierarchy;
pub(crate) mod model;
pub(crate) mod summary;
pub(crate) use crate::procedural::{region, ssa};

pub fn check(ir: &crate::ir::Ir) -> Vec<crate::AnalyzerError> {
    check_with_status(ir).0
}

fn check_with_status(ir: &crate::ir::Ir) -> (Vec<crate::AnalyzerError>, bool) {
    let result = crate::analysis::analyze(ir);
    (result.loops, result.complete)
}

#[cfg(test)]
pub(crate) fn is_complete(ir: &crate::ir::Ir) -> bool {
    check_with_status(ir).1
}

#[cfg(test)]
pub(crate) use crate::procedural::{
    function_barrier_evaluation_count, function_evaluation_count,
    function_result_region_probe_count, function_result_version_count,
    function_summary_graph_edge_count, function_summary_graph_node_count, module_context_entries,
    reset_function_evaluation_count, reset_module_context_entries,
    reset_traced_procedure_evaluation_count, reset_visible_source_probes,
    traced_procedure_evaluation_count, visible_source_probes, write_footprint_statement_visits,
};
#[cfg(test)]
pub(crate) use crate::procedural::{with_procedure_guard_limit, with_procedure_import_limit};
#[cfg(test)]
pub(crate) use diagnostics::{
    diagnostic_instance_probe_count, diagnostic_provenance_build_count, diagnostic_replay_count,
    reset_diagnostic_instance_probe_count, reset_diagnostic_provenance_build_count,
    reset_diagnostic_replay_count,
};
#[cfg(test)]
pub(crate) use graph::{
    cycle_decision_work, cycle_search_work, reset_cycle_decision_work, reset_cycle_search_work,
};
#[cfg(test)]
pub(crate) use ssa::{
    import_binding_visits, reset_import_binding_visits, reset_source_walk_visits,
    source_walk_visits,
};
#[cfg(test)]
pub(crate) use summary::{
    module_summary_work, reset_module_summary_work, with_module_summary_limit,
};

#[cfg(test)]
pub(crate) use crate::analysis::{
    analysis_size, reset_analysis_size, with_partition_extra_atom_limit,
};
