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
        always_ff {
            if_reset { internal_reg = 0; }
            else { internal_reg = din + 1; }
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
fn hier_ref_before_inst_declaration_diagnosed() {
    // The referenced instance must be converted before the reference;
    // a use above the declaration gets an explicit diagnostic instead of
    // a silent conversion failure.
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
    let errors = analyze_errors(&code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::ReferringBeforeDefinition { .. })),
        "expected referring_before_definition, got: {errors:?}"
    );
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
