//! ProtoStatement / analyzer-IR optimization passes.
//!
//! All passes here are JIT-adjacent: on wasm they degrade to identity /
//! empty stubs so `ProtoModule::conv` and friends can call the public
//! API without inline `#[cfg]` branches at every site.

#[cfg(not(target_family = "wasm"))]
pub(crate) mod dead_var_dce;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod dup_assign_dce;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod load_cache_lookahead;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod multi_write_analysis;

#[cfg(target_family = "wasm")]
pub(crate) mod multi_write_analysis {
    use crate::HashSet;
    use veryl_analyzer::conv::Context as AnalyzerContext;
    use veryl_analyzer::ir as air;
    use veryl_analyzer::ir::VarId;
    pub fn analyze_multi_write(
        _decls: &[air::Declaration],
        _analyzer_ctx: &mut AnalyzerContext,
        _force_all_ff: bool,
    ) -> HashSet<(VarId, usize)> {
        HashSet::default()
    }
}

#[cfg(target_family = "wasm")]
pub(crate) mod dup_assign_dce {
    use crate::ir::ProtoStatement;
    pub fn dce_aggressive(stmts: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
        stmts
    }
}

#[cfg(target_family = "wasm")]
pub(crate) mod dead_var_dce {
    use crate::HashSet;
    use crate::ir::ProtoStatement;
    use crate::ir::variable::VarOffset;
    pub fn enabled() -> bool {
        false
    }
    pub fn collect_dead_offsets(_slices: &[&[ProtoStatement]]) -> HashSet<VarOffset> {
        HashSet::default()
    }
    pub fn apply_counting(
        stmts: Vec<ProtoStatement>,
        _dead: &HashSet<VarOffset>,
    ) -> (Vec<ProtoStatement>, usize) {
        (stmts, 0)
    }
}
