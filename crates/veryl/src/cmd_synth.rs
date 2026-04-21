use crate::OptSynth;
use crate::context::Context;
use log::{info, warn};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::HashSet;
use std::fs;
use veryl_analyzer::ir::{Component, Ir, Module};
use veryl_analyzer::{Analyzer, Context as AnalyzerContext};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_parser::resource_table::{self, PathId};
use veryl_parser::veryl_token::TokenSource;
use veryl_synthesizer::{BuiltinLibrary, compute_timing_top_n, synthesize};

pub struct CmdSynth {
    opt: OptSynth,
}

impl CmdSynth {
    pub fn new(opt: OptSynth) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        let mut contexts = Vec::new();
        let mut user_paths: HashSet<PathId> = HashSet::new();
        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());
            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;
            let analyzer = Analyzer::new(metadata);
            analyzer.analyze_pass1(&path.prj, &parser.veryl);
            if path.prj != "$std" {
                user_paths.insert(resource_table::insert_path(&path.src));
            }
            let context = Context::new(path.clone(), input, parser, analyzer)?;
            contexts.push(context);
        }

        Analyzer::analyze_post_pass1();

        let mut ir = Ir::default();
        let mut analyzer_context = AnalyzerContext::default();
        for context in &contexts {
            let path = &context.path;
            context.analyzer.analyze_pass2(
                &path.prj,
                &context.parser.veryl,
                &mut analyzer_context,
                Some(&mut ir),
            );
        }

        Analyzer::analyze_post_pass2();

        let top_id = match &self.opt.top {
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

        let result = match synthesize(&ir, top_id) {
            Ok(r) => r,
            Err(err) => {
                warn!("Synthesis failed: {}", err);
                return Ok(false);
            }
        };

        let library = BuiltinLibrary::new();
        println!(
            "synth: {} — {} gates, {} FFs",
            top_id,
            result.gate_ir.module.cells.len(),
            result.gate_ir.module.ffs.len()
        );
        println!("library: {}", library.banner());

        let nothing_selected = !self.opt.dump_ir && !self.opt.dump_area && !self.opt.dump_timing;

        if self.opt.dump_ir {
            println!("\n-- gate ir --");
            println!("{}", result.gate_ir);
        }
        if self.opt.dump_area || nothing_selected {
            println!();
            print!("{}", result.area);
        }
        if self.opt.dump_timing || nothing_selected {
            let n = self.opt.top_paths.max(1);
            if n == 1 {
                println!();
                print!("{}", result.timing);
            } else {
                let reports = compute_timing_top_n(&result.gate_ir.module, &library, n);
                println!();
                println!("timing (top {} endpoints):", reports.len());
                for (rank, report) in reports.iter().enumerate() {
                    println!("  #{}  {}", rank + 1, report.summary());
                }
                for (rank, report) in reports.iter().enumerate() {
                    println!();
                    println!("path #{}", rank + 1);
                    // Skip the Display header ("timing:") since we already
                    // printed a per-rank label above.
                    let full = format!("{}", report);
                    for line in full.lines().skip(1) {
                        println!("{}", line);
                    }
                }
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
