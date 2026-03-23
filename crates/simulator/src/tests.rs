use crate::ir::Ir;
use crate::ir::{Config, build_ir, parse_hex_content};
use crate::ir::{Event, Value};
use crate::simulator::Simulator;
use crate::simulator_error::SimulatorError;
use crate::testbench::{
    TestResult, TestbenchStatement, build_event_map, convert_initial_to_testbench,
    run_native_testbench, run_testbench,
};
use std::str::FromStr;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::VarId;
use veryl_analyzer::{Analyzer, AnalyzerError, Context, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn analyze(code: &str, config: &Config) -> Ir {
    analyze_top(code, config, "Top").unwrap()
}

#[track_caller]
fn analyze_top(code: &str, config: &Config, top: &str) -> Result<Ir, SimulatorError> {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut errors = vec![];
    let mut ir = air::Ir::default();
    errors.append(&mut analyzer.analyze_pass1(&"prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2());

    dbg!(&errors);
    let errors: Vec<_> = errors
        .drain(0..)
        .filter(|x| !matches!(x, AnalyzerError::InvalidLogicalOperand { .. }))
        .collect();
    assert!(errors.is_empty());

    build_ir(&ir, top.into(), config)
}

/// Analyze with per-file project names (simulates different prj for std vs user code)
#[track_caller]
fn analyze_multi_file_prj(
    files: &[&str],
    config: &Config,
    top: &str,
    prjs: &[&str],
) -> Result<Ir, SimulatorError> {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();

    // Parse all files and run pass1 for each
    let mut parsers = vec![];
    let mut analyzers = vec![];
    let mut all_errors = vec![];

    for (i, code) in files.iter().enumerate() {
        let path = format!("file{}.veryl", i);
        let prj = if i < prjs.len() {
            prjs[i]
        } else {
            prjs[prjs.len() - 1]
        };
        let parser = Parser::parse(code, &path).unwrap();
        let analyzer = Analyzer::new(&metadata);
        all_errors.append(&mut analyzer.analyze_pass1(prj, &parser.veryl));
        parsers.push((prj.to_string(), parser));
        analyzers.push(analyzer);
    }

    all_errors.append(&mut Analyzer::analyze_post_pass1());

    // Pass2 for each file, accumulating IR
    let mut context = Context::default();
    let mut ir = air::Ir::default();
    for ((prj, parser), analyzer) in parsers.iter().zip(analyzers.iter()) {
        all_errors.append(&mut analyzer.analyze_pass2(
            prj,
            &parser.veryl,
            &mut context,
            Some(&mut ir),
        ));
    }
    all_errors.append(&mut Analyzer::analyze_post_pass2());

    dbg!(&all_errors);
    let errors: Vec<_> = all_errors
        .drain(0..)
        .filter(|x| !matches!(x, AnalyzerError::InvalidLogicalOperand { .. }))
        .collect();
    assert!(errors.is_empty());

    build_ir(&ir, top.into(), config)
}

mod error;
mod simulation;
mod testbench;
