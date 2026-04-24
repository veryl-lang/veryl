use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Synth {
    /// Default top module name. CLI `--top` overrides when supplied.
    #[serde(default)]
    pub top: Option<String>,
    /// Number of worst-delay endpoints to report in the timing dump.
    #[serde(default = "default_timing_paths")]
    pub timing_paths: usize,
    /// Clock frequency assumed for the dynamic-power estimate (MHz).
    #[serde(default = "default_clock_freq")]
    pub clock_freq: f64,
    /// Per-cycle toggle rate assumed for combinational nets (0.0–1.0).
    /// FF clock input is always assumed to toggle every cycle.
    #[serde(default = "default_activity")]
    pub activity: f64,
    /// Built-in cell library / PDK to use.
    #[serde(default)]
    pub library: Library,
}

/// Built-in PDK library identifiers. Each variant maps to a hard-coded
/// cell-data table in `veryl-synthesizer`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Library {
    /// SkyWater SKY130 130nm PDK (sky130_fd_sc_hd, Apache 2.0).
    #[default]
    #[serde(rename = "sky130")]
    Sky130,
    /// ASU ASAP7 7nm predictive PDK (asap7sc7p5t RVT, BSD 3-Clause).
    #[serde(rename = "asap7")]
    Asap7,
    /// GlobalFoundries GF180MCU 180nm PDK (gf180mcu_fd_sc_mcu7t5v0,
    /// Apache 2.0).
    #[serde(rename = "gf180mcu")]
    Gf180mcu,
    /// IHP SG13G2 130nm SiGe BiCMOS PDK (sg13g2_stdcell, Apache 2.0).
    #[serde(rename = "ihp-sg13g2")]
    IhpSg13g2,
}

impl Default for Synth {
    fn default() -> Self {
        // Route through serde so the `default =` fn pointers are honoured
        // — a naive derive(Default) would zero out the numeric fields.
        toml::from_str("").unwrap()
    }
}

fn default_timing_paths() -> usize {
    1
}

fn default_clock_freq() -> f64 {
    100.0
}

fn default_activity() -> f64 {
    0.1
}
