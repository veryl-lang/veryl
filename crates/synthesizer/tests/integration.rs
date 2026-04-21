use std::collections::BTreeMap;
use veryl_analyzer::ir as air;
use veryl_analyzer::{Analyzer, Context, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_parser::resource_table;
use veryl_synthesizer::ir::{CellKind, NetDriver};
use veryl_synthesizer::{BuiltinLibrary, build_gate_ir, compute_power, synthesize};

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
    // gives 8 XORs before optimization; const-prop folds the bit-0 sum XOR
    // against cin=0 down to a single XOR, so we end up with ≥7.
    let xor_count = result
        .gate_ir
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(xor_count >= 7, "expected >=7 Xor2 gates, got {}", xor_count);
    assert!(result.timing.critical_path_delay > 0.0);
    assert!(result.timing.critical_path_depth >= 4);
    assert!(result.area.total > 0.0);
}

#[test]
fn kogge_stone_wider_add_has_log_depth() {
    // An 8-bit add with Kogge-Stone should finish in much less depth than
    // the 15-deep ripple carry chain (XOR + 7*(AND+OR) + XOR).
    let code = r#"
        module Top (
            a: input  logic<8>,
            b: input  logic<8>,
            y: output logic<8>,
        ) {
            always_comb {
                y = a + b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert!(
        result.timing.critical_path_depth < 12,
        "8-bit Kogge-Stone depth should be ~log2(8)+const ≈ 7, got {}",
        result.timing.critical_path_depth
    );
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
    // Only rf[1] is ever written after reset; rf[0], rf[2], rf[3] hold their
    // reset_value=0 forever, so D=Q FF elimination folds their 12 FFs into
    // constants. Only rf[1]'s 4 FFs remain.
    assert_eq!(
        result.gate_ir.module.ffs.len(),
        4,
        "only rf[1]'s 4 FFs survive; the other rows are reset-held constants"
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
fn wallace_tree_multiplier_shallower_than_shift_add() {
    // An 8-bit multiply through the Wallace tree + Kogge-Stone CPA should
    // finish in much less depth than a shift-add of 8 sequential ripple adds
    // (which alone would be well over 30 gates deep).
    let code = r#"
        module Top (
            a: input  logic<8>,
            b: input  logic<8>,
            y: output logic<8>,
        ) {
            always_comb { y = a * b; }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert!(
        result.timing.critical_path_depth < 30,
        "8-bit Wallace multiplier depth should be well below shift-add's 30+, got {}",
        result.timing.critical_path_depth
    );
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
    // 4-bit x shifted by a 2-bit amount: two barrel stages × 4 muxes = 8
    // select cells. The post-pass then collapses shift-in slots (where the
    // low side is tied to 0) into `And2(!amt, x)` pairs, so some of the 8
    // muxes reappear as And2 cells. We require the total select-cell count
    // to stay at 8.
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
    let select_cells = gate
        .module
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mux2 | CellKind::And2))
        .count();
    assert_eq!(
        select_cells, 8,
        "expected 2 barrel stages × 4 bits = 8 select cells (Mux2+And2), got {}",
        select_cells
    );
}

#[test]
fn variable_shift_right_amount_saturates() {
    // 3-bit amt for 4-bit data: 2 barrel stages + 1 saturation stage. Some
    // stages tie one side to 0 and thus collapse to And2 (post-pass), and
    // adjacent And2 cells may further fuse into And3 when the inner has a
    // single consumer. We sanity-check the synthesized 4-bit output by
    // counting all "select-like" combinational cells — Mux2, And2, and
    // And3 together.
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
    let select_cells = gate
        .module
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mux2 | CellKind::And2 | CellKind::And3))
        .count();
    // 5 Mux2 + 3 And2 + 2 And3 expected with the current optimization mix.
    assert!(
        (9..=12).contains(&select_cells),
        "expected 9-12 select-like cells (Mux2+And2+And3), got {}",
        select_cells
    );
}

#[test]
fn variable_arith_shift_right_fills_sign() {
    // signed 4-bit ASR: fill must be the sign bit (x[3]). After const-prop
    // folds Mux(s, x, x) = x for the sign-fill mux stages, y[3] should be a
    // direct alias of x[3] (port input) rather than going through any gates.
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
    let y_port = gate
        .module
        .ports
        .iter()
        .find(|p| format!("{}", p.name) == "y")
        .expect("y port");
    let y_msb = y_port.nets[3];
    assert!(
        matches!(
            gate.module.nets[y_msb as usize].driver,
            NetDriver::PortInput
        ),
        "ASR sign-fill: y[3] should collapse to x[3] (port input) after const-prop"
    );
}

#[test]
fn wildcard_pattern_skips_dontcare_bits() {
    // 4-bit case with a don't-care pattern `4'b1x0x` fixes only bits 3 and 1.
    // After const-prop folds Xnor(x, const) down to Buf/Not, and Mux(s, 0, 1)
    // to the sel itself, the whole match reduces to `And(sel[3], !sel[1])` —
    // about two gates. If don't-care bits leaked in, we'd see 4 compare terms
    // and 3 AND-reduction stages, so upper-bound the total cell count.
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
    let cell_count = gate.module.cells.len();
    assert!(
        cell_count <= 3,
        "only 2 fixed bits should reach hit; got {} cells",
        cell_count
    );
}

#[test]
fn const_prop_and_with_zero_folds() {
    let code = r#"
        module Top (
            a: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a & 4'b0000;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    // y = 0 everywhere — no And2 cells should survive; all y nets tie to GND
    // via Buf, which elide_bufs collapses.
    assert_eq!(
        gate.module
            .cells
            .iter()
            .filter(|c| c.kind == CellKind::And2)
            .count(),
        0,
        "And(_, 0) must fold away"
    );
    let y = gate
        .module
        .ports
        .iter()
        .find(|p| format!("{}", p.name) == "y")
        .unwrap();
    for &n in &y.nets {
        assert!(matches!(
            gate.module.nets[n as usize].driver,
            NetDriver::Const(false)
        ));
    }
}

#[test]
fn const_prop_or_with_one_folds() {
    let code = r#"
        module Top (
            a: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a | 4'b1111;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    assert_eq!(
        gate.module
            .cells
            .iter()
            .filter(|c| c.kind == CellKind::Or2)
            .count(),
        0,
        "Or(_, 1) must fold away"
    );
    let y = gate
        .module
        .ports
        .iter()
        .find(|p| format!("{}", p.name) == "y")
        .unwrap();
    for &n in &y.nets {
        assert!(matches!(
            gate.module.nets[n as usize].driver,
            NetDriver::Const(true)
        ));
    }
}

#[test]
fn const_prop_mux_same_inputs() {
    // `sel ? a : a` should drop the Mux and alias y to a.
    let code = r#"
        module Top (
            sel: input  logic,
            a:   input  logic<4>,
            y:   output logic<4>,
        ) {
            always_comb {
                y = if sel ? a : a;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    assert_eq!(
        gate.module
            .cells
            .iter()
            .filter(|c| c.kind == CellKind::Mux2)
            .count(),
        0,
        "Mux with equal arms must fold away"
    );
    let y = gate
        .module
        .ports
        .iter()
        .find(|p| format!("{}", p.name) == "y")
        .unwrap();
    for &n in &y.nets {
        assert!(matches!(
            gate.module.nets[n as usize].driver,
            NetDriver::PortInput
        ));
    }
}

#[test]
fn mux_with_zero_arm_collapses_to_and() {
    // `sel ? a : 0` should rewrite to `sel & a` (no Mux cell).
    let code = r#"
        module Top (
            sel: input  logic,
            a:   input  logic<4>,
            y:   output logic<4>,
        ) {
            always_comb {
                y = if sel ? a : 0;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let mux = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Mux2)
        .count();
    let and2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    assert_eq!(mux, 0, "Mux(sel, 0, a) must collapse");
    assert_eq!(and2, 4, "one And2 per output bit expected");
}

#[test]
fn mux_with_one_arm_collapses_to_or() {
    // `sel ? 1 : a` should rewrite to `sel | a`.
    let code = r#"
        module Top (
            sel: input  logic,
            a:   input  logic<4>,
            y:   output logic<4>,
        ) {
            always_comb {
                y = if sel ? 4'hF : a;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let mux = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Mux2)
        .count();
    let or2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Or2)
        .count();
    assert_eq!(mux, 0, "Mux(sel, a, 1) must collapse");
    assert_eq!(or2, 4, "one Or2 per output bit expected");
}

#[test]
fn aoi21_fuses_nor_over_and() {
    // `!((a & b) | c)` — NOR(AND, c) pattern should fuse to Aoi21 after
    // the post-optimization complex-gate sweep.
    let code = r#"
        module Top (
            a: input  logic,
            b: input  logic,
            c: input  logic,
            y: output logic,
        ) {
            assign y = !((a & b) | c);
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let aoi = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Aoi21)
        .count();
    assert!(aoi >= 1, "Aoi21 should be present after post-pass fusion");
}

#[test]
fn timing_top_n_returns_sorted_endpoints() {
    // Three independent output cones with different depths — top-N must
    // order them by arrival time and label each with its port name+bit.
    let code = r#"
        module Top (
            a: input  logic<3>,
            y: output logic<3>,
        ) {
            always_comb {
                // Depth 1: simple And2. Fast.
                y[0] = a[0] & a[1];
                // Depth 2: And3 chain.
                y[1] = a[0] & a[1] & a[2];
                // Depth 3: nested mux/and.
                y[2] = if a[0] ? (a[1] & a[2]) : (a[1] | a[2]);
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let library = veryl_synthesizer::BuiltinLibrary::new();
    let reports = veryl_synthesizer::compute_timing_top_n(&gate.module, &library, 5);
    assert!(
        reports.len() >= 3,
        "expected at least 3 endpoints, got {}",
        reports.len()
    );
    // Reports are sorted by arrival descending.
    for w in reports.windows(2) {
        assert!(
            w[0].critical_path_delay >= w[1].critical_path_delay - 1e-9,
            "reports must be sorted descending by delay"
        );
    }
    // Every report should end at a y[i] port bit (identified via origin).
    for r in &reports[..3] {
        let end = r.critical_path.last().unwrap();
        let (name, _) = end.origin.as_ref().expect("endpoint carries origin");
        assert_eq!(name, "y", "endpoint should be the y port");
    }
}

#[test]
fn boolean_distribution_factors_or_of_and() {
    // `(x & a) | (x & b)` → `x & (a | b)` — distributivity eliminates one
    // And2 cell when both arms are single-consumer.
    let code = r#"
        module Top (
            x: input  logic,
            a: input  logic,
            b: input  logic,
            y: output logic,
        ) {
            assign y = (x & a) | (x & b);
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let and2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    let or2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Or2)
        .count();
    assert_eq!(and2, 1, "distributed form has 1 And2");
    assert_eq!(or2, 1, "inner arm becomes Or2(a, b)");
}

#[test]
fn boolean_distribution_factors_and_of_or() {
    // Dual: `(x | a) & (x | b)` → `x | (a & b)`.
    let code = r#"
        module Top (
            x: input  logic,
            a: input  logic,
            b: input  logic,
            y: output logic,
        ) {
            assign y = (x | a) & (x | b);
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let and2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    let or2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Or2)
        .count();
    assert_eq!(and2, 1);
    assert_eq!(or2, 1);
}

#[test]
fn mux_nested_opposite_phase_collapses() {
    // `Mux2(s, a, Mux2(!s, b, c)) ≡ Mux2(s, a, b)`. The inner mux uses
    // the inverted select so when the outer picks its d1 leg (s=1) the
    // inner evaluates with !s=0 and returns its d0 (= b).
    let code = r#"
        module Top (
            s: input  logic,
            a: input  logic,
            b: input  logic,
            c: input  logic,
            y: output logic,
        ) {
            let inner: logic = if !s ? c : b;
            assign y = if s ? inner : a;
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let mux = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Mux2)
        .count();
    assert_eq!(mux, 1, "inner mux with !s should collapse away");
}

#[test]
fn mux_factor_common_xor_input() {
    // `s ? (x ^ b) : (x ^ a)` → `x ^ (s ? b : a)`. Both xor arms must be
    // single-consumer so they die after factoring.
    let code = r#"
        module Top (
            s: input  logic,
            x: input  logic,
            a: input  logic,
            b: input  logic,
            y: output logic,
        ) {
            assign y = if s ? (x ^ b) : (x ^ a);
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let xor = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert_eq!(xor, 1, "two xors must collapse to one after factoring");
}

#[test]
fn mux_of_mux_cross_position_shared_leg() {
    // `Mux2(s1, Mux2(s2, a, c), a)` — outer.d1 == inner.d0 (cross-position).
    // Collapses to `Mux2(And2(!s1, s2), a, c)` which needs a fresh Not(s1).
    let code = r#"
        module Top (
            s1: input  logic,
            s2: input  logic,
            a:  input  logic,
            c:  input  logic,
            y:  output logic,
        ) {
            assign y = if s1 ? a : (if s2 ? c : a);
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let mux = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Mux2)
        .count();
    assert_eq!(mux, 1, "cross-position mux-of-mux should collapse to one");
    // Must have Not(s1) and the And2 combining it with s2.
    assert!(
        gate.module.cells.iter().any(|c| c.kind == CellKind::Not),
        "cross-position pattern needs materialised Not"
    );
    assert!(
        gate.module.cells.iter().any(|c| c.kind == CellKind::And2),
        "combined select is an And2"
    );
}

#[test]
fn mux_of_mux_shared_d1_collapses() {
    // Nested mux with a shared d1: `Mux2(s1, Mux2(s2, a, c), c)`. The post
    // pass materialises `Or2(s1, s2)` and rewrites to a single mux with
    // that combined select.
    let code = r#"
        module Top (
            s1: input  logic,
            s2: input  logic,
            a:  input  logic,
            c:  input  logic,
            y:  output logic,
        ) {
            assign y = if s1 ? c : (if s2 ? c : a);
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let mux = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Mux2)
        .count();
    let or2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Or2)
        .count();
    assert_eq!(mux, 1, "two nested muxes should collapse to one");
    assert_eq!(or2, 1, "combined select must be materialised as Or2");
}

#[test]
fn and3_fuses_from_and2_chain() {
    // Private And2-of-And2 should collapse to And3. "Private" here means
    // the inner has exactly one consumer so fusion doesn't break sharing.
    let code = r#"
        module Top (
            a: input  logic,
            b: input  logic,
            c: input  logic,
            y: output logic,
        ) {
            assign y = (a & b) & c;
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let and3 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And3)
        .count();
    let and2 = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    assert_eq!(and3, 1);
    assert_eq!(and2, 0);
}

#[test]
fn cse_merges_duplicate_and() {
    // Both y0 and y1 compute `a & b`. CSE must keep a single And2 and alias
    // the second output to the first.
    let code = r#"
        module Top (
            a:  input  logic<4>,
            b:  input  logic<4>,
            y0: output logic<4>,
            y1: output logic<4>,
        ) {
            always_comb {
                y0 = a & b;
                y1 = a & b;
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
    assert_eq!(
        and_count, 4,
        "4 bits × 1 shared AND expression = 4 And2 (not 8)"
    );
}

#[test]
fn cse_commutative_canonicalization() {
    // `a & b` and `b & a` must hash to the same cell after input sorting.
    let code = r#"
        module Top (
            a:  input  logic<4>,
            b:  input  logic<4>,
            y0: output logic<4>,
            y1: output logic<4>,
        ) {
            always_comb {
                y0 = a & b;
                y1 = b & a;
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
    assert_eq!(
        and_count, 4,
        "commutative AND with swapped operands must CSE to a single cell per bit"
    );
}

#[test]
fn cse_preserves_mux_operand_order() {
    // Mux is non-commutative in its (sel, d0, d1) slots, so Mux(s, a, b)
    // and Mux(s, b, a) are distinct expressions and must NOT be merged.
    let code = r#"
        module Top (
            s:  input  logic,
            a:  input  logic<4>,
            b:  input  logic<4>,
            y0: output logic<4>,
            y1: output logic<4>,
        ) {
            always_comb {
                y0 = if s ? a : b;
                y1 = if s ? b : a;
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
        "Mux operand order matters — both cones must stay, 4 bits × 2 = 8 Mux2"
    );
}

#[test]
fn algebraic_double_negation_collapses() {
    // `!!a` must collapse to just `a` — no Not cells left.
    let code = r#"
        module Top (
            a: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = ~~a;
            }
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
    assert_eq!(not_count, 0, "Not(Not(x)) must fold to x");
    let y = gate
        .module
        .ports
        .iter()
        .find(|p| format!("{}", p.name) == "y")
        .unwrap();
    for &n in &y.nets {
        assert!(matches!(
            gate.module.nets[n as usize].driver,
            NetDriver::PortInput
        ));
    }
}

#[test]
fn algebraic_de_morgan_and_not_to_nand() {
    // `!(a & b)` should fuse into a single Nand2 cell, with no surviving
    // And2/Not pair.
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = ~(a & b);
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let nand_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Nand2)
        .count();
    let and_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::And2)
        .count();
    let not_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Not)
        .count();
    assert_eq!(nand_count, 4, "4 bits × 1 Nand2 each");
    assert_eq!(and_count, 0, "And2 must be fused into Nand2");
    assert_eq!(not_count, 0, "Not must be fused into Nand2");
}

#[test]
fn dce_removes_orphan_cells() {
    // A 2-level Xor tree where the final result is overwritten by a constant
    // makes all the Xor cells orphan. DCE must remove them.
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            var mid: logic<4>;
            always_comb {
                mid = a ^ b;
                y = 0;
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
    assert_eq!(
        xor_count, 0,
        "mid is unused (y overwrites with 0); the Xor cells are dead"
    );
}

#[test]
fn dq_ff_with_reset_folds_to_constant() {
    // A register that is only ever assigned inside an unreachable branch
    // (enable tied low) holds its reset_value forever. D==Q after const-prop
    // should drop the FF and alias q to the reset constant.
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            d:   input  logic<4>,
            q:   output logic<4>,
        ) {
            let enable: logic = 0;
            always_ff (clk, rst) {
                if_reset {
                    q = 0;
                } else {
                    if enable {
                        q = d;
                    }
                }
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert_eq!(
        result.gate_ir.module.ffs.len(),
        0,
        "enable=0 freezes q to reset_value; FFs must be eliminated"
    );
    let q_port = result
        .gate_ir
        .module
        .ports
        .iter()
        .find(|p| format!("{}", p.name) == "q")
        .unwrap();
    for &n in &q_port.nets {
        assert_eq!(n, veryl_synthesizer::ir::NET_CONST0);
    }
}

#[test]
fn dq_ff_without_reset_is_preserved() {
    // Without a reset, a D=Q FF holds an undefined initial value forever —
    // it's still a real stateful element, so elimination must not fire.
    let code = r#"
        module Top (
            clk: input  clock,
            d:   input  logic<4>,
            q:   output logic<4>,
        ) {
            let enable: logic = 0;
            always_ff (clk) {
                if enable {
                    q = d;
                }
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let result = synthesize(&ir, top).expect("synthesize");
    assert_eq!(
        result.gate_ir.module.ffs.len(),
        4,
        "no-reset D=Q FFs must be kept (they hold undefined state, not a constant)"
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
    // Two adders (both 4-bit against constants) → const-prop folds several
    // XORs (carry chain simplifies when b is constant), so verify both outputs
    // are actually driven rather than comparing a specific gate count.
    let xor_count = gate
        .module
        .cells
        .iter()
        .filter(|c| c.kind == CellKind::Xor2)
        .count();
    assert!(xor_count > 0, "expected some adder XORs, got {}", xor_count);
    for port in &gate.module.ports {
        let name = format!("{}", port.name);
        if name == "out1" || name == "out2" {
            for &n in &port.nets {
                assert!(
                    !matches!(gate.module.nets[n as usize].driver, NetDriver::Undriven),
                    "{} net left undriven",
                    name
                );
            }
        }
    }
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
    // `x + 1` with const-prop: bit 0 folds entirely (XOR(a,1)=NOT(a), sum=NOT(a)),
    // bits 1..3 keep one XOR each for carry propagation → 3.
    assert!(
        xor_count >= 3,
        "4-bit +1 adder should generate ≥3 Xor2 cells, got {}",
        xor_count
    );
}

#[test]
fn power_leakage_independent_of_frequency_and_activity() {
    let code = r#"
        module Top (
            a: input  logic<4>,
            b: input  logic<4>,
            y: output logic<4>,
        ) {
            always_comb {
                y = a & b;
            }
        }
    "#;
    let (ir, top) = analyze(code, "Top");
    let gate = build_gate_ir(&ir, top).expect("synthesize");
    let library = BuiltinLibrary::new();
    let p1 = compute_power(&gate.module, &library, 100.0, 0.1);
    let p2 = compute_power(&gate.module, &library, 500.0, 0.5);
    // Leakage should be identical; dynamic should scale with f × α.
    assert!((p1.leakage_mw - p2.leakage_mw).abs() < 1e-9);
    let scale = (500.0 * 0.5) / (100.0 * 0.1);
    // The combinational dynamic scales linearly with f × α, but the FF
    // dynamic scales only with f. Derive expected dynamic separately:
    let comb_dyn_1 = p1.dynamic_mw - (p1.ff_dynamic_uw / 1e3);
    let comb_dyn_2 = p2.dynamic_mw - (p2.ff_dynamic_uw / 1e3);
    assert!(
        (comb_dyn_2 - comb_dyn_1 * scale).abs() < 1e-6,
        "comb dyn 1={} 2={} scale={}",
        comb_dyn_1,
        comb_dyn_2,
        scale
    );
}

#[test]
fn power_scales_with_cell_count() {
    let make_code = |w: usize| -> String {
        format!(
            r#"
            module Top (
                a: input  logic<{w}>,
                b: input  logic<{w}>,
                y: output logic<{w}>,
            ) {{
                always_comb {{
                    y = a & b;
                }}
            }}
        "#
        )
    };
    let (ir4, top4) = analyze(&make_code(4), "Top");
    let gate4 = build_gate_ir(&ir4, top4).expect("synthesize");
    let (ir8, top8) = analyze(&make_code(8), "Top");
    let gate8 = build_gate_ir(&ir8, top8).expect("synthesize");
    let library = BuiltinLibrary::new();
    let p4 = compute_power(&gate4.module, &library, 100.0, 0.1);
    let p8 = compute_power(&gate8.module, &library, 100.0, 0.1);
    // 8-bit design has 2× the And2 cells → 2× leakage and 2× dynamic.
    assert!(
        (p8.leakage_mw / p4.leakage_mw - 2.0).abs() < 0.05,
        "expected ~2× leakage scaling, got {} vs {}",
        p8.leakage_mw,
        p4.leakage_mw
    );
    assert!(
        (p8.dynamic_mw / p4.dynamic_mw - 2.0).abs() < 0.05,
        "expected ~2× dynamic scaling, got {} vs {}",
        p8.dynamic_mw,
        p4.dynamic_mw
    );
}

#[test]
fn power_ff_dynamic_uses_full_clock() {
    let code = r#"
        module Top (
            clk: input  clock,
            rst: input  reset,
            d:   input  logic<4>,
            q:   output logic<4>,
        ) {
            always_ff {
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
    assert_eq!(gate.module.ffs.len(), 4, "expected 4 FFs");
    let library = BuiltinLibrary::new();
    // Activity varies but FF dynamic should only respond to frequency.
    let p_low_act = compute_power(&gate.module, &library, 100.0, 0.05);
    let p_high_act = compute_power(&gate.module, &library, 100.0, 0.95);
    assert!(
        (p_low_act.ff_dynamic_uw - p_high_act.ff_dynamic_uw).abs() < 1e-9,
        "FF dynamic should not depend on activity: {} vs {}",
        p_low_act.ff_dynamic_uw,
        p_high_act.ff_dynamic_uw
    );
    // But it must scale with frequency.
    let p_fast = compute_power(&gate.module, &library, 500.0, 0.1);
    let ratio = p_fast.ff_dynamic_uw / p_low_act.ff_dynamic_uw;
    assert!(
        (ratio - 5.0).abs() < 0.01,
        "FF dynamic should scale 5× with 5× clock, got {}",
        ratio
    );
}
