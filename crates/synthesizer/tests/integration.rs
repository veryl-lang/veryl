use veryl_analyzer::ir as air;
use veryl_analyzer::{Analyzer, Context, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_parser::resource_table;
use veryl_synthesizer::ir::{CellKind, NetDriver};
use veryl_synthesizer::{build_gate_ir, synthesize};

#[track_caller]
fn analyze(code: &str, top: &str) -> (air::Ir, veryl_parser::resource_table::StrId) {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"test.veryl").expect("parse failed");
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let _ = analyzer.analyze_pass1("prj", &parser.veryl);
    let _ = Analyzer::analyze_post_pass1();
    let mut ir = air::Ir::default();
    let _ = analyzer.analyze_pass2("prj", &parser.veryl, &mut context, Some(&mut ir));
    let _ = Analyzer::analyze_post_pass2();

    (ir, resource_table::insert_str(top))
}

#[test]
fn simple_and_gate() {
    let code = r#"
        module Top (
            a: input  logic,
            b: input  logic,
            y: output logic,
        ) {
            always_comb {
                y = a & b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let and_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    assert!(
        and_count >= 1,
        "expected at least one And2, got {}",
        and_count
    );
    assert_eq!(gate.module.ffs.len(), 0);
    assert_eq!(gate.module.ports.len(), 3);
}

#[test]
fn multibit_or() {
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a | b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let or_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Or2)
        .count();
    assert!(
        or_count >= 4,
        "expected at least four Or2, got {}",
        or_count
    );
}

#[test]
fn simple_dff() {
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            d:   input  logic,
            q:   output logic,
        ) {
            always_ff (clk, rst) {
                if_reset {
                    q = 0;
                } else {
                    q = d;
                }
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    assert_eq!(gate.module.ffs.len(), 1);
    let ff = &gate.module.ffs[0];
    assert!(ff.reset.is_some());
    assert!(!ff.reset_value);
}

#[test]
fn ripple_carry_adder() {
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a + b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    // Full adder: 2x XOR + 2x AND + 1x OR. Four of them for a 4-bit adder
    // → at least 8 XORs.
    let xor_count = result
        .gate_ir
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(xor_count >= 8, "expected >=8 Xor2 gates, got {}", xor_count);
    assert!(result.timing.critical_path_delay > 0.0);
    assert!(result.timing.critical_path_depth >= 4);
    assert!(result.area.total > 0.0);
}

#[test]
fn mux_ternary() {
    let code = r#"
        module Top (
            sel: input  logic,
            a:   input  logic<8>,
            b:   input  logic<8>,
            y:   output logic<8>,
        ) {
            always_comb {
                y = if sel ? a : b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let mux_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Mux2)
        .count();
    assert_eq!(mux_count, 8, "expected 8 Mux2 cells for 8-bit mux");
}

#[test]
fn two_comb_blocks_drive_different_outputs() {
    // Regression: each comb block's finalize must not emit Buf(0) aliases
    // for variables driven by a different comb block.
    let code = r#"
        module Top (
            a:  input  logic,
            b:  input  logic,
            o1: output logic,
            o2: output logic,
        ) {
            always_comb {
                o1 = a & b;
            }
            always_comb {
                o2 = a | b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let and_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    let or_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Or2)
        .count();
    assert_eq!(and_count, 1, "one AND for o1");
    assert_eq!(or_count, 1, "one OR for o2");

    // Walk past Buf aliases to reach the real driver cell.
    let trace_driver_kind = |net_id: veryl_synthesizer::NetId| -> Option<CellKind> {
        let mut cur = net_id;
        loop {
            match &gate.module.nets[cur as usize].driver {
                NetDriver::Cell(idx) => {
                    let cell = &gate.module.cells[*idx];
                    if cell.kind == CellKind::Buf {
                        cur = cell.inputs[0];
                        continue;
                    }
                    return Some(cell.kind);
                }
                _ => return None,
            }
        }
    };

    for port in &gate.module.ports {
        let port_name = format!("{}", port.name);
        if port_name == "o1" {
            assert_eq!(trace_driver_kind(port.nets[0]), Some(CellKind::And2));
        } else if port_name == "o2" {
            assert_eq!(trace_driver_kind(port.nets[0]), Some(CellKind::Or2));
        }
    }
}

#[test]
fn counter_without_reset() {
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            q:   output logic<4>,
        ) {
            always_ff (clk, rst) {
                if_reset {
                    q = 0;
                } else {
                    q = q + 1;
                }
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert_eq!(result.gate_ir.module.ffs.len(), 4);
    for ff in &result.gate_ir.module.ffs {
        assert!(matches!(
            result.gate_ir.module.nets[ff.q as usize].driver,
            NetDriver::FfQ(_)
        ));
    }
    let xor_count = result
        .gate_ir
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(xor_count > 0);
}
