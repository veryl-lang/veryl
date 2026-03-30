use crate::ir::Ir;
use crate::ir::{Config, ModuleVariables, build_ir, parse_hex_content};
use crate::ir::{Event, Value};
use crate::simulator::Simulator;
use crate::simulator_error::SimulatorError;
use crate::testbench::{
    TestResult, TestbenchStatement, build_clock_periods, build_event_map,
    convert_initial_to_testbench, run_native_testbench, run_testbench,
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
        .filter(|x| {
            !matches!(
                x,
                AnalyzerError::InvalidLogicalOperand { .. }
                    | AnalyzerError::UnsignedArithShift { .. }
            )
        })
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
        .filter(|x| {
            !matches!(
                x,
                AnalyzerError::InvalidLogicalOperand { .. }
                    | AnalyzerError::UnsignedArithShift { .. }
            )
        })
        .collect();
    assert!(errors.is_empty());

    build_ir(&ir, top.into(), config)
}

/// Find the variable name that contains the given byte offset in ff or comb buffer.
/// Uses raw pointer arithmetic to match variable current_values against buffer base.
fn find_var_at_offset(
    module: &ModuleVariables,
    buf_base: *const u8,
    buf_len: usize,
    target_offset: usize,
    prefix: &str,
) -> Option<String> {
    for var in module.variables.values() {
        for &ptr in &var.current_values {
            let ptr_addr = ptr as usize;
            let base_addr = buf_base as usize;
            if ptr_addr >= base_addr && ptr_addr < base_addr + buf_len {
                let var_offset = ptr_addr - base_addr;
                let nb = var.native_bytes;
                if target_offset >= var_offset && target_offset < var_offset + nb {
                    let path = if prefix.is_empty() {
                        format!("{}", var.path)
                    } else {
                        format!("{}.{}", prefix, var.path)
                    };
                    return Some(path);
                }
            }
        }
    }
    for child in &module.children {
        let child_prefix = if prefix.is_empty() {
            format!("{}", child.name)
        } else {
            format!("{}.{}", prefix, child.name)
        };
        if let Some(name) =
            find_var_at_offset(child, buf_base, buf_len, target_offset, &child_prefix)
        {
            return Some(name);
        }
    }
    None
}

/// Dual simulator that runs JIT and interpreter in lockstep, comparing state
/// after each step. Panics on the first byte-level difference with detailed
/// diagnostic output including cycle number, offset, and variable name.
struct DualSimulator {
    jit: Simulator,
    interp: Simulator,
    use_4state: bool,
    cycle: usize,
}

impl DualSimulator {
    #[track_caller]
    fn new(code: &str, use_4state: bool) -> Self {
        let jit_config = Config {
            use_4state,
            use_jit: true,
            ..Default::default()
        };
        let interp_config = Config {
            use_4state,
            use_jit: false,
            ..Default::default()
        };

        let jit_ir = analyze(code, &jit_config);
        let interp_ir = analyze(code, &interp_config);

        let jit = Simulator::new(jit_ir, None);
        let interp = Simulator::new(interp_ir, None);

        DualSimulator {
            jit,
            interp,
            use_4state,
            cycle: 0,
        }
    }

    #[track_caller]
    fn new_with_top(code: &str, use_4state: bool, top: &str) -> Self {
        let jit_config = Config {
            use_4state,
            use_jit: true,
            ..Default::default()
        };
        let interp_config = Config {
            use_4state,
            use_jit: false,
            ..Default::default()
        };

        let jit_ir = analyze_top(code, &jit_config, top).unwrap();
        let interp_ir = analyze_top(code, &interp_config, top).unwrap();

        let jit = Simulator::new(jit_ir, None);
        let interp = Simulator::new(interp_ir, None);

        DualSimulator {
            jit,
            interp,
            use_4state,
            cycle: 0,
        }
    }

    fn set(&mut self, port: &str, value: Value) {
        self.jit.set(port, value.clone());
        self.interp.set(port, value);
    }

    fn get_clock(&self, port: &str) -> (Event, Event) {
        (
            self.jit.get_clock(port).unwrap(),
            self.interp.get_clock(port).unwrap(),
        )
    }

    fn get_reset(&self, port: &str) -> (Event, Event) {
        (
            self.jit.get_reset(port).unwrap(),
            self.interp.get_reset(port).unwrap(),
        )
    }

    #[track_caller]
    fn step(&mut self, jit_event: &Event, interp_event: &Event) {
        self.jit.step(jit_event);
        self.interp.step(interp_event);
        self.cycle += 1;

        // Ensure comb is settled on both
        self.jit.ensure_comb_updated();
        self.interp.ensure_comb_updated();

        self.compare_buffers();
    }

    #[track_caller]
    fn step_clock(&mut self, port: &str) {
        let (jit_ev, interp_ev) = self.get_clock(port);
        self.step(&jit_ev, &interp_ev);
    }

    #[track_caller]
    fn step_reset(&mut self, port: &str) {
        let (jit_ev, interp_ev) = self.get_reset(port);
        self.step(&jit_ev, &interp_ev);
    }

    /// Step with a synthetic clock event (for modules without a real clock port).
    #[track_caller]
    fn step_synthetic(&mut self) {
        let ev = Event::Clock(VarId::SYNTHETIC);
        self.step(&ev, &ev);
    }

    #[track_caller]
    fn compare_buffers(&self) {
        // Compare FF values
        self.compare_buffer_region(
            &self.jit.ir.ff_values,
            &self.interp.ir.ff_values,
            self.jit.ir.ff_values.as_ptr(),
            self.jit.ir.ff_values.len(),
            "ff_values",
        );
        // Compare comb values
        self.compare_buffer_region(
            &self.jit.ir.comb_values,
            &self.interp.ir.comb_values,
            self.jit.ir.comb_values.as_ptr(),
            self.jit.ir.comb_values.len(),
            "comb_values",
        );
    }

    #[track_caller]
    fn compare_buffer_region(
        &self,
        jit_buf: &[u8],
        interp_buf: &[u8],
        buf_base: *const u8,
        buf_len: usize,
        label: &str,
    ) {
        assert_eq!(
            jit_buf.len(),
            interp_buf.len(),
            "cycle {}: {} length mismatch: JIT={} vs interp={}",
            self.cycle,
            label,
            jit_buf.len(),
            interp_buf.len()
        );

        for (offset, (jit_byte, interp_byte)) in jit_buf.iter().zip(interp_buf.iter()).enumerate() {
            if jit_byte != interp_byte {
                let var_name = find_var_at_offset(
                    &self.jit.ir.module_variables,
                    buf_base,
                    buf_len,
                    offset,
                    "",
                )
                .unwrap_or_else(|| "<unknown>".to_string());

                panic!(
                    "JIT/interpreter mismatch at cycle {}, {} offset {:#x} (variable: {})\n\
                     JIT byte:    {:#04x}\n\
                     Interp byte: {:#04x}\n\
                     use_4state: {}",
                    self.cycle, label, offset, var_name, jit_byte, interp_byte, self.use_4state,
                );
            }
        }
    }

    /// Get a port value from the JIT simulator.
    fn get(&mut self, port: &str) -> Option<Value> {
        self.jit.get(port)
    }

    /// Get a variable value by hierarchical path from the JIT simulator.
    fn get_var(&mut self, path: &str) -> Option<Value> {
        self.jit.get_var(path)
    }
}

/// Run a test with DualSimulator for both use_4state=false and use_4state=true.
/// The closure receives a `&mut DualSimulator` and drives the simulation.
#[track_caller]
fn verify_jit_interpreter_equivalence<F>(code: &str, driver: F)
where
    F: Fn(&mut DualSimulator),
{
    for use_4state in [false, true] {
        let mut dual = DualSimulator::new(code, use_4state);
        driver(&mut dual);
    }
}

mod error;
mod simulation;
mod testbench;
