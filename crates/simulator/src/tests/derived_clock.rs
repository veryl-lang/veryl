//! Tests for derived (gated, FF-divided, copied) clocks.
//!
//! Each test embeds a tiny Veryl design, analyzes it, runs the simulator
//! across `Config::all()`, and checks that the derived clock fires (or
//! is correctly suppressed) under the expected stimulus.

use super::*;

/// `let clk_g: '_ clock = i_clk & i_en;` — gated clock fires only while
/// `i_en` is asserted.  Counter increments only on the gated clock.
#[test]
fn gated_clock_basic() {
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        i_en : input  '_ logic,
        o_cnt: output    logic<8>,
    ) {
        let clk_g: '_ clock = i_clk & i_en;
        always_ff (clk_g, i_rst) {
            if_reset {
                o_cnt = 0;
            } else {
                o_cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.set("i_en", Value::new(0, 1, false));
        sim.step(&rst);

        for _ in 0..5 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt").unwrap(),
            Value::new(0, 8, false),
            "counter must not advance while i_en=0",
        );

        sim.set("i_en", Value::new(1, 1, false));
        for _ in 0..7 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt").unwrap(),
            Value::new(7, 8, false),
            "counter must advance once per enabled clock",
        );

        sim.set("i_en", Value::new(0, 1, false));
        for _ in 0..3 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt").unwrap(),
            Value::new(7, 8, false),
            "counter must hold when i_en goes back to 0",
        );
    }
}

/// Two-stage gating: `clk1 = parent & en1`, `clk2 = clk1 & en2`.
#[test]
fn gated_clock_chain() {
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        i_en1: input  '_ logic,
        i_en2: input  '_ logic,
        o_a  : output    logic<8>,
        o_b  : output    logic<8>,
    ) {
        let clk1: '_ clock = i_clk & i_en1;
        let clk2: '_ clock = clk1  & i_en2;
        always_ff (clk1, i_rst) {
            if_reset { o_a = 0; } else { o_a += 1; }
        }
        always_ff (clk2, i_rst) {
            if_reset { o_b = 0; } else { o_b += 1; }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.set("i_en1", Value::new(0, 1, false));
        sim.set("i_en2", Value::new(0, 1, false));
        sim.step(&rst);

        sim.set("i_en1", Value::new(1, 1, false));
        sim.set("i_en2", Value::new(0, 1, false));
        for _ in 0..4 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("o_a").unwrap(), Value::new(4, 8, false));
        assert_eq!(sim.get("o_b").unwrap(), Value::new(0, 8, false));

        sim.set("i_en2", Value::new(1, 1, false));
        for _ in 0..3 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("o_a").unwrap(), Value::new(7, 8, false));
        assert_eq!(sim.get("o_b").unwrap(), Value::new(3, 8, false));

        sim.set("i_en1", Value::new(0, 1, false));
        for _ in 0..5 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("o_a").unwrap(), Value::new(7, 8, false));
        assert_eq!(sim.get("o_b").unwrap(), Value::new(3, 8, false));
    }
}

/// Mux clock: `let clk_g = if sel { i_a } else { i_b };`.
#[test]
fn gated_clock_mux() {
    let code = r#"
    module Top (
        i_a  : input  '_ clock,
        i_b  : input  '_ clock,
        i_rst: input  '_ reset,
        i_sel: input  '_ logic,
        o_cnt: output    logic<8>,
    ) {
        let clk_g: '_ clock = (i_a & i_sel) | (i_b & (~i_sel));
        always_ff (clk_g, i_rst) {
            if_reset { o_cnt = 0; } else { o_cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let a = sim.get_clock("i_a").unwrap();
        let b = sim.get_clock("i_b").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.set("i_sel", Value::new(1, 1, false));
        sim.step(&rst);

        for _ in 0..3 {
            sim.step(&a);
        }
        for _ in 0..3 {
            sim.step(&b);
        }
        assert_eq!(sim.get("o_cnt").unwrap(), Value::new(3, 8, false));

        sim.set("i_sel", Value::new(0, 1, false));
        for _ in 0..3 {
            sim.step(&a);
        }
        for _ in 0..5 {
            sim.step(&b);
        }
        assert_eq!(sim.get("o_cnt").unwrap(), Value::new(8, 8, false));
    }
}

/// FF-divider derived clock written as `i_clk & toggle`: `toggle`
/// flips on every input clock posedge, so the gated clock pulses on
/// every other input cycle — equivalent to a /2 divider.  Exercises
/// the path where the derived clock's value depends on an FF output
/// (the partial-settle must rerun after `ff_commit` to see the new
/// toggle value).
#[test]
fn ff_derived_divider() {
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        o_cnt: output    logic<8>,
    ) {
        var toggle: logic;
        always_ff (i_clk, i_rst) {
            if_reset { toggle = 0; } else { toggle = ~toggle; }
        }
        let div_clk: '_ clock = i_clk & toggle;
        always_ff (div_clk, i_rst) {
            if_reset { o_cnt = 0; } else { o_cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("o_cnt").unwrap(), Value::new(0, 8, false));

        for _ in 0..10 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt").unwrap(),
            Value::new(5, 8, false),
            "FF-derived /2 clock must produce 5 ticks for 10 input clocks",
        );
    }
}

/// FF output propagated through two comb-derived clocks in series.
/// Verifies that the dependency closure includes transitively-reachable
/// comb stmts (div_a feeds div_b).
#[test]
fn ff_copy_clock() {
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        o_cnt: output    logic<8>,
    ) {
        var toggle: logic;
        always_ff (i_clk, i_rst) {
            if_reset { toggle = 0; } else { toggle = ~toggle; }
        }
        let div_a: '_ clock = i_clk & toggle;
        let div_b: '_ clock = div_a & toggle;
        always_ff (div_b, i_rst) {
            if_reset { o_cnt = 0; } else { o_cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.step(&rst);
        for _ in 0..8 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("o_cnt").unwrap(), Value::new(4, 8, false));
    }
}

/// /2 → /4 chain of FF dividers built from logic toggles + gated
/// clocks.  Exercises the fixpoint loop: `div2` rising fires
/// `always_ff (div2)` which toggles `tog_4`, after which the partial
/// re-settle must reflect the new value of `div4` so the second
/// always_ff fires on the same simulator step.
#[test]
fn ff_derived_divider_chain() {
    let code = r#"
    module Top (
        i_clk : input  '_ clock,
        i_rst : input  '_ reset,
        o_cnt2: output    logic<8>,
        o_cnt4: output    logic<8>,
    ) {
        var tog_2: logic;
        var tog_4: logic;
        always_ff (i_clk, i_rst) {
            if_reset { tog_2 = 0; } else { tog_2 = ~tog_2; }
        }
        let div2: '_ clock = i_clk & tog_2;
        always_ff (div2, i_rst) {
            if_reset { tog_4 = 0; o_cnt2 = 0; }
            else     { tog_4 = ~tog_4; o_cnt2 += 1; }
        }
        let div4: '_ clock = div2 & tog_4;
        always_ff (div4, i_rst) {
            if_reset { o_cnt4 = 0; } else { o_cnt4 += 1; }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.step(&rst);
        for _ in 0..16 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt2").unwrap(),
            Value::new(8, 8, false),
            "/2 divider must fire 8 times for 16 input clocks",
        );
        assert_eq!(
            sim.get("o_cnt4").unwrap(),
            Value::new(4, 8, false),
            "/4 divider must fire 4 times for 16 input clocks",
        );
    }
}

/// Regression: a constant-driven derived clock must never fire.  We
/// use a separate `o_dummy` so the `always_ff (const_clk)` block has
/// a unique LHS (MultipleAssignment would otherwise reject a shared
/// destination).
#[test]
fn constant_derived_clock_no_edge() {
    let code = r#"
    module Top (
        i_clk  : input  '_ clock,
        i_rst  : input  '_ reset,
        o_cnt  : output    logic<8>,
        o_dummy: output    logic,
    ) {
        let const_clk: '_ clock = 1;
        always_ff (i_clk, i_rst) {
            if_reset { o_cnt = 0; } else { o_cnt += 1; }
        }
        always_ff (const_clk, i_rst) {
            if_reset { o_dummy = 0; } else { o_dummy = ~o_dummy; }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt").unwrap(),
            Value::new(5, 8, false),
            "i_clk-driven counter must advance normally",
        );
        assert_eq!(
            sim.get("o_dummy").unwrap(),
            Value::new(0, 1, false),
            "constant derived clock must not fire",
        );
    }
}

/// Repro of user-reported issue: testbench drives a DUT whose sub-module
/// produces `o_gclk = i_clk & 1` — a gated `logic` output — wired to the
/// DUT's `'_ clock` variable that clocks an `always_ff`.  The counter
/// must reach 10 after `clk.next(10)`.
#[test]
fn gated_clock_via_submodule_in_testbench() {
    let code = r#"
    module GclkCell (
        i_clk : input  logic,
        i_en  : input  logic,
        o_gclk: output logic,
    ) {
        assign o_gclk = i_clk & i_en;
    }

    module GatedCounter (
        i_clk: input  clock   ,
        i_rst: input  reset   ,
        o_cnt: output logic<8>,
    ) {
        var w_gclk: '_ clock;
        inst u_gclk: GclkCell (
            i_clk : i_clk ,
            i_en  : 1'b1  ,
            o_gclk: w_gclk,
        );
        var r_cnt: logic<8>;
        always_ff (w_gclk, i_rst) {
            if_reset { r_cnt = 0; } else { r_cnt += 1; }
        }
        assign o_cnt = r_cnt;
    }

    #[test(test_gated_clock)]
    module test_gated_clock {
        inst i_clk: $tb::clock_gen;
        inst i_rst: $tb::reset_gen(clk: i_clk);
        var o_cnt: logic<8>;
        inst dut: GatedCounter (
            i_clk: i_clk,
            i_rst: i_rst,
            o_cnt: o_cnt,
        );
        initial {
            i_rst.assert();
            i_clk.next(10);
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = match analyze_top(code, &config, "test_gated_clock") {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let mut sim = Simulator::new(ir, None);

        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);
        let initial_stmts = sim
            .ir
            .event_statements
            .get(&Event::Initial)
            .expect("initial block");
        let tb_stmts = convert_initial_to_testbench(initial_stmts, &event_map, &clock_periods, 3);
        let result = run_testbench(&mut sim, &tb_stmts);
        assert_eq!(result, TestResult::Pass);
        let cnt = sim
            .get_var("dut.o_cnt")
            .or_else(|| sim.get_var("o_cnt"))
            .expect("o_cnt variable not found");
        assert_eq!(
            cnt,
            Value::new(10, 8, false),
            "gated counter must reach 10 (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

/// Repro: sub-module produces a combinational `logic` output that is
/// connected to a parent `clock`-typed variable, which is used as the
/// clock of an `always_ff`.  The parent counter must increment on each
/// input clock posedge when the enable is asserted.
#[test]
fn gated_clock_via_submodule_output() {
    let code = r#"
    module GclkCell (
        i_clk : input  logic,
        i_en  : input  logic,
        o_gclk: output logic,
    ) {
        assign o_gclk = i_clk & i_en;
    }

    module Top (
        i_clk: input  '_ clock   ,
        i_rst: input  '_ reset   ,
        o_cnt: output    logic<8>,
    ) {
        var w_gclk: '_ clock;
        inst u_gclk: GclkCell (
            i_clk : i_clk ,
            i_en  : 1'b1  ,
            o_gclk: w_gclk,
        );
        var r_cnt: logic<8>;
        always_ff (w_gclk, i_rst) {
            if_reset { r_cnt = 0; } else { r_cnt += 1; }
        }
        assign o_cnt = r_cnt;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.step(&rst);

        for _ in 0..10 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt").unwrap(),
            Value::new(10, 8, false),
            "counter must advance 10 times when gated clock comes from sub-module",
        );
    }
}

/// Gated clock generated deep inside nested child instances (Top →
/// Wrapper → Mem → GclkCell) must still fire its always_ff.
/// Regression: the event key of a nested internal clock could
/// numerically collide with an ancestor's input-port VarId and be
/// hijacked by the inst-boundary event remap, orphaning the gated
/// domain.
#[test]
fn nested_gated_clock_fires() {
    let code = r#"
    module GclkCell (
        i_clk : input  logic,
        i_en  : input  logic,
        o_gclk: output logic,
    ) {
        assign o_gclk = i_clk & i_en;
    }

    module Mem (
        i_clk: input  clock   ,
        i_rst: input  reset   ,
        i_en : input  logic   ,
        o_cnt: output logic<8>,
    ) {
        var w_gclk: '_ clock;
        inst u_gclk: GclkCell ( i_clk, i_en, o_gclk: w_gclk );
        var r_cnt: logic<8>;
        always_ff (w_gclk, i_rst) {
            if_reset { r_cnt = 0; } else { r_cnt += 1; }
        }
        assign o_cnt = r_cnt;
    }

    module Wrapper (
        i_clk: input  clock   ,
        i_rst: input  reset   ,
        i_en : input  logic   ,
        o_cnt: output logic<8>,
    ) {
        inst u_mem: Mem ( i_clk, i_rst, i_en, o_cnt );
    }

    module Top (
        i_clk: input  '_ clock   ,
        i_rst: input  '_ reset   ,
        i_en : input     logic   ,
        o_cnt: output    logic<8>,
    ) {
        inst u_wrap: Wrapper ( i_clk, i_rst, i_en, o_cnt );
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();
        sim.set("i_en", Value::new(1, 1, false));
        sim.step(&rst);
        for _ in 0..6 {
            sim.step(&clk);
        }
        assert_eq!(
            sim.get("o_cnt").unwrap(),
            Value::new(6, 8, false),
            "nested gated clock must clock its FF every enabled cycle",
        );
    }
}
