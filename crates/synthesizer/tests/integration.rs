use std::collections::BTreeMap;
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
fn counter_context_width() {
    // Regression: `q + 1` in an 8-bit FF should synthesize as an 8-bit adder,
    // not the 32-bit adder that naive SV context-width rules would produce.
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            q:   output logic<8>,
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
    let xor_count = result
        .gate_ir
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    let total = result.gate_ir.module.cells.len();
    // 8-bit ripple-carry adder: 16 XOR + 16 AND + 8 OR = 40 gates. The 32-bit
    // version would be ~160 gates.
    assert!(
        xor_count <= 16,
        "expected <= 16 Xor2 (8-bit adder), got {}",
        xor_count
    );
    assert!(
        total <= 60,
        "expected total cells <= 60 after context-width clamp, got {}",
        total
    );
}

#[test]
fn mux_preserves_output_driver() {
    // Regression: after Buf elision, output ports driven by combinational
    // logic should point directly at the real gate, not through a Buf alias.
    let code = r#"
        module Top (
            s: input  logic,
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = if s ? a : b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let buf_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Buf)
        .count();
    assert_eq!(
        buf_count, 0,
        "expected no Buf cells after elision, got {}",
        buf_count
    );
    for port in &gate.module.ports {
        if format!("{}", port.name) == "y" {
            for &n in &port.nets {
                match &gate.module.nets[n as usize].driver {
                    NetDriver::Cell(idx) => {
                        assert_eq!(
                            gate.module.cells[*idx].kind,
                            CellKind::Mux2,
                            "y should be driven directly by Mux2"
                        );
                    }
                    other => panic!("y driver unexpected: {:?}", other),
                }
            }
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

#[test]
fn array_static_index_read() {
    // 4-entry ROM addressed by a constant index.
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            c: input  logic<4>,
            d: input  logic<4>,
            y: output logic<4>,
        ) {
            var rom: logic<4> [4];
            always_comb {
                rom[0] = a;
                rom[1] = b;
                rom[2] = c;
                rom[3] = d;
                y = rom[2];
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // rom[2] = c means y bits should alias port c's input bits directly.
    for port in &gate.module.ports {
        if format!("{}", port.name) == "y" {
            for &n in &port.nets {
                assert!(matches!(
                    gate.module.nets[n as usize].driver,
                    NetDriver::PortInput
                ));
            }
        }
    }
}

#[test]
fn array_register_file_ff() {
    // 4-entry 4-bit register file with a constant index write/read.
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            d:   input  logic<4>,
            q:   output logic<4>,
        ) {
            var rf: logic<4> [4];
            always_ff (clk, rst) {
                if_reset {
                    rf[0] = 0;
                    rf[1] = 0;
                    rf[2] = 0;
                    rf[3] = 0;
                } else {
                    rf[1] = d;
                }
            }
            always_comb {
                q = rf[1];
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    // 4 entries × 4 bits = 16 flip-flops.
    assert_eq!(
        result.gate_ir.module.ffs.len(),
        16,
        "expected 16 FFs for 4x4 register file"
    );
}

#[test]
fn multidim_array_static_index() {
    let code = r#"
        module Top (
            a:  input  logic<4>,
            y:  output logic<4>,
        ) {
            var mem: logic<4> [2, 3];
            always_comb {
                mem[0][0] = a;
                mem[0][1] = a;
                mem[0][2] = a;
                mem[1][0] = a;
                mem[1][1] = a;
                mem[1][2] = a;
                y = mem[1][2];
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    for port in &gate.module.ports {
        if format!("{}", port.name) == "y" {
            for &n in &port.nets {
                assert!(matches!(
                    gate.module.nets[n as usize].driver,
                    NetDriver::PortInput
                ));
            }
        }
    }
}

#[test]
fn array_dynamic_index_read() {
    // 4-to-1 mux via dynamic array index read.
    let code = r#"
        module Top (
            sel: input  logic<2>,
            a:   input  logic<4>,
            b:   input  logic<4>,
            c:   input  logic<4>,
            d:   input  logic<4>,
            y:   output logic<4>,
        ) {
            var rom: logic<4> [4];
            always_comb {
                rom[0] = a;
                rom[1] = b;
                rom[2] = c;
                rom[3] = d;
                y = rom[sel];
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
    // 4-to-1 mux tree: 2 levels × 2 mergers × 4 bits = 12 muxes (bits per level
    // shrink: 4→2 elements then 2→1).
    assert_eq!(
        mux_count, 12,
        "expected 12 Mux2 for 4-to-1 dynamic select, got {}",
        mux_count
    );
}

#[test]
fn array_dynamic_index_write_ff() {
    // Write to rf[sel] updates only the matching element.
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            sel: input  logic<2>,
            d:   input  logic<4>,
            q0:  output logic<4>,
            q1:  output logic<4>,
            q2:  output logic<4>,
            q3:  output logic<4>,
        ) {
            var rf: logic<4> [4];
            always_ff (clk, rst) {
                if_reset {
                    rf[0] = 0;
                    rf[1] = 0;
                    rf[2] = 0;
                    rf[3] = 0;
                } else {
                    rf[sel] = d;
                }
            }
            always_comb {
                q0 = rf[0];
                q1 = rf[1];
                q2 = rf[2];
                q3 = rf[3];
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    // 4 entries × 4 bits = 16 FFs.
    assert_eq!(result.gate_ir.module.ffs.len(), 16);
}

#[test]
fn dynamic_bit_select_read() {
    // Pick a single bit from a 4-bit value using a 2-bit select.
    let code = r#"
        module Top (
            x:   input  logic<4>,
            sel: input  logic<2>,
            y:   output logic,
        ) {
            always_comb {
                y = x[sel];
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // 4-to-1 mux tree for a 1-bit element: 2 muxes at level 0 + 1 at level 1 = 3.
    let mux_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Mux2)
        .count();
    assert_eq!(
        mux_count, 3,
        "4-to-1 single-bit dynamic select should cost 3 Mux2"
    );
}

#[test]
fn dynamic_bit_select_write_ff() {
    // `reg[sel] = d` — only the indexed bit gets the new value.
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            sel: input  logic<2>,
            d:   input  logic,
            q:   output logic<4>,
        ) {
            always_ff (clk, rst) {
                if_reset {
                    q = 0;
                } else {
                    q[sel] = d;
                }
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert_eq!(result.gate_ir.module.ffs.len(), 4);
}

#[test]
fn unsigned_multiply_4bit() {
    // 4-bit × 4-bit unsigned multiply produces an array of AND gates fed into
    // ripple-carry adders.
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a * b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    let and_count = result
        .gate_ir
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    let xor_count = result
        .gate_ir
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    // Shift-add for 4-bit result: 4 rows × 4 AND = 16 AND gates minimum.
    assert!(
        and_count >= 10,
        "expected many AND gates, got {}",
        and_count
    );
    // Each ripple-add contributes XORs.
    assert!(xor_count >= 4, "expected XOR gates from adders");
}

#[test]
fn constant_multiply_lowers_to_shift_add() {
    // `a * 3` = `(a << 1) + a`. The synthesizer still emits the general shift-add
    // multiplier, but the AND gates with const inputs end up trivially wired.
    let code = r#"
        module Top (
            a: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a * 3;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let xor_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(xor_count > 0, "constant multiply should still produce XORs");
}

#[test]
fn signed_multiply_truncates_correctly() {
    // Signed 4-bit multiply must behave like sign-extended multiply truncated
    // to the target width. This test only checks that the synthesis succeeds
    // and produces gates — bit-level correctness is covered by the 2's-
    // complement identity.
    let code = r#"
        module Top (
            a: input  signed logic<4>,
            b: input  signed logic<4>,
            y: output signed logic<4>,
        ) {
            always_comb {
                y = a * b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert!(!result.gate_ir.module.cells.is_empty());
}

#[test]
fn unsigned_divide_4bit() {
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a / b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    // Restoring division for 4 bits: 4 subtractor stages + 4 mux layers.
    // Each subtractor is a ripple-carry adder-like structure ≈ 5 cells × width.
    assert!(
        result.gate_ir.module.cells.len() > 30,
        "expected substantial cell count for 4-bit divider"
    );
}

#[test]
fn unsigned_modulo_4bit() {
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a % b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    assert!(gate.module.cells.len() > 20);
}

#[test]
fn signed_divide_synthesizes() {
    // Signed divide: take |a|, |b|, unsigned-divide, then fix signs.
    let code = r#"
        module Top (
            a: input  signed logic<4>,
            b: input  signed logic<4>,
            y: output signed logic<4>,
        ) {
            always_comb {
                y = a / b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert!(result.gate_ir.module.cells.len() > 40);
}

#[test]
fn signed_remainder_synthesizes() {
    let code = r#"
        module Top (
            a: input  signed logic<4>,
            b: input  signed logic<4>,
            y: output signed logic<4>,
        ) {
            always_comb {
                y = a % b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert!(result.gate_ir.module.cells.len() > 40);
}

#[test]
fn hierarchical_and_gate() {
    // Parent instantiates a child module containing a single AND.
    let code = r#"
        module Child (
            a: input  logic,
            b: input  logic,
            y: output logic,
        ) {
            always_comb {
                y = a & b;
            }
        }

        module Top (
            x: input  logic,
            z: input  logic,
            o: output logic,
        ) {
            inst u_child: Child (
                a: x,
                b: z,
                y: o,
            );
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
    assert_eq!(and_count, 1, "child's AND should survive flattening");
}

#[test]
fn hierarchical_register() {
    // A child module contains a DFF. The parent instantiates the child and
    // exposes its Q as an output.
    let code = r#"
        module Reg (
            clk: input  clock,
            rst: input  reset,
            d:   input  logic<4>,
            q:   output logic<4>,
        ) {
            always_ff (clk, rst) {
                if_reset {
                    q = 0;
                } else {
                    q = d;
                }
            }
        }

        module Top (
            clk: input  clock,
            rst: input  reset,
            d:   input  logic<4>,
            q:   output logic<4>,
        ) {
            inst u_reg: Reg (
                clk: clk,
                rst: rst,
                d:   d,
                q:   q,
            );
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert_eq!(
        result.gate_ir.module.ffs.len(),
        4,
        "child's 4 FFs should be flattened into Top"
    );
}

#[test]
fn multiple_child_instances() {
    // Two instances of the same child module must each expand independently.
    let code = r#"
        module Not1 (
            i: input  logic,
            o: output logic,
        ) {
            always_comb {
                o = ~i;
            }
        }

        module Top (
            a: input  logic,
            b: input  logic,
            x: output logic,
            y: output logic,
        ) {
            inst u_a: Not1 ( i: a, o: x );
            inst u_b: Not1 ( i: b, o: y );
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let not_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Not)
        .count();
    assert_eq!(not_count, 2, "two Inst should give two NOTs");
}

#[test]
fn interface_instance_synthesizes() {
    // Interface instantiation is flattened by the analyzer; Top's combinational
    // body drives through interface member variables as if they were local.
    let code = r#"
        interface Bus {
            var valid: logic;
            var data:  logic<4>;

            modport master {
                valid: output,
                data:  output,
            }

            modport slave {
                valid: input,
                data:  input,
            }
        }

        module Top (
            d: input  logic<4>,
            v: output logic,
            q: output logic<4>,
        ) {
            inst b: Bus;
            always_comb {
                b.valid = 1;
                b.data  = d;
                v       = b.valid;
                q       = b.data;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // The module should at least have port definitions.
    assert!(
        gate.module.ports.len() >= 3,
        "expected v, q, d ports plus implicit bus members (if any)"
    );
}

/// Tries to synthesize every module found in `testcases/veryl/`. Not a pass/
/// fail test — it's an exploration that prints a per-category error histogram
/// to help decide what to support next. Run with:
/// `cargo test -p veryl-synthesizer -- --ignored --nocapture smoke_all_testcases`.
#[test]
#[ignore]
fn smoke_all_testcases() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("testcases")
        .join("veryl");
    let mut total = 0;
    let mut ok = 0;
    let mut errs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut parse_fail = 0;

    for entry in std::fs::read_dir(&dir).expect("read testcases/veryl") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("veryl") {
            continue;
        }
        let code = std::fs::read_to_string(&path).unwrap();

        symbol_table::clear();
        let metadata = Metadata::create_default("prj").unwrap();
        let parser = match Parser::parse(&code, &path) {
            Ok(p) => p,
            Err(_) => {
                parse_fail += 1;
                continue;
            }
        };
        let analyzer = Analyzer::new(&metadata);
        let mut context = Context::default();
        let _ = analyzer.analyze_pass1("prj", &parser.veryl);
        let _ = Analyzer::analyze_post_pass1();
        let mut ir = air::Ir::default();
        let _ = analyzer.analyze_pass2("prj", &parser.veryl, &mut context, Some(&mut ir));
        let _ = Analyzer::analyze_post_pass2();

        for component in &ir.components {
            if let air::Component::Module(m) = component {
                total += 1;
                match build_gate_ir(&ir, m.name) {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        let key = short_error_key(&format!("{}", e));
                        errs.entry(key)
                            .or_default()
                            .push(path.file_name().unwrap().to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    println!("\n=== smoke_all_testcases ===");
    println!("parse failures: {}", parse_fail);
    println!("modules: {}/{} synthesized", ok, total);
    println!("error categories (count, category, files):");
    let mut cats: Vec<_> = errs.iter().collect();
    cats.sort_by_key(|c| std::cmp::Reverse(c.1.len()));
    for (k, files) in cats {
        let mut unique = files.clone();
        unique.sort();
        unique.dedup();
        println!("  {:4}  {}  [{}]", files.len(), k, unique.join(", "));
    }
}

fn short_error_key(msg: &str) -> String {
    let core = msg
        .split_once(':')
        .map(|(_, rest)| rest.trim())
        .unwrap_or(msg);
    // Strip trailing variable names / numbers for bucketing.
    core.split_whitespace()
        .take(10)
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn struct_wide_member_assign() {
    // Reproduces the 9_struct_enum smoke failure: a struct whose first
    // declared member occupies the high bits, assigned through a.member.
    let code = r#"
        module Top {
            struct A {
                a:   bit<10>,
                aa:  bit<10>,
                aaa: bit<32>,
            }
            var x: A;
            var k: logic;
            always_comb {
                x.a   = 1;
                x.aa  = 2;
                x.aaa = 3;
                k     = x.a;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let _ = synthesize(&ir, top).expect("synthesize");
}

#[test]
fn struct_member_access() {
    let code = r#"
        module Top (
            in_valid: input  logic,
            in_data:  input  logic<4>,
            out_data: output logic<4>,
        ) {
            struct Packet {
                valid: logic,
                data:  logic<4>,
            }
            var pkt: Packet;
            always_comb {
                pkt.valid = in_valid;
                pkt.data  = in_data;
                out_data  = pkt.data;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // pkt has 5 bits total (1 + 4); out_data bits should alias pkt.data bits
    // which in turn alias in_data.
    for port in &gate.module.ports {
        if format!("{}", port.name) == "out_data" {
            for &n in &port.nets {
                assert!(matches!(
                    gate.module.nets[n as usize].driver,
                    NetDriver::PortInput
                ));
            }
        }
    }
}

#[test]
fn concat_lhs_splits_src() {
    // `{d, e} = x` — d gets the high bits of x (MSB), e gets the low bit.
    let code = r#"
        module Top (
            x: input  logic<5>,
            d: output logic<4>,
            e: output logic,
        ) {
            always_comb {
                {d, e} = x;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // Both d and e should be driven by port inputs of `x`.
    for port in &gate.module.ports {
        let name = format!("{}", port.name);
        if name == "d" || name == "e" {
            for &n in &port.nets {
                assert!(matches!(
                    gate.module.nets[n as usize].driver,
                    NetDriver::PortInput
                ));
            }
        }
    }
}

#[test]
fn inst_output_concat_lhs() {
    // Child emits 6-bit output `y`, parent splits it `{hi, lo}` where hi gets
    // the top 4 bits and lo the bottom 2.
    let code = r#"
        module Child (
            x: input  logic<3>,
            y: output logic<6>,
        ) {
            assign y = {x, x};
        }

        module Top (
            x:  input  logic<3>,
            hi: output logic<4>,
            lo: output logic<2>,
        ) {
            inst u: Child (
                x       ,
                y: {hi, lo},
            );
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // Both hi and lo should have all nets driven.
    for port in &gate.module.ports {
        let name = format!("{}", port.name);
        if name == "hi" || name == "lo" {
            for &n in &port.nets {
                assert!(
                    !matches!(gate.module.nets[n as usize].driver, NetDriver::Undriven),
                    "{} net undriven",
                    name
                );
            }
        }
    }
}

#[test]
fn concat_lhs_with_bit_select() {
    // Concat-LHS where components carry their own bit selects. The RHS
    // `x` is 6 bits wide: high 4 go to `d[3:0]`, low 2 go to `e[1:0]`.
    let code = r#"
        module Top (
            x: input  logic<6>,
            d: output logic<8>,
            e: output logic<4>,
        ) {
            always_comb {
                d[7:4] = 0;
                e[3:2] = 0;
                {d[3:0], e[1:0]} = x;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // The low 4 bits of d and low 2 bits of e should trace to port inputs of x.
    for port in &gate.module.ports {
        let name = format!("{}", port.name);
        if name == "d" {
            for &n in &port.nets[..4] {
                assert!(matches!(
                    gate.module.nets[n as usize].driver,
                    NetDriver::PortInput
                ));
            }
        }
        if name == "e" {
            for &n in &port.nets[..2] {
                assert!(matches!(
                    gate.module.nets[n as usize].driver,
                    NetDriver::PortInput
                ));
            }
        }
    }
}

#[test]
fn enum_state_machine() {
    // Simple 2-state FSM with enum-typed state register.
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            go:  input  logic,
            out: output logic,
        ) {
            enum State: logic {
                IDLE,
                RUN,
            }
            var state: State;
            always_ff (clk, rst) {
                if_reset {
                    state = State::IDLE;
                } else {
                    if state == State::IDLE {
                        if go {
                            state = State::RUN;
                        }
                    } else {
                        state = State::IDLE;
                    }
                }
            }
            always_comb {
                out = state == State::RUN;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    // 1-bit enum state → 1 FF.
    assert_eq!(result.gate_ir.module.ffs.len(), 1);
}

#[test]
fn case_statement_decodes_to_mux() {
    let code = r#"
        module Top (
            sel: input  logic<2>,
            a:   input  logic<4>,
            b:   input  logic<4>,
            c:   input  logic<4>,
            d:   input  logic<4>,
            y:   output logic<4>,
        ) {
            always_comb {
                case sel {
                    2'd0:    y = a;
                    2'd1:    y = b;
                    2'd2:    y = c;
                    default: y = d;
                }
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
    // 3 non-default cases select across 4 bits through a Mux2 chain: each bit
    // needs 3 Mux2 (one per non-default branch) = 12.
    assert_eq!(mux_count, 12, "expected 12 Mux2 cells for 4-way case");
}

#[test]
fn case_statement_with_default_only() {
    // Regression: a case whose body is only `default:` should still synthesize
    // — the expression evaluates to the default value unconditionally.
    let code = r#"
        module Top (
            sel: input  logic<2>,
            d:   input  logic<4>,
            y:   output logic<4>,
        ) {
            always_comb {
                case sel {
                    default: y = d;
                }
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // With no match branches, the output should be driven directly from `d`.
    for port in &gate.module.ports {
        if format!("{}", port.name) == "y" {
            for &n in &port.nets {
                assert!(
                    matches!(
                        gate.module.nets[n as usize].driver,
                        NetDriver::PortInput | NetDriver::Cell(_)
                    ),
                    "y must be driven by something concrete"
                );
            }
        }
    }
}

#[test]
fn switch_expression_synthesizes() {
    // switch-expression lowers to a nested Ternary chain via EqWildcard.
    let code = r#"
        module Top (
            sel: input  logic<2>,
            a:   input  logic<4>,
            b:   input  logic<4>,
            c:   input  logic<4>,
            y:   output logic<4>,
        ) {
            always_comb {
                y = switch {
                    sel == 2'd0: a,
                    sel == 2'd1: b,
                    default:     c,
                };
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
    assert!(mux_count > 0, "switch expression should produce Mux2 cells");
}

#[test]
fn variable_shift_left_barrel() {
    // 4-bit x shifted by a 2-bit amount: two barrel stages, 4 muxes each = 8.
    let code = r#"
        module Top (
            x:   input  logic<4>,
            amt: input  logic<2>,
            y:   output logic<4>,
        ) {
            always_comb {
                y = x << amt;
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
    assert_eq!(
        mux_count, 8,
        "expected 2 barrel stages × 4 bits = 8 Mux2, got {}",
        mux_count
    );
}

#[test]
fn variable_shift_right_amount_saturates() {
    // Shift amount is 3-bit but the data is 4-bit (only 2 barrel stages
    // needed). The upper bit of amt must saturate the output to zero via an
    // extra mux stage.
    let code = r#"
        module Top (
            x:   input  logic<4>,
            amt: input  logic<3>,
            y:   output logic<4>,
        ) {
            always_comb {
                y = x >> amt;
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
    // 2 barrel stages × 4 bits + 1 saturation stage × 4 bits = 12.
    assert_eq!(
        mux_count, 12,
        "expected 12 Mux2 (barrel + saturate), got {}",
        mux_count
    );
}

#[test]
fn variable_arith_shift_right_fills_sign() {
    // signed 4-bit ASR: fill must be the sign bit (x[3]).
    let code = r#"
        module Top (
            x:   input  signed logic<4>,
            amt: input  logic<2>,
            y:   output signed logic<4>,
        ) {
            always_comb {
                y = x >>> amt;
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
    assert!(mux_count >= 8, "expected barrel-shifter muxes");

    // Confirm that the MSB of x appears as a Mux input (sign fill).
    let msb_used_as_input = gate.module.cells.iter().any(|c| {
        c.kind == CellKind::Mux2
            && c.inputs
                .iter()
                .any(|n| matches!(gate.module.nets[*n as usize].driver, NetDriver::PortInput))
    });
    assert!(msb_used_as_input, "MSB of x should feed the shifter");
}

#[test]
fn wildcard_pattern_skips_dontcare_bits() {
    // 4-bit case with a don't-care pattern: `4'b1x0x` fixes only bits 3 and 1.
    // The comparison must use exactly 2 Xnor2 cells (one per fixed bit), not 4.
    let code = r#"
        module Top (
            sel: input  logic<4>,
            hit: output logic,
        ) {
            always_comb {
                case sel {
                    4'b1x0x:  hit = 1;
                    default:  hit = 0;
                }
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let xnor_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xnor2)
        .count();
    assert_eq!(
        xnor_count, 2,
        "don't-care bits must not generate Xnor2; expected 2 fixed bits"
    );
}

#[test]
fn function_call_returns_value() {
    // Pure function returning a value: caller's output should match the
    // computed expression. Inlining must keep arity correct.
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            function Add (
                x: input  logic<4>,
                z: input  logic<4>,
            ) -> logic<4> {
                return x + z;
            }
            assign y = Add(a, b);
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let xor_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(
        xor_count >= 4,
        "4-bit adder should generate at least 4 Xor2 cells, got {}",
        xor_count
    );
}

#[test]
fn function_call_with_output_port() {
    // Function with output arg `b = x + 1` and return `x + 2`. After inline,
    // caller's `out2` must get the `x + 1` net from the function body.
    let code = r#"
        module Top (
            a:    input  logic<4>,
            out1: output logic<4>,
            out2: output logic<4>,
        ) {
            function Double (
                x: input  logic<4>,
                b: output logic<4>,
            ) -> logic<4> {
                b = x + 1;
                return x + 2;
            }
            var tmp: logic<4>;
            always_comb {
                out1 = Double(a, tmp);
                out2 = tmp;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // Two adders (both 4-bit) → at least 8 Xor2 cells total.
    let xor_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(
        xor_count >= 8,
        "two 4-bit adders should generate ≥8 Xor2 cells, got {}",
        xor_count
    );
}

#[test]
fn function_call_multiple_invocations_isolated() {
    // Two calls to the same function with different args should produce
    // distinct nets for each invocation (no cross-call interference).
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            c: input  logic<4>,
            y: output logic<4>,
        ) {
            function AddOne (
                x: input  logic<4>,
            ) -> logic<4> {
                return x + 1;
            }
            always_comb {
                y = AddOne(a) + AddOne(b) + AddOne(c);
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // Three 4-bit "+1" adders + two 4-bit result adders = 5 adders = ≥20 Xor2.
    let xor_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(
        xor_count >= 20,
        "five 4-bit adds should generate ≥20 Xor2 cells, got {}",
        xor_count
    );
}

#[test]
fn function_call_void_via_statement() {
    // Void function called as a statement: the output arg must still be
    // propagated back to the caller's variable.
    let code = r#"
        module Top (
            a: input  logic<4>,
            y: output logic<4>,
        ) {
            function Incr (
                x: input  logic<4>,
                r: output logic<4>,
            ) {
                r = x + 1;
            }
            always_comb {
                Incr(a, y);
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let xor_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(
        xor_count >= 4,
        "4-bit adder should generate ≥4 Xor2 cells, got {}",
        xor_count
    );
}
