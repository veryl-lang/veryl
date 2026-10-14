//! The coarse tier clears acyclic modules before the precise tier runs.
use super::*;
use crate::comb_loop_detect::{
    FIRST_ALLOWANCE, reset_steps_taken, steps_taken, with_both_tiers, with_precise_tier_only,
    with_step_limit,
};

fn reported_loops(errors: &[AnalyzerError]) -> Vec<String> {
    let mut loops = errors
        .iter()
        .filter_map(|error| match error {
            AnalyzerError::CombinationalLoop { identifier, .. } => Some(identifier.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    loops.sort();
    loops
}

/// A module whose procedure needs more than `FIRST_ALLOWANCE * 8` precise
/// steps: sixteen unrolled gates copied through a runtime loop.
fn heavy_module(name: &str) -> String {
    let gates = (0..16)
        .map(|_| "if s { v = !v; } else { v = 0; }")
        .collect::<String>();
    let calls = (0..16)
        .map(|index| format!("o[{index}] = gate(1'b1, i);"))
        .collect::<String>();
    format!(
        r#"
        module {name} (i: input logic, n: input u32, o: output logic<16>) {{
            function gate (s: input logic, x: input logic) -> logic {{
                var v: logic;
                v = x;
                {gates}
                return v;
            }}
            always_comb {{ o = 0; for _iteration in 0..n {{ {calls} }} }}
        }}
        "#
    )
}

const HEAVY_LIMIT: usize = FIRST_ALLOWANCE * 8;

#[test]
fn acyclic_modules_skip_the_precise_tier() {
    with_both_tiers(|| {
        let code = r#"
            module Top (i: input logic<8>, s: input logic, o: output logic<8>) {
                var a: logic<8>;
                always_comb {
                    if s { a = i; } else { a = ~i; }
                }
                assign o = a + 1;
            }
        "#;
        reset_steps_taken();
        assert!(comb_loop_analysis_is_complete(code));
        assert_eq!(steps_taken(), 0);
        assert!(analyze(code).is_empty());
    })
}

#[test]
fn a_coarse_cycle_runs_the_precise_tier() {
    with_both_tiers(|| {
        // The regions of `a` feed each other, but no bit feeds itself: a[0] is
        // `i`, a[2] is a[0], a[1] is a[2] and a[3] is a[1].
        let code = r#"
            module Top (i: input logic, o: output logic<4>) {
                var a: logic<4>;
                assign a[1:0] = {a[2], i};
                assign a[3:2] = a[1:0];
                assign o = a;
            }
        "#;
        reset_steps_taken();
        assert!(comb_loop_analysis_is_complete(code));
        assert!(steps_taken() > 0);
        assert!(reported_loops(&analyze(code)).is_empty());
    })
}

#[test]
fn an_acyclic_module_out_of_precise_steps_is_complete() {
    with_both_tiers(|| {
        let code = heavy_module("Top");
        with_step_limit(HEAVY_LIMIT, || {
            with_precise_tier_only(|| assert!(!comb_loop_analysis_is_complete(&code)));
            assert!(comb_loop_analysis_is_complete(&code));
            assert!(reported_loops(&analyze(&code)).is_empty());
        });
    })
}

#[test]
fn an_incomplete_child_leaves_only_parents_that_need_its_summary_incomplete() {
    with_both_tiers(|| {
        let child = heavy_module("Heavy");
        let acyclic = format!(
            r#"
            {child}
            module Top (i: input logic, n: input u32, o: output logic<16>) {{
                inst heavy: Heavy (i, n, o);
            }}
            "#
        );
        // The parent closes a path through the child, so only the child's
        // precise summary could decide it.
        let cyclic = format!(
            r#"
            {child}
            module Top (n: input u32, o: output logic<16>) {{
                var x: logic;
                inst heavy: Heavy (i: x, n, o);
                assign x = o[0];
            }}
            "#
        );
        with_step_limit(HEAVY_LIMIT, || {
            with_precise_tier_only(|| assert!(!comb_loop_analysis_is_complete(&acyclic)));
            assert!(comb_loop_analysis_is_complete(&acyclic));
            assert!(!comb_loop_analysis_is_complete(&cyclic));
        });
    })
}

#[test]
fn a_cleared_child_is_summarized_for_a_precise_parent() {
    with_both_tiers(|| {
        // The child alone is acyclic. Each parent closes a coarse cycle through
        // it, which only the child's precise summary decides.
        let child = r#"
            module Swap (i: input logic<2>, o: output logic<2>) {
                assign o = {i[0], 1'b0};
            }
        "#;
        let looped = format!(
            r#"
            {child}
            module Top (o: output logic<2>) {{
                var a: logic<2>;
                var b: logic<2>;
                inst swap: Swap (i: a, o: b);
                assign a = {{1'b0, b[1]}};
                assign o = b;
            }}
            "#
        );
        let separated = format!(
            r#"
            {child}
            module Top (o: output logic<2>) {{
                var a: logic<2>;
                var b: logic<2>;
                inst swap: Swap (i: a, o: b);
                assign a = {{b[0], 1'b0}};
                assign o = b;
            }}
            "#
        );
        assert!(!reported_loops(&analyze(&looped)).is_empty());
        assert!(comb_loop_analysis_is_complete(&looped));
        assert!(reported_loops(&analyze(&separated)).is_empty());
        assert!(comb_loop_analysis_is_complete(&separated));
    })
}

#[test]
fn black_boxes_leave_modules_incomplete_without_the_precise_tier() {
    with_both_tiers(|| {
        // The precise tier adds no dependency through a black box either,
        // so the coarse tier decides both modules, and both stay incomplete.
        let code = r#"
            module Child (i: input logic, o: output logic) {
                inst ext: $sv::Ext (i_data: i, o_data: o);
            }
            module Top (i: input logic, o: output logic) {
                inst child: Child (i, o);
            }
        "#;
        reset_steps_taken();
        assert!(!comb_loop_analysis_is_complete(code));
        assert_eq!(steps_taken(), 0);
        assert!(reported_loops(&analyze(code)).is_empty());
    })
}

#[test]
fn deep_hierarchies_are_complete_beyond_the_precise_budget() {
    with_both_tiers(|| {
        const LIMIT: usize = 512;
        let mut code = String::from(
            "module Leaf (i: input logic<16>, c: input logic, o: output logic<16>) { always_comb { if c { o = i; } else { o = 0; } } }\n",
        );
        let mut previous = "Leaf".to_string();
        for level in 0..32 {
            code.push_str(&format!(
                r#"
                module Wrapper{level} (i: input logic<16>, c: input logic, o: output logic<16>) {{
                    var middle: logic<16>;
                    inst left: {previous} (i: i, c: c, o: middle);
                    inst right: {previous} (i: middle, c: c, o: o);
                }}
                "#
            ));
            previous = format!("Wrapper{level}");
        }
        with_step_limit(LIMIT, || {
            with_precise_tier_only(|| assert!(!comb_loop_analysis_is_complete(&code)));
            assert!(comb_loop_analysis_is_complete(&code));
        });
    })
}

#[test]
fn distinct_guarded_function_expansions_are_complete_beyond_the_precise_budget() {
    with_both_tiers(|| {
        const DEPTH: usize = 24;
        let mut functions = String::from(
            "function f0(x: input logic, sel: input logic) -> logic { return if sel ? x : 0; }\n",
        );
        for depth in 1..=DEPTH {
            let previous = depth - 1;
            functions.push_str(&format!(
                "function f{depth}(x: input logic, sel: input logic) -> logic {{ return f{previous}(x, sel) ^ f{previous}(x, ~sel); }}\n"
            ));
        }
        let code = format!(
            "module Top (i: input logic, s: input logic, o: output logic) {{ {functions} assign o = f{DEPTH}(i, s); }}"
        );
        with_precise_tier_only(|| assert!(!comb_loop_analysis_is_complete(&code)));
        assert!(comb_loop_analysis_is_complete(&code));
    })
}

/// Loops that reach the coarse graph only through less direct effects. The
/// precise tier deciding alone checks that the coarse graph has each loop it
/// reports, and both tiers report the same loops.
#[test]
fn indirect_effects_reach_the_coarse_graph() {
    with_both_tiers(|| {
        let cases = [
            // A call in a ternary arm writes a module variable under its
            // condition.
            r#"
            module Top (o: output logic) {
                var c: logic;
                var w: logic;
                var r: logic;
                function set () -> logic { w = 1; return 0; }
                always_comb { w = 0; r = if c ? set() : 0; }
                assign c = w;
                assign o = r;
            }
            "#,
            // A call on the right of a short-circuit operator.
            r#"
            module Top (o: output logic) {
                var c: logic;
                var w: logic;
                var r: logic;
                function set () -> logic { w = 1; return 0; }
                always_comb { w = 0; r = c && set(); }
                assign c = w;
                assign o = r;
            }
            "#,
            // An output argument writes through a selector it reads.
            r#"
            module Top (o: output logic) {
                var values: logic<2>;
                var k: logic;
                function set (value: output logic) { value = 1; }
                always_comb {
                    values = 0;
                    set(values[k]);
                    k = values[0];
                }
                assign o = k;
            }
            "#,
            // A call inside `$display` still writes.
            r#"
            module Top (o: output logic) {
                var a: logic;
                var w: logic;
                function touch (x: input logic) -> logic { w = x; return 0; }
                always_comb { $display("%d", touch(a)); }
                assign a = w;
                assign o = a;
            }
            "#,
            // A recursive call is opaque, and an opaque read anywhere in a body
            // makes the result read every argument, even when it is dropped.
            r#"
        module Top (o: output logic) {
            var a: logic;
            function f (x: input logic) -> logic {
                var t: logic;
                t = f(x);
                return 0;
            }
            assign a = f(a);
            assign o = a;
        }
        "#,
            // A nested call writes a module variable.
            r#"
            module Top (o: output logic) {
                var a: logic;
                var w: logic;
                var r: logic;
                function effect () -> logic { w = a; return 0; }
                function make () -> logic { return effect(); }
                always_comb { w = 0; r = make(); }
                assign a = w;
                assign o = r;
            }
            "#,
        ];
        for code in cases {
            let precise = with_precise_tier_only(|| reported_loops(&analyze(code)));
            assert!(!precise.is_empty(), "{code}");
            assert_eq!(reported_loops(&analyze(code)), precise, "{code}");
        }
    })
}

#[test]
fn disjoint_parts_of_one_variable_do_not_close_a_coarse_cycle() {
    with_both_tiers(|| {
        // A testbench drives a child from one element of a variable and reads it
        // back into another, as std's ecc tests do.
        let child = heavy_module("Heavy");
        let code = format!(
            r#"
            {child}
            module Top (n: input u32, o: output logic<16>) {{
                var packed: logic<2, 16>;
                var unpacked: logic<16> [2];
                inst first: Heavy (i: packed[0][0], n, o: packed[1]);
                inst second: Heavy (i: unpacked[0][0], n, o: unpacked[1]);
                assign packed[0] = 0;
                assign unpacked[0] = 0;
                assign o = packed[1] ^ unpacked[1];
            }}
            "#
        );
        with_step_limit(HEAVY_LIMIT, || {
            with_precise_tier_only(|| assert!(!comb_loop_analysis_is_complete(&code)));
            assert!(comb_loop_analysis_is_complete(&code));
        });
    })
}

#[test]
fn overlapping_parts_of_one_variable_close_a_coarse_cycle() {
    with_both_tiers(|| {
        let code = r#"
            module Top (o: output logic<4>) {
                var a: logic<4>;
                assign a[1:0] = a[3:2];
                assign a[3:2] = ~a[1:0];
                assign o = a;
            }
        "#;
        assert!(!reported_loops(&analyze(code)).is_empty());
        reset_steps_taken();
        assert!(comb_loop_analysis_is_complete(code));
        assert!(steps_taken() > 0);
    })
}

#[test]
fn calls_in_case_targets_and_loop_bounds_write_under_their_conditions() {
    // `set` writes `w` only when `c` holds, and `c` is `w`. The coarse graph
    // must see the condition, whether or not the precise tier reports it.
    for statement in [
        "case set(n) { 0: r = 1; default: r = 0; }",
        "for _i in 0..set(n) { r = 1; }",
    ] {
        let code = format!(
            r#"
            module Top (n: input logic<4>, o: output logic) {{
                var c: logic;
                var w: logic;
                var r: logic;
                function set (x: input logic<4>) -> logic<4> {{ w = 1; return x; }}
                always_comb {{ w = 0; r = 0; if c {{ {statement} }} }}
                assign c = w;
                assign o = r;
            }}
            "#
        );
        with_both_tiers(|| {
            reset_steps_taken();
            comb_loop_analysis_is_complete(&code);
            assert!(steps_taken() > 0, "{code}");
        });
    }
}

#[test]
fn a_module_beyond_precise_positions_reports_the_overflow() {
    // The coarse tier leaves positions beyond the precise tier's range to it,
    // with or without accesses, and the parent stays incomplete.
    let code = r#"
        module Huge (i: input logic, o: output logic) {
            var _value: logic [1048576, 1048576, 1048576, 8];
            assign o = i;
        }
        module Top (i: input logic, o: output logic) {
            inst huge: Huge (i, o);
        }
    "#;
    with_both_tiers(|| {
        assert!(analyze(code).iter().any(|error| matches!(
            error,
            AnalyzerError::CombinationalLoopPositionOverflow { .. }
        )));
        assert!(!comb_loop_analysis_is_complete(code));
    });
}

#[test]
fn function_fanout_summarizes_each_body_once() {
    // Each level calls the previous one twice, down to a recursive one,
    // whose call the conversion leaves opaque. A walk per call would take
    // 2^DEPTH bodies.
    const DEPTH: usize = 40;
    let mut functions =
        String::from("function level0(x: input logic) -> logic { return level0(x); }\n");
    for depth in 1..=DEPTH {
        let previous = depth - 1;
        functions.push_str(&format!(
            "function level{depth}(x: input logic) -> logic {{ return level{previous}(x) ^ level{previous}(~x); }}\n"
        ));
    }
    let code = format!(
        "module Top (i: input logic, o: output logic) {{ {functions} assign o = level{DEPTH}(i); }}"
    );
    let precise = with_precise_tier_only(|| comb_loop_analysis_is_complete(&code));
    with_both_tiers(|| {
        reset_steps_taken();
        assert_eq!(comb_loop_analysis_is_complete(&code), precise);
        assert_eq!(steps_taken(), 0);
    });
}
