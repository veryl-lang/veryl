use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Test {
    #[serde(default)]
    pub simulator: SimType,
    #[serde(default)]
    pub dsim: DsimProperty,
    #[serde(default)]
    pub vcs: VcsProperty,
    #[serde(default)]
    pub verilator: VerilatorProperty,
    #[serde(default)]
    pub vivado: VivadoProperty,
    #[serde(default)]
    pub waveform_target: WaveFormTarget,
    #[serde(default)]
    pub waveform_format: WaveFormFormat,
    #[serde(default)]
    pub include_files: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum SimType {
    #[default]
    #[serde(rename = "verilator")]
    Verilator,
    #[serde(rename = "vcs")]
    Vcs,
    #[serde(rename = "dsim")]
    Dsim,
    #[serde(rename = "vivado")]
    Vivado,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DsimProperty {
    #[serde(default)]
    pub compile_args: Vec<String>,
    #[serde(default)]
    pub simulate_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VcsProperty {
    #[serde(default)]
    pub compile_args: Vec<String>,
    #[serde(default)]
    pub simulate_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerilatorProperty {
    #[serde(default)]
    pub compile_args: Vec<String>,
    #[serde(default)]
    pub simulate_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VivadoProperty {
    #[serde(default)]
    pub compile_args: Vec<String>,
    #[serde(default)]
    pub elaborate_args: Vec<String>,
    #[serde(default)]
    pub simulate_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum WaveFormTarget {
    #[default]
    #[serde(rename = "target")]
    Target,
    #[serde(rename = "directory")]
    Directory { path: PathBuf },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum WaveFormFormat {
    #[default]
    #[serde(rename = "vcd")]
    Vcd,
    #[serde(rename = "fst")]
    Fst,
}

impl WaveFormFormat {
    pub fn extension(self) -> &'static str {
        match self {
            WaveFormFormat::Vcd => "vcd",
            WaveFormFormat::Fst => "fst",
        }
    }
}
