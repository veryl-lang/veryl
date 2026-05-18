use crate::HashMap;
use miette::{self, IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default, Serialize)]
pub struct ConvStats {
    pub count: u64,
    pub total_us: u64,
    pub avg_us: u64,
    pub min_us: u64,
    pub max_us: u64,
}

#[derive(Clone, Default)]
pub struct ConvProfile {
    pub entries: HashMap<String, ConvStats>,
}

impl ConvProfile {
    pub fn record(&mut self, name: &str, duration: Duration) {
        let us = duration.as_micros() as u64;
        let entry = self.entries.entry(name.to_string()).or_default();
        if entry.count == 0 {
            entry.min_us = us;
            entry.max_us = us;
        } else {
            entry.min_us = entry.min_us.min(us);
            entry.max_us = entry.max_us.max(us);
        }
        entry.total_us += us;
        entry.count += 1;
        entry.avg_us = entry.total_us / entry.count;
    }

    pub fn to_toml_file(&self, path: &str) -> Result<()> {
        let toml_string = self.to_toml_string();
        fs::write(path, toml_string)
            .into_diagnostic()
            .wrap_err(format!("Failed to write conv profile to {path}"))
    }

    pub fn to_toml_string(&self) -> String {
        use std::collections::BTreeMap;
        let output: BTreeMap<_, _> = self.entries.iter().collect();
        toml::to_string(&output).unwrap_or_default()
    }
}

pub struct ConvProfileGuard {
    profiler: Arc<Mutex<ConvProfile>>,
    name: String,
    start: Instant,
}

impl ConvProfileGuard {
    pub fn new(profiler: Arc<Mutex<ConvProfile>>, name: impl Into<String>) -> Self {
        Self {
            profiler,
            name: name.into(),
            start: Instant::now(),
        }
    }
}

impl Drop for ConvProfileGuard {
    fn drop(&mut self) {
        if let Ok(mut p) = self.profiler.lock() {
            p.record(&self.name, self.start.elapsed());
        }
    }
}
