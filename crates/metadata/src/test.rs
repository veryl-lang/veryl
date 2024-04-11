use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Test {
    #[serde(default)]
    pub simulator: SimType,
    #[serde(default)]
    pub vcs: VcsProperty,
    #[serde(default)]
    pub verilator: VerilatorProperty,
    #[serde(default)]
    pub vivado: VivadoProperty,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum SimType {
    #[default]
    #[serde(rename = "verilator")]
    Verilator,
    #[serde(rename = "vcs")]
    Vcs,
    #[serde(rename = "vivado")]
    Vivado,
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
