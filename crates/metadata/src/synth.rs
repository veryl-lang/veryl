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
