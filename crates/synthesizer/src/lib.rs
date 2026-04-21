pub mod analysis;
pub mod conv;
pub mod ir;
pub mod library;
pub mod synthesizer_error;

pub use analysis::{AreaReport, PathStep, StepKind, TimingReport, compute_timing_top_n};
pub use ir::{
    Cell, CellKind, ClockEdge, FfCell, GateIr, GateModule, GatePort, NetId, NetInfo, PortDir,
    ResetPolarity, ResetSpec,
};
pub use library::{BuiltinLibrary, CellInfo};
pub use synthesizer_error::SynthesizerError;

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

pub fn synthesize(ir: &AnalyzerIr, top: StrId) -> Result<SynthResult, SynthesizerError> {
    let gate_ir = build_gate_ir(ir, top)?;
    let library = BuiltinLibrary::new();
    let area = analysis::compute_area(&gate_ir.module, &library);
    let timing = analysis::compute_timing(&gate_ir.module, &library);
    Ok(SynthResult {
        gate_ir,
        area,
        timing,
    })
}
