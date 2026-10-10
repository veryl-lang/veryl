//! Whole-comb compilation, memoised by the comb pipeline's structural key.
//!
//! A shared DUT's C is emitted and fingerprinted once rather than per test:
//! at suite scale the emit is the dominant per-test build cost, and pure
//! waste when the backend declines.

use super::{CompiledWhole, LocalizeInfo};
use crate::ir::Config;
use crate::ir::ProtoStatement;
use std::sync::Arc;

/// What the emitter needs beyond the statements themselves: the per-chunk
/// localization sets and the const-cone split's event-written set.  Each is
/// passed through a thread-local channel around the emit and cleared after,
/// so they travel together.
#[derive(Default)]
pub struct WholeCombShape<'a> {
    pub localize: Option<&'a LocalizeInfo>,
    pub const_unsafe: Option<&'a crate::HashSet<isize>>,
}

pub(crate) fn compile_whole_comb(
    backends: &mut super::BackendRegistry,
    config: &Config,
    key: u128,
    cache: &crate::ir::comb_pipeline_cache::CombPipelineCache,
    stmts: &[ProtoStatement],
    #[cfg_attr(target_family = "wasm", allow(unused_variables))] shape: WholeCombShape<'_>,
) -> Option<Arc<dyn CompiledWhole>> {
    cache.whole_comb_get_or_compute(key, config.dut_reuse, || {
        let ctx = super::CompileCtx {
            config,
            use_4state: config.use_4state,
            contains_compiled_block: false,
            df_marks: None,
        };
        // Thread-local channel to the AOT-C emitter (no-op when
        // VERYL_AOT_C_LOCALIZE is off); cleared after so it cannot leak into
        // a later module's emit.
        #[cfg(not(target_family = "wasm"))]
        if let Some((block, ranges)) = shape.localize {
            super::aot_c::emit::set_localize_blocklist(block.clone(), ranges.clone());
        }
        // Same contract for the const-cone split's event-written-comb set:
        // `None` (unboundable event writes) leaves the split unarmed.
        #[cfg(not(target_family = "wasm"))]
        if let Some(unsafe_comb) = shape.const_unsafe {
            super::aot_c::emit::set_const_unsafe(unsafe_comb.clone());
        }
        let r = backends.try_compile_whole_comb(&ctx, stmts);
        #[cfg(not(target_family = "wasm"))]
        super::aot_c::emit::clear_localize_blocklist();
        #[cfg(not(target_family = "wasm"))]
        super::aot_c::emit::clear_const_unsafe();
        r
    })
}
