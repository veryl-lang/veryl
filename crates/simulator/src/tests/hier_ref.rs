use super::*;

// Hierarchical testbench references (`dut.u_sub.x`) into module instances.

// Signals observed only by the testbench still count as unused for the
// analyzer (the hierarchical read does not register a reference), hence
// the allow attributes.
const HIER_DUT: &str = r#"
    module Sub (
        clk: input clock,
        rst: input reset,
        din: input logic<4>,
    ) {
        #[allow(unused_variable)]
        var internal_reg: logic<4>;
        #[allow(unused_variable)]
        let internal_let: logic<4> = din + 2;

        struct Inner { x: logic<4>, y: logic<4> }
        struct Pair { hi: logic<8>, lo: Inner }
        #[allow(unused_variable)]
        var st: Pair;
        #[allow(unused_variable)]
        var arr: Pair [2];
        union U { p: logic<8>, q: logic<8> }
        #[allow(unused_variable)]
        var un: U;

        always_ff {
            if_reset {
                internal_reg = 0;
                st = Pair'{hi: 0, lo: Inner'{x: 0, y: 0}};
                arr = '{Pair'{hi: 0, lo: Inner'{x: 0, y: 0}}, Pair'{hi: 0, lo: Inner'{x: 0, y: 0}}};
                un.p = 0;
            } else {
                internal_reg = din + 1;
                st = Pair'{hi: 8'ha5, lo: Inner'{x: 4'h3, y: 4'hc}};
                arr = '{Pair'{hi: 8'h11, lo: Inner'{x: 4'h1, y: 4'h2}},
                        Pair'{hi: 8'h33, lo: Inner'{x: 4'h3, y: 4'h4}}};
                un.p = 8'h7e;
            }
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
        din: input logic<4>,
    ) {
        inst u_sub: Sub (clk, rst, din);
    }
"#;

fn hier_testbench(body: &str) -> String {
    format!(
        r#"
    {HIER_DUT}

    #[test(hier_test)]
    module hier_test {{
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var din: logic<4>;

        inst dut: Top (clk, rst, din);

        initial {{
            rst.assert();
            din = 4'b0001;
            clk.next();
            {body}
            $finish();
        }}
    }}
    "#
    )
}

#[track_caller]
fn run_hier_test(code: &str) -> Vec<(Config, TestResult, Simulator)> {
    let mut ret = vec![];
    for config in Config::all() {
        let ir = analyze_top(code, &config, "hier_test")
            .unwrap_or_else(|x| panic!("build failed for {config:?}: {x:?}"));
        let mut sim = Simulator::new(ir, None);

        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);
        let stmts = sim.ir.event_statements.get(&Event::Initial).unwrap();
        let tb_stmts = convert_initial_to_testbench(stmts, &event_map, &clock_periods, 3);
        let result = run_testbench(&mut sim, &tb_stmts);
        ret.push((config, result, sim));
    }
    ret
}

#[test]
fn hier_ref_assert_reads_committed_ff() {
    // After clk.next() the FF holds din + 1 = 2; the reference must see the
    // committed value, exactly like an RTL reader.
    let code = hier_testbench(r#"$assert(dut.u_sub.internal_reg == 4'h2, "ff mismatch");"#);
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_assert_failure_detected() {
    // A wrong expectation must fail: proves the reference reads the real
    // value rather than a constant.
    let code = hier_testbench(r#"$assert(dut.u_sub.internal_reg == 4'hf, "expected");"#);
    for (config, result, _) in run_hier_test(&code) {
        assert!(
            matches!(result, TestResult::Fail(_)),
            "config: {config:?}, result: {result:?}"
        );
    }
}

#[test]
fn hier_ref_bit_select() {
    // internal_reg == 2 -> bit 1 is set, bit 0 is clear.
    let code = hier_testbench(
        r#"
            $assert(dut.u_sub.internal_reg[1] == 1'b1, "bit1");
            $assert(dut.u_sub.internal_reg[0] == 1'b0, "bit0");
        "#,
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_survives_dce() {
    // internal_let is read only by the testbench; the DCE census must count
    // that read and keep the comb assign alive.
    let code = hier_testbench(r#"$assert(dut.u_sub.internal_let == 4'h3, "let mismatch");"#);
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_in_display_and_expression() {
    // $display args, compound expressions, and if conditions share the
    // same conversion funnel.
    let code = hier_testbench(
        r#"
            $display("reg = %h", dut.u_sub.internal_reg);
            $assert(dut.u_sub.internal_reg + dut.u_sub.internal_let == 4'h5, "sum");
            if dut.u_sub.internal_reg == 4'h2 {
                $display("ok");
            } else {
                $assert(0 == 1, "if-cond took wrong branch");
            }
        "#,
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_get_var_hierarchical_path() {
    // Simulator::get_var must resolve dotted paths through instance children
    // (VarPath::from_str splits on '.'). The in-body reference keeps the
    // variable alive: get_var alone is not a DCE root.
    let code = hier_testbench(r#"$assert(dut.u_sub.internal_reg == 4'h2, "keep alive");"#);
    for (config, result, mut sim) in run_hier_test(&code) {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
        let value = sim
            .get_var("dut.u_sub.internal_reg")
            .expect("hierarchical get_var failed");
        assert_eq!(value, Value::new(2, 4, false), "config: {config:?}");
    }
}

#[test]
fn hier_ref_no_shadowing_by_nested_inst_name() {
    // A testbench-local struct variable that happens to share its name with
    // an instance nested inside the DUT must keep resolving as a plain
    // variable; only instances of the test module itself root a
    // hierarchical reference.
    let code = format!(
        r#"
    {HIER_DUT}

    #[test(hier_test)]
    module hier_test {{
        struct Cfg {{
            field: logic<4>,
        }}

        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var din  : logic<4>;
        var u_sub: Cfg     ;

        inst dut: Top (clk, rst, din);

        initial {{
            rst.assert();
            din         = 4'b0001;
            u_sub.field = 4'h9;
            clk.next();
            $assert(u_sub.field == 4'h9, "local struct field");
        }}
    }}
    "#
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

fn analyze_errors(code: &str) -> Vec<AnalyzerError> {
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
    errors
}

#[test]
fn hier_ref_invisible_outside_test_module() {
    // A hierarchical reference in a normal module must keep reporting
    // invisible_identifier.
    let code = r#"
    module Sub (
        clk: input clock,
        o_q: output logic,
    ) {
        var internal: logic;
        always_ff (clk) { internal = 1; }
        assign o_q = internal;
    }

    module Top (
        clk: input clock,
        o_x: output logic,
    ) {
        var q: logic;
        inst u_sub: Sub (clk, o_q: q);
        assign o_x = u_sub.internal;
    }
    "#;
    let errors = analyze_errors(code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvisibleIndentifier { .. })),
        "expected invisible_identifier, got: {errors:?}"
    );
}

#[test]
fn hier_ref_in_function_rejected() {
    // A function body is converted once and shared with RTL callers, so a
    // hierarchical reference inside it must be rejected even when the
    // function is first called from an initial block.
    let code = hier_testbench(r#"$assert(f() == 4'h2, "fn");"#).replace(
        "initial {",
        r#"function f () -> logic<4> {
            return dut.u_sub.internal_reg;
        }
        always_comb {
            y = f();
        }
        var y: logic<4>;
        initial {"#,
    );
    let errors = analyze_errors(&code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvisibleIndentifier { .. })),
        "expected invisible_identifier, got: {errors:?}"
    );
}

#[test]
fn hier_ref_unknown_member_diagnosed() {
    let code = hier_testbench(r#"$assert(dut.u_sub.no_such_signal == 4'h2, "typo");"#);
    let errors = analyze_errors(&code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::UnknownMember { .. })),
        "expected unknown_member, got: {errors:?}"
    );
}

#[test]
fn hier_ref_struct_member() {
    let code = hier_testbench(
        r#"
            $assert(dut.u_sub.st.hi == 8'ha5, "member");
            $assert(dut.u_sub.st.lo.x == 4'h3, "nested member");
            $assert(dut.u_sub.st.lo.y == 4'hc, "nested member y");
            $assert(dut.u_sub.st.hi[3:0] == 4'h5, "bit select on a member");
            $assert(dut.u_sub.st == 16'ha53c, "whole struct");
            $assert(dut.u_sub.arr[1].hi == 8'h33, "array element member");
            $assert(dut.u_sub.arr[0].lo.y == 4'h2, "array element nested member");
            $assert(dut.u_sub.un.q == 8'h7e, "union member");
        "#,
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_unknown_struct_member_names_the_struct() {
    // The owner must be the struct, not the instance that holds it.
    let code = hier_testbench(r#"$assert(dut.u_sub.st.nope == 8'h0, "typo");"#);
    let errors = analyze_errors(&code);
    assert!(
        errors.iter().any(|x| matches!(
            x,
            AnalyzerError::UnknownMember { name, member, .. }
                if name == "dut.u_sub.st" && member == "nope"
        )),
        "expected unknown_member on the struct, got: {errors:?}"
    );
}

#[test]
fn hier_ref_before_inst_declaration_works() {
    // Module items are order-free: initial blocks convert after the other
    // declarations, so a hierarchical reference above the instance
    // declaration resolves normally.
    let code = format!(
        r#"
    {HIER_DUT}

    #[test(hier_test)]
    module hier_test {{
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var din: logic<4>;

        initial {{
            rst.assert();
            din = 4'b0001;
            clk.next();
            $assert(dut.u_sub.internal_reg == 4'h2, "early");
            $finish();
        }}

        inst dut: Top (clk, rst, din);
    }}
    "#
    );
    for (config, result, _) in run_hier_test(&code) {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_rtl_context_diagnosed() {
    // Hierarchical references are testbench-only; RTL inside a test module
    // must keep reporting invisible_identifier.
    let code = hier_testbench("").replace(
        "initial {",
        r#"var y: logic<4>;
        always_comb {
            y = dut.u_sub.internal_reg;
        }
        initial {"#,
    );
    let errors = analyze_errors(&code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvisibleIndentifier { .. })),
        "expected invisible_identifier, got: {errors:?}"
    );
}

#[test]
fn hier_ref_instance_array_diagnosed() {
    let code = r#"
    module ASub {
        #[allow(unused_variable)]
        let tap: logic<4> = 4'h3;
    }

    module ATop {
        inst u_sub: ASub;
    }

    #[test(hier_test)]
    module hier_test {
        inst clk: $tb::clock_gen;
        inst arr: ATop [2];

        initial {
            clk.next();
            $assert(arr.u_sub.tap == 4'h3, "array");
            $finish();
        }
    }
    "#;
    let errors = analyze_errors(code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvalidFactor { .. })),
        "expected invalid_factor, got: {errors:?}"
    );
}

// A generate-for block stores its children under a `label[index]` segment
// (e.g. `g_leaf[0]`); a testbench reference must fold that hop to reach the
// instance inside. Each iteration is given a distinct value so a wrong fold
// (or one that collapses every index to the same instance) is observable.
const GEN_HIER_DUT: &str = r#"
    module GLeaf (
        clk  : input clock    ,
        i_val: input logic<32>,
    ) {
        #[allow(unused_variable)]
        var mem: logic<32>;
        always_ff {
            mem = i_val;
        }
    }

    module GMid (
        clk: input clock,
    ) {
        for i in 0..2 :g_leaf {
            inst u_leaf: GLeaf (clk, i_val: 32'hcafe_0000 + i);
        }
    }

    module GDeepTop (
        clk: input clock,
    ) {
        inst u_mid: GMid (clk);
    }
"#;

fn gen_hier_testbench(dut_type: &str, body: &str) -> String {
    format!(
        r#"
    {GEN_HIER_DUT}

    #[test(hier_test)]
    module hier_test {{
        inst clk: $tb::clock_gen;

        inst dut: {dut_type} (clk);

        initial {{
            clk.next();
            {body}
            $finish();
        }}
    }}
    "#
    )
}

#[test]
fn hier_ref_generate_block_direct() {
    // Generate block directly under the DUT: `dut.g_leaf[0].u_leaf.mem`.
    let code = gen_hier_testbench(
        "GMid",
        r#"
            $assert(dut.g_leaf[0].u_leaf.mem == 32'hcafe_0000, "g0");
            $assert(dut.g_leaf[1].u_leaf.mem == 32'hcafe_0001, "g1");
        "#,
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_generate_block_through_inst() {
    // Generate block one plain-instance hop below the DUT:
    // `dut.u_mid.g_leaf[0].u_leaf.mem`. The hop must descend into the plain
    // instance's module before the generate label can be folded.
    let code = gen_hier_testbench(
        "GDeepTop",
        r#"
            $assert(dut.u_mid.g_leaf[0].u_leaf.mem == 32'hcafe_0000, "g0");
            $assert(dut.u_mid.g_leaf[1].u_leaf.mem == 32'hcafe_0001, "g1");
        "#,
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

// A hierarchical read whose element index is a runtime value (a testbench
// loop variable) must evaluate the index at simulation time, not fold it to
// a compile-time constant. Each element carries a distinct value so a wrong
// fold (which collapses every read to element 0) is caught for k >= 1.
const DYN_IDX_DUT: &str = r#"
    module DLeaf (
        clk: input clock,
    ) {
        #[allow(unused_variable)]
        var mem: logic<32> [4];
        always_ff {
            mem[0] = 32'hcafe_0000;
            mem[1] = 32'hcafe_0001;
            mem[2] = 32'hcafe_0002;
            mem[3] = 32'hcafe_0003;
        }
    }

    module DGenMid (
        clk: input clock,
    ) {
        for g in 0..2 :g_leaf {
            inst u_leaf: DLeaf (clk);
        }
    }

    module DPlainMid (
        clk: input clock,
    ) {
        inst u_leaf: DLeaf (clk);
    }

    module DTop (
        clk: input clock,
    ) {
        inst u_gen  : DGenMid   (clk);
        inst u_plain: DPlainMid (clk);
    }
"#;

#[test]
fn hier_ref_dynamic_index_generate_and_plain() {
    // Covers both a generate-nested leaf and a plain leaf.
    let code = format!(
        r#"
    {DYN_IDX_DUT}

    #[test(hier_test)]
    module hier_test {{
        inst clk: $tb::clock_gen;
        inst dut: DTop (clk);

        initial {{
            clk.next();
            for k in 0..4 {{
                var vg: logic<32>;
                var vp: logic<32>;
                vg = dut.u_gen.g_leaf[0].u_leaf.mem[k];
                vp = dut.u_plain.u_leaf.mem[k];
                $assert(vg == 32'hcafe_0000 + k, "gen dynamic index");
                $assert(vp == 32'hcafe_0000 + k, "plain dynamic index");
            }}
            $finish();
        }}
    }}
    "#
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_dynamic_index_from_hierarchical_reference() {
    // The index is itself a hierarchical reference (`mem[dut...idx]`); the
    // reference nested in it must be resolved, not left as a placeholder.
    let dut = r#"
    module DLeaf (
        clk: input clock,
    ) {
        #[allow(unused_variable)]
        var mem: logic<32> [4];
        always_ff {
            mem[0] = 32'hcafe_0000;
            mem[1] = 32'hcafe_0001;
            mem[2] = 32'hcafe_0002;
            mem[3] = 32'hcafe_0003;
        }
    }

    module IdxTop (
        clk: input clock,
    ) {
        #[allow(unused_variable)]
        var sel: logic<2>;
        always_ff {
            sel = 2'd2;
        }
        inst u_leaf: DLeaf (clk);
    }
    "#;
    let code = format!(
        r#"
    {dut}

    #[test(hier_test)]
    module hier_test {{
        inst clk: $tb::clock_gen;
        inst dut: IdxTop (clk);

        initial {{
            clk.next();
            var v: logic<32>;
            v = dut.u_leaf.mem[dut.sel];
            $assert(v == 32'hcafe_0002, "index via hierarchical reference");
            $finish();
        }}
    }}
    "#
    );
    let results = run_hier_test(&code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_nested_test_module_resolves_locally() {
    // A test module instantiated inside another test module carries its own
    // initial statements; its hierarchical references must resolve against
    // its own instances, not same-named instances of the enclosing top.
    let code = r#"
    module KSub3 {
        #[allow(unused_variable)]
        let tap: logic<4> = 4'h3;
    }
    module KTop3 {
        inst u_sub: KSub3;
    }
    module KSub7 {
        #[allow(unused_variable)]
        let tap: logic<4> = 4'h7;
    }
    module KTop7 {
        inst u_sub: KSub7;
    }

    #[test(inner_t)]
    module inner_t {
        inst dut: KTop3;
        initial {
            $assert(dut.u_sub.tap == 4'h3, "inner");
        }
    }

    #[test(hier_test)]
    module hier_test {
        inst clk: $tb::clock_gen;
        inst dut: KTop7;
        initial {
            clk.next();
            $assert(dut.u_sub.tap == 4'h7, "outer");
        }
        inst sub: inner_t;
    }
    "#;
    let results = run_hier_test(code);
    assert!(!results.is_empty());
    for (config, result, _) in results {
        assert_eq!(result, TestResult::Pass, "config: {config:?}");
    }
}

#[test]
fn hier_ref_runtime_select_reads_the_indexed_element() {
    // Regression: a runtime select on a hierarchical read was evaluated
    // eagerly, folding the unknown index to 0. No error — just wrong data,
    // which let a `$assert` on the wrong element pass.
    let code = r#"
    module Sub (
        clk: input clock,
        rst: input reset,
    ) {
        #[allow(unused_variable)]
        var s: logic<4, 8>;
        #[allow(unused_variable)]
        var v: logic<8>;
        #[allow(unused_variable)]
        var k: logic<2>;
        always_ff {
            if_reset {
                for i in 0..4 {
                    s[i] = (i * 16 + 1) as 8;
                }
                v = 8'b10100100;
                k = 2;
            }
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
    ) {
        inst u_sub: Sub (clk, rst);
    }

    #[test(hier_dyn_select)]
    module hier_dyn_select {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        inst dut: Top (clk, rst);

        initial {
            rst.assert();
            clk.next();
            for i in 0..4 {
                $assert(dut.u_sub.s[i] == ((i * 16 + 1) as 8), "packed element");
                for b in 0..8 {
                    $assert(dut.u_sub.s[i][b] == (((i * 16 + 1) >> b) & 1) as 1, "element bit");
                }
            }
            for i in 0..8 {
                $assert(dut.u_sub.v[i] == ((8'b10100100 >> i) & 1) as 1, "bit select");
            }
            // An index that is itself a hierarchical read used to be rejected.
            $assert(dut.u_sub.s[dut.u_sub.k] == 8'h21, "index from the hierarchy");
            $finish();
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze_top(code, &config, "hier_dyn_select")
            .unwrap_or_else(|x| panic!("build failed for {config:?}: {x:?}"));
        let module_name = ir.name.to_string();
        assert_eq!(
            run_native_testbench(ir, None, module_name).unwrap(),
            TestResult::Pass,
            "config: {config:?}"
        );
    }
}

#[test]
fn hier_ref_struct_member_in_interface() {
    // An interface flattens into multi-segment variable paths, so the member
    // must resolve against `bus.pkt`, not `bus`.
    let code = r#"
    package pk {
        struct P { f: logic<8>, g: logic<8> }
    }
    interface If {
        var pkt:   pk::P;
        var valid: logic;
        modport mp { pkt: output, valid: output }
    }
    module Sub (
        clk: input clock,
        rst: input reset,
    ) {
        inst bus: If;
        always_ff {
            if_reset {
                bus.pkt.f = 8'h7a;
                bus.pkt.g = 8'ha7;
                bus.valid = 1;
            }
        }
    }
    module Top (
        clk: input clock,
        rst: input reset,
    ) {
        inst u_sub: Sub (clk, rst);
    }
    #[test(hier_if_test)]
    module hier_if_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        inst dut: Top (clk, rst);
        initial {
            rst.assert();
            clk.next();
            $assert(dut.u_sub.bus.valid == 1, "flattened member");
            $assert(dut.u_sub.bus.pkt == 16'h7aa7, "whole struct member");
            $assert(dut.u_sub.bus.pkt.f == 8'h7a, "struct member f");
            $assert(dut.u_sub.bus.pkt.g == 8'ha7, "struct member g");
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "hier_if_test")
            .unwrap_or_else(|x| panic!("build failed for {config:?}: {x:?}"));
        let module_name = ir.name.to_string();
        assert_eq!(
            run_native_testbench(ir, None, module_name).unwrap(),
            TestResult::Pass,
            "config: {config:?}"
        );
    }
}
