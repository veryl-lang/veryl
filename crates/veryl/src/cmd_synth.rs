use crate::OptSynth;
use crate::pipeline::{self, AnalyzeOptions};
use log::warn;
use miette::Result;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::env;
use std::time::Instant;
use veryl_analyzer::ir::{Component, Ir, Module};
use veryl_metadata::Metadata;
use veryl_parser::resource_table::{self, PathId};
use veryl_parser::veryl_token::TokenSource;
use veryl_synthesizer::{
    RamConfig, compute_power, compute_timing_top_n, library_for, port_label, synthesize_with,
};

pub struct CmdSynth {
    opt: OptSynth,
}

impl CmdSynth {
    pub fn new(opt: OptSynth) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        // For the default-top heuristic below.
        let user_paths: HashSet<PathId> = paths
            .iter()
            .filter(|path| path.prj != "$std")
            .map(|path| resource_table::insert_path(&path.src))
            .collect();

        // Abort on a fatal error: synthesizing a broken design is meaningless.
        let options = AnalyzeOptions {
            defines: &[],
            emit_mode: false,
            incremental: false,
            fail_fast: true,
        };
        let mut ir = Ir::default();
        let timed = env::var_os("VERYL_SYNTH_TIME").is_some();
        let t_analyze = Instant::now();
        let _ = pipeline::analyze(metadata, &paths, options, Some(&mut ir), None)?;
        if timed {
            eprintln!(
                "[synth-time] analyze (parse+pass1/2): {:.3}s",
                t_analyze.elapsed().as_secs_f64()
            );
        }

        // CLI `--top` > toml default > first user module.
        let top_override = self.opt.top.as_ref().or(metadata.synth.top.as_ref());
        let top_id = match top_override {
            Some(name) => resource_table::insert_str(name),
            None => {
                let mut candidate = None;
                for c in &ir.components {
                    if let Component::Module(m) = c
                        && is_user_module(m, &user_paths)
                    {
                        candidate = Some(m.name);
                        break;
                    }
                }
                match candidate {
                    Some(id) => id,
                    None => {
                        warn!("No module found to synthesize");
                        return Ok(false);
                    }
                }
            }
        };

        let library = library_for(metadata.synth.library);

        let ram_config = RamConfig::from(&metadata.synth);
        let result = match synthesize_with(&ir, top_id, metadata.synth.library, ram_config) {
            Ok(r) => r,
            Err(err) => {
                warn!("Synthesis failed: {}", err);
                return Ok(false);
            }
        };
        println!(
            "synth: {} — {} gates, {} FFs",
            top_id,
            result.gate_ir.module.cells.len(),
            result.gate_ir.module.ffs.len()
        );
        println!("library: {}", library.banner());

        let nothing_selected = !self.opt.dump_ir
            && !self.opt.dump_area
            && !self.opt.dump_timing
            && !self.opt.dump_power;
        let show_area = self.opt.dump_area || nothing_selected;
        let show_timing = self.opt.dump_timing || nothing_selected;
        let show_power = self.opt.dump_power || nothing_selected;
        let show_any_report = show_area || show_timing || show_power;

        // Computed once; the summary line and the detail block both need it.
        let power = compute_power(
            &result.gate_ir.module,
            library,
            metadata.synth.clock_freq,
            metadata.synth.activity,
        );

        if self.opt.dump_ir {
            println!("\n-- gate ir --");
            println!("{}", result.gate_ir);
        }

        // Top-level numbers up front so detail blocks don't have to be scanned.
        if show_any_report {
            let start = port_label(result.timing.critical_path.first());
            let end = port_label(result.timing.critical_path.last());
            println!();
            println!("summary:");
            // Width 11 fits up to ~10 M um² / 1 W without breaking alignment.
            println!(
                "  {:<8}{:>11.2} um²  (comb {:.2}, seq {:.2}, mem {:.2})",
                "area:",
                result.area.total,
                result.area.combinational,
                result.area.sequential,
                result.area.memory,
            );
            println!(
                "  {:<8}{:>11.3} ns   {:>5} levels  {} → {}",
                "timing:",
                result.timing.critical_path_delay,
                result.timing.critical_path_depth,
                start,
                end,
            );
            println!(
                "  {:<8}{:>11.4} mW   (leak {:.4} mW, dyn {:.4} mW)",
                "power:", power.total_mw, power.leakage_mw, power.dynamic_mw,
            );
            // Pad the continuation to the same column as values above.
            println!(
                "  {:<8}{:>11} @ f_clk = {} MHz, activity = {:.2}",
                "", "", power.clock_freq_mhz, power.activity,
            );
        }

        if show_area {
            println!();
            println!("area:");
            // Skip Display's first line — already shown in the summary.
            let full = format!("{}", result.area);
            for line in full.lines().skip(1) {
                println!("{}", line);
            }
        }
        // Inferred-RAM breakdown: only with explicit --dump-area, grouped by
        // shape (depth × width) and port count.
        if self.opt.dump_area {
            let rams = &result.gate_ir.module.ram_blocks;
            if !rams.is_empty() {
                let bit_area = library.sram_model().bit_area;
                let mut groups: HashMap<(usize, usize, usize, usize), usize> = HashMap::new();
                for r in rams {
                    *groups
                        .entry((r.depth, r.width, r.read_ports.len(), r.write_ports.len()))
                        .or_insert(0) += 1;
                }
                let mut rows: Vec<_> = groups.into_iter().collect();
                // Largest stored-bit groups first.
                rows.sort_by_key(|&((d, w, _, _), c)| Reverse(d * w * c));
                println!();
                println!("ram: {} blocks", rams.len());
                for ((depth, width, reads, writes), count) in rows {
                    let bits = depth * width * count;
                    println!(
                        "  {:>6}×{:<4} {}R{}W  ×{:<3} {:>9} bits  {:>12.2} um²",
                        depth,
                        width,
                        reads,
                        writes,
                        count,
                        bits,
                        bits as f64 * bit_area,
                    );
                }
            }
        }
        if show_timing {
            let n = self
                .opt
                .timing_paths
                .unwrap_or(metadata.synth.timing_paths)
                .max(1);
            if n == 1 {
                println!();
                println!("timing:");
                let full = format!("{}", result.timing);
                for line in full.lines().skip(1) {
                    println!("{}", line);
                }
            } else {
                let reports = compute_timing_top_n(&result.gate_ir.module, library, n);
                println!();
                println!("timing (top {} endpoints):", reports.len());
                for (rank, report) in reports.iter().enumerate() {
                    println!("  #{}  {}", rank + 1, report.summary());
                }
                for (rank, report) in reports.iter().enumerate() {
                    println!();
                    println!("path #{}", rank + 1);
                    // Skip Display's "timing:" — we print a per-rank label above.
                    let full = format!("{}", report);
                    for line in full.lines().skip(1) {
                        println!("{}", line);
                    }
                }
            }
        }
        if show_power {
            println!();
            println!("power:");
            // Skip Display's first two lines (totals + assumptions) — in summary.
            let full = format!("{}", power);
            for line in full.lines().skip(2) {
                println!("{}", line);
            }
        }

        Ok(true)
    }
}

fn is_user_module(m: &Module, user_paths: &HashSet<PathId>) -> bool {
    match m.token.beg.source {
        TokenSource::File { path, .. } => user_paths.contains(&path),
        _ => false,
    }
}
