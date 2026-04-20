pub mod analysis;
pub mod conv;
pub mod error;
pub mod ir;
pub mod library;

pub use analysis::{AreaReport, PathStep, StepKind, TimingReport};
pub use error::SynthError;
pub use ir::{
    Cell, CellKind, ClockEdge, FfCell, GateIr, GateModule, GatePort, NetId, NetInfo, PortDir,
    ResetPolarity, ResetSpec,
};
pub use library::{BuiltinLibrary, CellInfo};

use veryl_analyzer::ir::Ir as AnalyzerIr;
use veryl_parser::resource_table::StrId;

pub fn build_gate_ir(ir: &AnalyzerIr, top: StrId) -> Result<GateIr, SynthError> {
    for c in &ir.components {
        if let veryl_analyzer::ir::Component::Module(m) = c
            && m.name == top
        {
            let module = conv::convert_module(m)?;
            return Ok(GateIr { module });
        }
    }
    Err(SynthError::TopModuleNotFound {
        name: top.to_string(),
    })
}

pub struct SynthResult {
    pub gate_ir: GateIr,
    pub area: AreaReport,
    pub timing: TimingReport,
}

pub fn synthesize(ir: &AnalyzerIr, top: StrId) -> Result<SynthResult, SynthError> {
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
