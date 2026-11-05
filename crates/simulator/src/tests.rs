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

/// Whether the comb runs as AOT-C code, i.e. the dataflow settle's `.so`.
/// With dataflow off the comb has no AOT-C path, so this holds vacuously.
pub(crate) fn comb_is_aot_native(ir: &Ir) -> bool {
    !dataflow_on() || ir.dataflow.as_ref().is_some_and(|d| d.cell.get().is_some())
}

pub(crate) fn dataflow_on() -> bool {
    crate::ir::opt::dataflow::enabled()
}

/// xorshift64*, for generated tests: deterministic across platforms.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

/// Whether the AOT-C comb can land on this host. A C compiler is not enough:
/// where the shared object builds but does not load, the comb silently stays
/// on Cranelift, so a test asserting native coverage must skip there.
pub(crate) fn aot_c_loads() -> bool {
    static LOADS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOADS.get_or_init(|| {
        if !crate::backend::aot_c::cc_available() {
            return false;
        }
        let code = r#"
        module Top (a: input logic<8>, b: output logic<8>) {
            assign b = a + 1;
        }
        "#;
        let config = Config {
            use_4state: false,
            use_jit: true,
            aot_c: true,
            aot_c_async: false,
            aot_c_min_stmts: 0,
            ..Default::default()
        };
        comb_is_aot_native(&analyze(code, &config))
    })
}

#[track_caller]
fn analyze(code: &str, config: &Config) -> Ir {
    analyze_top(code, config, "Top").unwrap()
}

/// The analyzer diagnostic a test deliberately provokes. Anything else it
/// reports fails the test, so a design cannot be waved through by accident.
#[derive(Clone, Copy, PartialEq)]
enum Allowed {
    Nothing,
    CombLoop,
    MismatchAssignment,
}

impl Allowed {
    fn permits(self, error: &AnalyzerError) -> bool {
        match self {
            Allowed::Nothing => false,
            Allowed::CombLoop => matches!(error, AnalyzerError::CombinationalLoop { .. }),
            // The unpacked-dimension mismatch is one kind of this warning.
            Allowed::MismatchAssignment => {
                matches!(error, AnalyzerError::MismatchAssignment { .. })
            }
        }
    }
}

#[track_caller]
fn analyze_top(code: &str, config: &Config, top: &str) -> Result<Ir, SimulatorError> {
    analyze_top_inner(code, config, top, Allowed::Nothing, &[])
}

#[track_caller]
fn analyze_with_defines(code: &str, config: &Config, defines: &[&str]) -> Ir {
    analyze_top_inner(code, config, "Top", Allowed::Nothing, defines).unwrap()
}

/// The analyzer IR itself, for the tests that build more than one top out of
/// one design (cross-test DUT reuse keys on `Arc<Component>` identity, which
/// only holds within a single `air::Ir`).
#[track_caller]
fn analyze_air(code: &str) -> air::Ir {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut errors = vec![];
    let mut ir = air::Ir::default();
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));

    dbg!(&errors);
    assert!(errors.is_empty());
    ir
}

/// For the tests targeting the simulator's own combinational-loop detector:
/// the analyzer rejects such designs first, so nothing else may wave it through.
#[track_caller]
fn analyze_top_allowing_comb_loop(
    code: &str,
    config: &Config,
    top: &str,
) -> Result<Ir, SimulatorError> {
    analyze_top_inner(code, config, top, Allowed::CombLoop, &[])
}

/// For the tests targeting a port connection the analyzer already reports, at
/// Warning severity, so `build` and `test` still reach the simulator with it.
#[track_caller]
fn analyze_top_allowing_mismatch_assignment(
    code: &str,
    config: &Config,
    top: &str,
) -> Result<Ir, SimulatorError> {
    analyze_top_inner(code, config, top, Allowed::MismatchAssignment, &[])
}

#[track_caller]
fn analyze_top_inner(
    code: &str,
    config: &Config,
    top: &str,
    allow: Allowed,
    defines: &[&str],
) -> Result<Ir, SimulatorError> {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    context.config.defines = defines
        .iter()
        .map(|x| veryl_parser::resource_table::insert_str(x))
        .collect();
    symbol_table::set_defines(&context.config.defines);

    let mut errors = vec![];
    let mut ir = air::Ir::default();
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));

    dbg!(&errors);
    let errors: Vec<_> = errors
        .drain(0..)
        .filter(|x| {
            !matches!(
                x,
                AnalyzerError::InvalidLogicalOperand { .. }
                    | AnalyzerError::UnsignedArithShift { .. }
            ) && !allow.permits(x)
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
    for ((_prj, parser), analyzer) in parsers.iter().zip(analyzers.iter()) {
        all_errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    }
    all_errors.append(&mut Analyzer::analyze_post_pass2(&ir));

    dbg!(&all_errors);
    let errors: Vec<_> = all_errors
        .drain(0..)
        .filter(|x| {
            !matches!(
                x,
                AnalyzerError::InvalidLogicalOperand { .. }
                    | AnalyzerError::UnsignedArithShift { .. }
                    | AnalyzerError::CombinationalLoop { .. }
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
                        format!("{}.cur", var.path)
                    } else {
                        format!("{}.{}.cur", prefix, var.path)
                    };
                    return Some(path);
                }
            }
        }
        for &ptr in &var.next_values {
            let ptr_addr = ptr as usize;
            let base_addr = buf_base as usize;
            if ptr_addr >= base_addr && ptr_addr < base_addr + buf_len {
                let var_offset = ptr_addr - base_addr;
                let nb = var.native_bytes;
                if target_offset >= var_offset && target_offset < var_offset + nb {
                    let path = if prefix.is_empty() {
                        format!("{}.nxt", var.path)
                    } else {
                        format!("{}.{}.nxt", prefix, var.path)
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
        // A reset step is a clock edge with the net asserted; these fixtures
        // have one clock, so call sites keep passing just the reset event.
        if matches!(jit_event, Event::Reset(..)) {
            let jit_clock = sole_clock_event(&self.jit);
            let interp_clock = sole_clock_event(&self.interp);
            self.jit.step_reset(&jit_clock, jit_event);
            self.interp.step_reset(&interp_clock, interp_event);
        } else {
            self.jit.step(jit_event);
            self.interp.step(interp_event);
        }
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
        let ev = Event::clock(VarId::SYNTHETIC);
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
        // Compare comb values.  Under the comb relayout (or fusion) the two
        // arms transform their storage independently (the JIT arm's
        // compiled-block structure keys a different schedule / different
        // defs disappear), so byte-identical buffers are no longer expected
        // — compare per-variable through each arm's own meta instead.
        if crate::ir::comb_layout::enabled(self.jit.ir.use_4state)
            || crate::ir::opt::comb_fusion::enabled(self.jit.ir.use_4state)
        {
            self.compare_vars_recursive(
                &self.jit.ir.module_variables,
                &self.interp.ir.module_variables,
            );
        } else {
            self.compare_buffer_region(
                &self.jit.ir.comb_values,
                &self.interp.ir.comb_values,
                self.jit.ir.comb_values.as_ptr(),
                self.jit.ir.comb_values.len(),
                "comb_values",
            );
        }
    }

    #[track_caller]
    fn compare_vars_recursive(&self, jit: &ModuleVariables, interp: &ModuleVariables) {
        // The two arms must expose the SAME variable tree — a silent skip
        // here would exempt whatever storage one arm forgot from comparison.
        assert_eq!(
            jit.variables.len(),
            interp.variables.len(),
            "variable sets differ between the arms",
        );
        assert_eq!(
            jit.children.len(),
            interp.children.len(),
            "child module counts differ between the arms",
        );
        // Storage the fusion pass retired is never written again, and the two
        // arms retire different defs (only the interp arm is CompiledBlock
        // free), so skip any element whose pointer lands in either arm's
        // fused set.
        let fused = |ir: &crate::ir::Ir, ptr: *mut u8| -> bool {
            if ir.fused_comb_offsets.is_empty() {
                return false;
            }
            let base = ir.comb_values.as_ptr() as usize;
            let addr = ptr as usize;
            if addr < base || addr >= base + ir.comb_values.len() {
                return false;
            }
            let off = (addr - base) as isize;
            ir.fused_comb_offsets.contains(&off)
        };
        let mut ids: Vec<_> = jit.variables.keys().collect();
        ids.sort();
        for vid in ids {
            let jv = &jit.variables[vid];
            let iv = interp
                .variables
                .get(vid)
                .unwrap_or_else(|| panic!("variable {} missing from the interp arm", jv.path));
            let vs = crate::ir::variable::value_size(jv.native_bytes, self.use_4state);
            assert_eq!(
                jv.current_values.len(),
                iv.current_values.len(),
                "value counts differ for {}",
                jv.path,
            );
            for (i, (&jp, &ip)) in jv
                .current_values
                .iter()
                .zip(iv.current_values.iter())
                .enumerate()
            {
                if fused(&self.jit.ir, jp) || fused(&self.interp.ir, ip) {
                    continue;
                }
                let jb = unsafe { std::slice::from_raw_parts(jp, vs) };
                let ib = unsafe { std::slice::from_raw_parts(ip, vs) };
                if jb != ib {
                    panic!(
                        "JIT/interpreter mismatch at cycle {}, variable {}[{}]\n\
                         JIT bytes:    {:02x?}\n\
                         Interp bytes: {:02x?}\n\
                         use_4state: {}",
                        self.cycle, jv.path, i, jb, ib, self.use_4state,
                    );
                }
            }
        }
        for (jc, ic) in jit.children.iter().zip(interp.children.iter()) {
            self.compare_vars_recursive(jc, ic);
        }
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
}

/// The clock event of a single-clock fixture.  Lowest VarId wins so a
/// multi-clock fixture still resolves deterministically.
fn sole_clock_event(sim: &Simulator) -> Event {
    sim.ir
        .event_statements
        .keys()
        .filter_map(|x| match x {
            Event::Clock(id, elem) => Some((*id, *elem)),
            _ => None,
        })
        .min()
        .map(|(id, elem)| Event::Clock(id, elem))
        .expect("fixture has no clock event to take the reset step on")
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

mod comb_order;
mod comb_rewrite;
mod comb_wide;
mod component;
mod component_sim;
mod dataflow;
mod derived_clock;
mod dut_reuse;
mod error;
mod guarded_cover;
mod hier_ref;
mod simulation;
mod testbench;

mod selection_regressions;

mod ff_payload_regressions;

mod wide_bitwise_regressions;
