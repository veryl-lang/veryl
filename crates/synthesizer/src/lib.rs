#[cfg(feature = "aig")]
pub mod aig;
pub mod analysis;
pub mod conv;
pub mod ir;
pub mod library;
pub mod synthesizer_error;

pub use analysis::{
    AreaReport, PathStep, PowerKindRow, PowerReport, StepKind, TimingReport, compute_power,
    compute_timing_top_n, port_label,
};
pub use ir::{
    Cell, CellKind, ClockEdge, FfCell, GateIr, GateModule, GatePort, NetId, NetInfo, PortDir,
    RamBlock, RamReadPort, RamWritePort, ResetPolarity, ResetSpec,
};
pub use library::{CellInfo, CellLibrary, SramModel, library_for};
pub use synthesizer_error::SynthesizerError;
pub use veryl_metadata::Library;

use std::env;
use std::time::Instant;
use veryl_analyzer::ir::Ir as AnalyzerIr;
use veryl_parser::resource_table::StrId;

pub fn build_gate_ir(ir: &AnalyzerIr, top: StrId) -> Result<GateIr, SynthesizerError> {
    for c in &ir.components {
        if let veryl_analyzer::ir::Component::Module(m) = c
            && m.name == top
        {
            let module = conv::convert_module(m)?;
            return Ok(GateIr { module });
        }
    }
    Err(SynthesizerError::TopModuleNotFound {
        name: top.to_string(),
    })
}

pub struct SynthResult {
    pub gate_ir: GateIr,
    pub area: AreaReport,
    pub timing: TimingReport,
}

pub fn synthesize(
    ir: &AnalyzerIr,
    top: StrId,
    library: Library,
) -> Result<SynthResult, SynthesizerError> {
    let timed = env::var_os("VERYL_SYNTH_TIME").is_some();
    let t = Instant::now();
    let gate_ir = build_gate_ir(ir, top)?;
    if timed {
        eprintln!(
            "[synth-time] build_gate_ir: {:.3}s ({} cells, {} ffs, {} rams)",
            t.elapsed().as_secs_f64(),
            gate_ir.module.cells.len(),
            gate_ir.module.ffs.len(),
            gate_ir.module.ram_blocks.len(),
        );
    }
    let lib = library_for(library);
    let t = Instant::now();
    let area = analysis::compute_area(&gate_ir.module, lib);
    if timed {
        eprintln!(
            "[synth-time] compute_area: {:.3}s",
            t.elapsed().as_secs_f64()
        );
    }
    let t = Instant::now();
    let timing = analysis::compute_timing(&gate_ir.module, lib);
    if timed {
        eprintln!(
            "[synth-time] compute_timing: {:.3}s",
            t.elapsed().as_secs_f64()
        );
    }
    Ok(SynthResult {
        gate_ir,
        area,
        timing,
    })
}
