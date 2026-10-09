use crate::ir::Config;
use crate::output_buffer;
use crate::simulator::Simulator;
use crate::tests::analyze;
use veryl_analyzer::value::Value;

fn aot_config(validate: bool) -> Config {
    Config {
        use_4state: false,
        use_jit: true,
        aot_c: true,
        aot_c_event: true,
        aot_c_async: false,
        aot_c_validate: validate,
        aot_c_min_stmts: 0,
        ..Default::default()
    }
}

#[test]
fn validated_event_keeps_the_marks_of_the_code_it_keeps() {
    // A validated fire keeps the Cranelift result, so the readers' marks must
    // come from the Cranelift code (or mark everything), never from the AOT-C
    // function whose stores were rolled back.
    if !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (clk: input clock, rst: input reset, o: output logic<8>) {
        var r: logic<8>;
        var c: logic<8>;
        always_ff (clk, rst) {
            if_reset { r = 0; c = 0; } else {
                c = c + 1;
                for i in 0..4 {
                    if i == 0 { r = c; if c[0] { break; } }
                }
            }
        }
        assign o = r;
    }
    "#;
    for validate in [false, true] {
        let ir = analyze(code, &aot_config(validate));
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step_reset(&clk, &rst);
        let mut v = Vec::new();
        for _ in 0..8 {
            sim.step(&clk);
            sim.ensure_comb_updated();
            v.push(sim.get("o").unwrap().payload_u64());
        }
        assert_eq!(v, (0..8u64).collect::<Vec<_>>(), "validate={validate}");
    }
}

#[test]
fn validated_wide_select_reaches_its_readers() {
    if !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        i_v  : input  signed logic<32>,
        o_hi : output logic<32>,
        o_lo : output logic<32>,
    ) {
        var x: logic<1184>;
        always_ff {
            if_reset {
                x = 0;
            } else {
                x[1183:512] = i_v;
            }
        }
        assign o_hi = x[1183:1152];
        assign o_lo = x[543:512];
    }
    "#;
    let run = |config: &Config| {
        let ir = analyze(code, config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();
        let mut out = vec![];
        for v in [0xFFFF_FFFFu64, 0x0000_0001, 0x8000_0000, 0x1234_5678] {
            sim.set("i_v", Value::new(v, 32, true));
            sim.step_reset(&clk, &rst);
            sim.step(&clk);
            out.push((
                sim.get("o_hi").unwrap().payload_u64(),
                sim.get("o_lo").unwrap().payload_u64(),
            ));
        }
        out
    };
    let interp = run(&Config::default());
    assert_eq!(run(&aot_config(false)), interp, "plain");
    assert_eq!(run(&aot_config(true)), interp, "validate");
}

#[test]
fn check_reruns_the_comb_without_repeating_its_output() {
    if !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        clk: input  clock   ,
        a  : input  logic<8>,
        o  : output logic<8>,
    ) {
        var t: logic<8>;
        always_comb {
            t = a + 1;
            if a == 8'd7 {
                $display("hit a=%h t=%h", a, t);
            }
        }
        always_ff (clk) {
            o = t;
        }
    }
    "#;
    output_buffer::enable();
    let ir = analyze(code, &aot_config(false));
    ir.dataflow_check.set(1);
    let mut sim = Simulator::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    for a in [5u64, 7, 9] {
        sim.set("a", Value::new(a, 8, false));
        sim.step(&clk);
    }
    let output = output_buffer::take();
    assert_eq!(output.matches("hit a=07 t=08").count(), 1, "{output:?}");
}

#[test]
fn check_reports_a_suppressed_mark() {
    if !crate::tests::dataflow_on() || !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        a: input  logic<8>,
        o: output logic<8>,
    ) {
        assign o = a + 1;
    }
    "#;
    let ir = analyze(code, &aot_config(false));
    assert!(ir.dataflow_ready());
    ir.dataflow_check.set(1);
    ir.dataflow_check_panic.set(false);
    let mut sim = Simulator::new(ir, None);
    sim.set("a", Value::new(1, 8, false));
    assert_eq!(sim.get("o").unwrap().payload_u64(), 2);
    assert_eq!(sim.ir.dataflow_check_mismatches.get(), 0);

    sim.set("a", Value::new(5, 8, false));
    sim.ir.dataflow_dirty.borrow_mut().fill(0);
    // The reference value is kept even though the cell did not run.
    assert_eq!(sim.get("o").unwrap().payload_u64(), 6);
    assert_eq!(sim.ir.dataflow_check_mismatches.get(), 1);
    assert!(!sim.ir.dataflow_live.get());
}

#[test]
fn a_declined_dataflow_settle_is_reported() {
    if !crate::tests::dataflow_on() {
        return;
    }
    let listed = |name: &str| {
        crate::residency::degraded_modules().contains(&format!("dataflow_declined:{name}"))
    };
    let code = |name: &str| {
        format!("module {name} (a: input logic<8>, o: output logic<8>) {{ assign o = a + 1; }}")
    };
    let four_state = Config {
        use_4state: true,
        ..aot_config(false)
    };
    let name = "DfDeclinedFourState";
    crate::tests::analyze_top(&code(name), &four_state, name).unwrap();
    assert!(listed(name), "a 4-state module big enough for AOT-C");

    // Below the size floor no dataflow settle is expected.
    let small = Config {
        aot_c_min_stmts: usize::MAX,
        ..four_state
    };
    let name = "DfDeclinedTooSmall";
    crate::tests::analyze_top(&code(name), &small, name).unwrap();
    assert!(!listed(name), "a module below the size floor");
}

/// Two cells (`x`, and `o` reading it with other inputs) plus an unrelated
/// cell `q` that drives settles without touching the others.
const TWO_CELLS: &str = r#"
    module Top (
        a : input  logic<8>,
        z1: input  logic<8>,
        z2: input  logic<8>,
        c : input  logic<8>,
        o : output logic<8>,
        q : output logic<8>,
    ) {
        var x: logic<8>;
        assign x = a & 8'h0f;
        assign o = x + (z1 ^ z2);
        assign q = c + 1;
    }
"#;

/// Settles through `c` and `q` until the runtime picks the given mode.
fn settle_until_mode(sim: &mut Simulator, coarse: bool, tick: &mut u64) {
    for _ in 0..8192 {
        if sim.ir.dataflow_mode.was_coarse.get() == coarse {
            return;
        }
        *tick += 1;
        sim.set("c", Value::new(*tick & 0x7f, 8, false));
        sim.get("q").unwrap();
    }
    panic!("the mode never became coarse={coarse}");
}

#[test]
fn leaving_the_whole_settle_entry_reruns_every_cell() {
    if !crate::tests::dataflow_on() || !crate::backend::aot_c::cc_available() {
        return;
    }
    let ir = analyze(TWO_CELLS, &aot_config(false));
    assert!(
        ir.dataflow
            .as_ref()
            .unwrap()
            .cell
            .get()
            .unwrap()
            .coarse_func
            .is_some()
    );
    let mut sim = Simulator::new(ir, None);
    let v = |x: u64| Value::new(x, 8, false);
    sim.set("z1", v(0));
    sim.set("z2", v(0));
    sim.set("a", v(0x31));
    assert_eq!(sim.get("o").unwrap(), v(0x01));
    let mut tick = 0;
    settle_until_mode(&mut sim, true, &mut tick);
    // The whole-settle entry moves `x` past its cell's boundary shadow.
    sim.set("a", v(0x36));
    assert_eq!(sim.get("o").unwrap(), v(0x06));
    settle_until_mode(&mut sim, false, &mut tick);
    // `x` returns to the value in that shadow, so only a rerun of every cell
    // reaches `o`.
    sim.set("a", v(0x41));
    assert_eq!(sim.get("o").unwrap(), v(0x01));
}

#[test]
fn whole_settle_entry_runs_after_an_unmarked_commit() {
    if !crate::tests::dataflow_on() || !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        clk: input  clock   ,
        a  : input  logic<8>,
        c  : input  logic<8>,
        o  : output logic<8>,
        q  : output logic<8>,
    ) {
        var r: logic<8>;
        always_ff (clk) { r = a; }
        assign o = r + 1;
        assign q = c + 1;
    }
    "#;
    let ir = analyze(code, &aot_config(false));
    assert!(
        ir.dataflow
            .as_ref()
            .unwrap()
            .cell
            .get()
            .unwrap()
            .coarse_func
            .is_some()
    );
    let mut sim = Simulator::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    let v = |x: u64| Value::new(x, 8, false);
    sim.set("a", v(3));
    sim.step(&clk);
    assert_eq!(sim.get("o").unwrap(), v(4));
    let mut tick = 0;
    settle_until_mode(&mut sim, true, &mut tick);
    // Nothing combinational reads `a`: only the commit says `r` moved.
    sim.set("a", v(9));
    sim.step(&clk);
    assert_eq!(sim.get("o").unwrap(), v(10));
}

#[test]
fn whole_settle_entry_leaves_cell_boundaries_in_the_buffer() {
    // A settle with only the side-effect cells dirty runs the per-cell
    // dispatch even in whole-settle mode, so those cells must find what the
    // whole-settle entry wrote.
    if !crate::tests::dataflow_on() || !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        u: input  logic<8>,
        o: output logic<8>,
    ) {
        let x: logic<8> = a + 8'd1;
        always_comb {
            if b == 8'd1 {
                $display("x=%h", x);
            }
        }
        assign o = b;
    }
    "#;
    output_buffer::enable();
    let ir = analyze(code, &aot_config(false));
    assert!(ir.dataflow_ready());
    let mut sim = Simulator::new(ir, None);
    let v = |x: u64| Value::new(x, 8, false);
    sim.set("b", v(0));
    let mut tick = 0u64;
    while !sim.ir.dataflow_mode.was_coarse.get() {
        tick += 1;
        assert!(tick < 8192, "the mode never became coarse");
        sim.set("a", v(tick & 0x3f));
        sim.get("o").unwrap();
    }
    sim.set("a", v(0x50));
    sim.get("o").unwrap();
    output_buffer::take();
    output_buffer::enable();
    sim.set("b", v(1));
    sim.get("o").unwrap();
    // `u` has no reader: only the side-effect cell runs.
    sim.set("u", v(7));
    sim.get("o").unwrap();
    let out = output_buffer::take();
    assert!(out.contains("x="), "{out:?}");
    assert_eq!(
        out.matches("x=51").count(),
        out.matches("x=").count(),
        "{out:?}"
    );
}

#[test]
fn readmemh_in_an_event_reaches_its_comb_readers() {
    let hex = std::env::temp_dir().join(format!("df_readmemh_{}.hex", std::process::id()));
    std::fs::write(&hex, "0A 14 1E 28\n").unwrap();
    let code = format!(
        r#"
    module Top (clk: input '_ clock, o: output logic<8>, p: output logic<8>) {{
        #[allow(initial_assign)]
        var mem: logic<8> [4];
        #[allow(initial_assign)]
        var q: logic<8>;
        initial {{
            $readmemh("{}", mem);
            q = 8'h33;
        }}
        always_ff (clk) {{ q = q + 1; }}
        assign o = mem[1] + mem[2];
        assign p = q;
    }}
    "#,
        hex.to_str().unwrap()
    );
    let mut configs = vec![Config {
        use_jit: true,
        ..Default::default()
    }];
    if crate::backend::aot_c::cc_available() {
        configs.push(aot_config(false));
    }
    for config in configs {
        let ir = analyze(&code, &config);
        let mut sim = Simulator::new(ir, None);
        let mut trace = vec![];
        let mut rec = |sim: &mut Simulator| {
            trace.push((
                sim.get("o").unwrap().payload_u64(),
                sim.get("p").unwrap().payload_u64(),
            ));
        };
        rec(&mut sim);
        sim.step(&crate::ir::Event::Initial);
        rec(&mut sim);
        let clk = sim.get_clock("clk").unwrap();
        sim.step(&clk);
        rec(&mut sim);
        assert_eq!(trace[1..], [(0x32, 0x33), (0x32, 0x34)], "{config:?}");
    }
    let _ = std::fs::remove_file(&hex);
}

#[test]
fn validate_checks_the_dataflow_settle() {
    if !crate::tests::dataflow_on() || !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        a: input  logic<8>,
        o: output logic<8>,
    ) {
        assign o = a + 1;
    }
    "#;
    let ir = analyze(code, &aot_config(true));
    assert!(ir.dataflow_ready());
    let mut sim = Simulator::new(ir, None);
    sim.set("a", Value::new(1, 8, false));
    assert_eq!(sim.get("o").unwrap().payload_u64(), 2);
    sim.set("a", Value::new(5, 8, false));
    sim.ir.dataflow_dirty.borrow_mut().fill(0);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sim.get("o")));
    assert!(r.is_err(), "a suppressed mark must fail --backend-validate");
}

#[test]
fn an_emitter_decline_is_reported() {
    if !crate::tests::dataflow_on() || !crate::backend::aot_c::cc_available() {
        return;
    }
    let name = "DfDeclinedWideDiv";
    let code = format!(
        "module {name} (a: input logic<200>, b: input logic<200>, o: output logic<200>) {{ assign o = a / b; }}"
    );
    let ir = crate::tests::analyze_top(&code, &aot_config(false), name).unwrap();
    assert!(ir.dataflow.is_none());
    assert!(
        crate::residency::degraded_modules().contains(&format!("dataflow_declined:{name}")),
        "a module the emitter declines must be listed"
    );
}
