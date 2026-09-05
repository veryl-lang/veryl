use super::*;

#[test]
fn comb_loop_false_negative_early_return_controls_a_later_captured_write() {
    // update(stop) leaves value at zero on the return path and writes one on
    // the continuation path. Since stop = value, the captured write is in a
    // real control-dependency loop even though the function result is constant.
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var stop : logic;
            var value: logic;
            var dummy: logic;
            function update (condition: input logic) -> logic {
                value = 0;
                if condition {
                    return 0;
                }
                value = 1;
                return 0;
            }
            assign stop = value;
            always_comb {
                dummy = update(stop);
                o = value | dummy;
            }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "an early return condition controls the captured final value: {errors:#?}"
    );
}

#[test]
fn comb_loop_condition_with_two_continuing_function_arms_does_not_control_later_write() {
    assert_comb_loop(
        "a condition does not control continuation when both function arms continue",
        r#"
        module Top (
            o: output logic,
        ) {
            var condition: logic;
            var value    : logic;
            var dummy    : logic;
            function update (select: input logic) -> logic {
                value = 0;
                if select {
                    dummy = 0;
                } else {
                    dummy = 1;
                }
                value = 1;
                return dummy;
            }
            assign condition = value;
            always_comb {
                o = update(condition);
            }
        }
        "#,
        false,
    );
}

fn assert_comb_loop(case: &str, code: &str, expected: bool) {
    let errors = analyze(code);
    let actual = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert_eq!(actual, expected, "{case}: {errors:?}");
}

fn assert_incomplete_assignment_without_comb_loop(case: &str, code: &str) {
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{case}: retained entry state must be diagnosed as an incomplete assignment: {errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{case}: state retention is not combinational feedback: {errors:#?}"
    );
}

macro_rules! comb_loop_case {
    ($name:ident, $case:literal, $code:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let code = $code;
            assert_comb_loop($case, code.as_ref(), $expected);
        }
    };
}

fn unaligned_instance_function_actual_code(actual: &str) -> String {
    format!(
        r#"
        module Child (i: input logic, o: output logic) {{ assign o = i; }}
        module Top (o: output logic) {{
            var feedback: logic;
            var passed: logic;
            function only_a (a: input logic, b: input logic) -> logic {{ return !a; }}
            inst u: Child (i: {actual}, o: passed);
            assign feedback = passed;
            assign o = passed;
        }}
        "#
    )
}

comb_loop_case!(
    comb_loop_instance_actual_function_retains_module_capture,
    "an instance actual function retains a module-scope capture",
    r#"
    module Child (i: input logic, o: output logic) { assign o = i; }
    module Top (o: output logic) {
        var x: logic;
        var passed: logic;
        function get_x () -> logic { return x; }
        inst u: Child (i: get_x(), o: passed);
        assign x = passed;
        assign o = passed;
    }
    "#,
    true
);

comb_loop_case!(
    comb_loop_instance_actual_function_keeps_disjoint_capture,
    "an instance actual function keeps a disjoint captured bit loop-free",
    r#"
    module Child (i: input logic, o: output logic) { assign o = i; }
    module Top (o: output logic) {
        var x: logic<2>;
        var passed: logic;
        function get_high () -> logic { return x[1]; }
        inst u: Child (i: get_high(), o: passed);
        assign x[0] = passed;
        assign x[1] = 0;
        assign o = passed;
    }
    "#,
    false
);

comb_loop_case!(
    comb_loop_unaligned_function_return_ignores_unused_actual,
    "an unaligned function return ignores an unused actual",
    unaligned_instance_function_actual_code("only_a(0, feedback)"),
    false
);

comb_loop_case!(
    comb_loop_unaligned_function_return_retains_used_actual,
    "an unaligned function return retains its used actual",
    unaligned_instance_function_actual_code("only_a(feedback, 0)"),
    true
);

#[test]
fn comb_loop_core_semantics_and_region_regressions_function_call_caller_side_feedthrough_links_read_x_write_x()
 {
    // Function call: caller-side feedthrough links read x -> write x.
    let code = r#"
    module ModuleA (
        a: input  logic<8>,
        b: output logic<8>,
    ) {
        function ident (
            x: input logic<8>,
        ) -> logic<8> {
            return x;
        }

        var c: logic<8>;
        assign b = ident(c);
        assign c = b;
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::CombinationalLoop { .. }));
}

#[test]
fn comb_loop_statement_order_and_observer_semantics_function_summaries_come_from_the_specialized_body_merely_evaluating_an()
 {
    // Function summaries come from the specialized body. Merely evaluating an
    // unused actual argument is not a signal-value dependency of the return.
    let code = r#"
    module Top (
        o: output logic,
    ) {
        function ignore (
            unused: input logic,
        ) -> logic {
            return 0;
        }
        var feedback: logic;
        assign o = ignore(feedback);
        assign feedback = o;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "unused function argument formed a false loop: {errors:?}"
    );
}

#[test]
fn comb_loop_function_global_read_contributes_value_dependency_a_called_function_retains_a_captured_module_scope_read()
 {
    assert_comb_loop(
        "a called function retains a captured module-scope read",
        r#"
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function get_x () -> logic {
                return x;
            }
            always_comb {
                o = get_x();
            }
            always_comb {
                x = o;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_function_global_read_contributes_value_dependency_a_captured_function_read_remains_feed_forward_without_a_return_path()
 {
    assert_comb_loop(
        "a captured function read remains feed-forward without a return path",
        r#"
        module Top (
            i: input  logic,
            o: output logic,
        ) {
            var x: logic;
            function get_x () -> logic {
                return x;
            }
            always_comb {
                o = get_x();
            }
            always_comb {
                x = i;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_function_global_write_contributes_procedural_effect_a_called_function_retains_a_captured_module_scope_write()
 {
    assert_comb_loop(
        "a called function retains a captured module-scope write",
        r#"
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function set_x (
                a: input logic,
            ) {
                x = a;
            }
            always_comb {
                set_x(o);
            }
            always_comb {
                o = x;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_function_global_write_contributes_procedural_effect_a_captured_function_write_remains_feed_forward_without_a_return_path()
 {
    assert_comb_loop(
        "a captured function write remains feed-forward without a return path",
        r#"
        module Top (
            i: input  logic,
            o: output logic,
        ) {
            var x: logic;
            function set_x (
                a: input logic,
            ) {
                x = a;
            }
            always_comb {
                set_x(i);
            }
            always_comb {
                o = x;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_vector_function_return_bits_a_vector_function_return_preserves_bit_identity()
{
    assert_comb_loop(
        "a vector function return preserves bit identity",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            function identity (
                x: input logic<2>,
            ) -> logic<2> {
                return x;
            }
            var value: logic<2>;
            assign o = identity(value);
            assign value[0] = 0;
            assign value[1] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_vector_function_return_bits_a_vector_function_return_retains_same_bit_feedback()
 {
    assert_comb_loop(
        "a vector function return retains same-bit feedback",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            function identity (
                x: input logic<2>,
            ) -> logic<2> {
                return x;
            }
            var value: logic<2>;
            assign o = identity(value);
            assign value[0] = o[0];
            assign value[1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_function_write_without_value_dependency_is_recorded() {
    // Why this case exists: clear_x writes x even though the written value has
    // no signal dependency. That write kills LiveOnEntry before o reads x;
    // omitting it invents x -> o -> x feedback.
    assert_comb_loop(
        "a constant captured function write participates in procedural order",
        r#"
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function clear_x () {
                x = 0;
            }
            always_comb {
                clear_x();
                o = x;
                x = o;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_observer_function_side_effect_is_recorded() {
    // Why this case exists: IEEE 1800-2023 11.3.5 preserves side effects of
    // evaluated expressions. touch(o) is evaluated as a display argument, and
    // 9.2.2.2 makes its captured x write part of the always_comb procedure.
    assert_comb_loop(
        "a display argument retains a called function global write",
        r#"
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function touch (
                a: input logic,
            ) -> logic {
                x = a;
                return 0;
            }
            always_comb {
                $display("touch=%d", touch(o));
            }
            always_comb {
                o = x;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_instance_actual_function_side_effect_is_recorded() {
    // Why this case exists: IEEE 1800-2023 4.9.6 models an input connection as
    // an implicit continuous assignment. Its actual expression is evaluated
    // even when the child has no output feedthrough, so touch(o) still writes x.
    assert_comb_loop(
        "an instance input actual retains a called function global write",
        r#"
        module Sink (
            i: input logic,
        ) {}
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function touch (
                a: input logic,
            ) -> logic {
                x = a;
                return 0;
            }
            inst u: Sink (
                i: touch(o),
            );
            assign o = x;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_instance_actual_preserves_dependencies_between_captured_writes() {
    assert_comb_loop(
        "an instance actual preserves dependencies between captured writes",
        r#"
        module Sink (i: input logic) {}
        module Top {
            var q: logic;
            var p: logic;
            function f (a: input logic) -> logic {
                p = q;
                q = a;
                return 0;
            }
            inst u: Sink (i: f(0));
            always_comb { q = p; }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_instance_actual_evaluates_an_unused_function_argument_once() {
    assert_comb_loop(
        "an unused outer argument retains its evaluated function side effect",
        r#"
        module Sink (
            i: input logic,
        ) {}
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function touch (
                a: input logic,
            ) -> logic {
                x = a;
                return 0;
            }
            function ignore (
                unused: input logic,
            ) -> logic {
                return 0;
            }
            inst u: Sink (
                i: ignore(touch(o)),
            );
            assign o = x;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_vector_function_output_bits() {
    // Why this case exists: output argument y is a vector identity of x.
    // Broadcasting all x bits to all y bits invents value[1] -> o[0] feedback.
    assert_comb_loop(
        "a vector function output argument preserves bit identity",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            function copy (
                x: input  logic<2>,
                y: output logic<2>,
            ) {
                y = x;
            }
            var value: logic<2>;
            always_comb {
                copy(value, o);
            }
            assign value[0] = 0;
            assign value[1] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_function_output_state_does_not_leak_between_calls() {
    assert_comb_loop(
        "a conditionally assigned function output starts with fresh state on each call",
        r#"
        module Top (
            p: output logic,
            q: output logic,
        ) {
            function f (
                x: input  logic,
                y: output logic,
            ) {
                if x {
                    y = 1;
                }
            }
            var a: logic;
            var b: logic;
            always_comb {
                f(a, p);
                f(b, q);
            }
            assign a = q;
            assign b = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_function_output_retains_same_call_control_feedback() {
    assert_comb_loop(
        "a function output retains control feedback within the same call",
        r#"
        module Top (
            p: output logic,
            q: output logic,
        ) {
            function f (
                x: input  logic,
                y: output logic,
            ) {
                if x {
                    y = 1;
                }
            }
            var a: logic;
            var b: logic;
            always_comb {
                f(a, p);
                f(b, q);
            }
            assign a = 0;
            assign b = q;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_function_local_state_does_not_leak_between_calls() {
    assert_comb_loop(
        "a function local starts with fresh state on each call",
        r#"
        module Top (
            p: output logic,
            q: output logic,
        ) {
            function f (
                x: input  logic,
                y: output logic,
            ) {
                var temporary: logic;
                if x {
                    temporary = 1;
                }
                y = temporary;
            }
            var a: logic;
            var b: logic;
            always_comb {
                f(a, p);
                f(b, q);
            }
            assign a = q;
            assign b = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_function_return_state_does_not_leak_between_calls() {
    assert_comb_loop(
        "a function return starts with fresh state on each call",
        r#"
        module Top (
            p: output logic,
            q: output logic,
        ) {
            function f (
                x: input logic,
            ) -> logic {
                if x {
                    return 1;
                }
            }
            var a: logic;
            var b: logic;
            assign p = f(a);
            assign q = f(b);
            assign a = q;
            assign b = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_wide_function_output_bits_without_scalarization() {
    // Why this case exists: function boundary precision must come from
    // observed endpoint propagation, not a width-limited per-bit expansion.
    assert_comb_loop(
        "a wide function output keeps disjoint endpoint bits independent",
        r#"
        module Top (
            o: output logic<128>,
        ) {
            function copy (
                x: input  logic<128>,
                y: output logic<128>,
            ) {
                y = x;
            }
            var value: logic<128>;
            always_comb {
                copy(value, o);
            }
            assign value[126:0] = 0;
            assign value[127] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_wide_function_output_retains_matching_endpoint_feedback() {
    assert_comb_loop(
        "a wide function output retains feedback at the matching endpoint",
        r#"
        module Top (
            o: output logic<128>,
        ) {
            function copy (
                x: input  logic<128>,
                y: output logic<128>,
            ) {
                y = x;
            }
            var value: logic<128>;
            always_comb {
                copy(value, o);
            }
            assign value[126:0] = 0;
            assign value[127] = o[127];
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_split_destination_reuses_one_function_evaluation() {
    const WIDTH: usize = 256;
    let observed_bits = (0..WIDTH)
        .map(|bit| format!("assign observed[{bit}] = result[{bit}];"))
        .collect::<Vec<_>>()
        .join("\n");
    crate::comb_loop_detect::reset_function_evaluation_count();
    let errors = analyze(&format!(
        r#"
        module Top (
            i       : input  logic<{WIDTH}>,
            observed: output logic<{WIDTH}>,
        ) {{
            function identity (
                x: input logic<{WIDTH}>,
            ) -> logic<{WIDTH}> {{
                return x;
            }}
            var result: logic<{WIDTH}>;
            assign result = identity(i);
            {observed_bits}
        }}
        "#
    ));
    assert!(
        errors.is_empty(),
        "split observation of one function result is acyclic: {errors:#?}"
    );
    assert_eq!(
        crate::comb_loop_detect::function_evaluation_count(),
        1,
        "splitting the destination must not reevaluate the same function call"
    );
    assert_eq!(
        crate::comb_loop_detect::function_result_version_count(),
        WIDTH,
        "each split destination must request only its matching return region"
    );
    assert!(
        crate::comb_loop_detect::function_result_region_probe_count() <= WIDTH * 12,
        "return-region lookup must be logarithmic rather than scanning all regions per bit"
    );
}

#[test]
fn comb_loop_static_loop_reevaluates_nested_function_actuals() {
    crate::comb_loop_detect::reset_function_evaluation_count();
    assert_comb_loop(
        "a nested call is reevaluated when its static-loop actual changes",
        r#"
        module Top (
            o: output logic,
        ) {
            function inner (
                x: input logic,
            ) -> logic {
                return x;
            }
            function outer (
                x: input logic<2>,
            ) -> logic {
                var result: logic;
                result = 0;
                for i in 0..2 {
                    if inner(x[i]) {
                        result = 1;
                    } else {
                        result = 0;
                    }
                }
                return result;
            }
            var feedback: logic;
            assign o = outer({feedback, 1'b0});
            assign feedback = o;
        }
        "#,
        true,
    );
    assert_eq!(
        crate::comb_loop_detect::function_barrier_evaluation_count(),
        2,
        "both static-loop invocations must cross the callee cache barrier"
    );
}

#[test]
fn comb_loop_preserves_split_function_return_bits() {
    // Why this case exists: {high, low}[0] is low. Returning o[0] to high is
    // acyclic when low is constant, even though the return uses two regions.
    assert_comb_loop(
        "a concatenated function return preserves each source bit",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            function combine_bits (
                high: input logic,
                low : input logic,
            ) -> logic<2> {
                return {high, low};
            }
            var high: logic;
            var low: logic;
            assign o = combine_bits(high, low);
            assign high = o[0];
            assign low = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_distinguishes_generic_function_specializations() {
    // Why this case exists: recurse::<2> and recurse::<1> are distinct finite
    // specializations. Elaboration reduces the call to passed = feedback, so
    // treating the second specialization as infinite recursion hides a real SCC.
    let errors = analyze_with_large_stack(
        r#"
        module Top (
            o: output logic,
        ) {
            function recurse::<N: u32> (
                x: input logic,
            ) -> logic {
                gen M: u32 = N - 1;
                if N == 1 {
                    return x;
                } else {
                    return recurse::<M>(x);
                }
            }
            var feedback: logic;
            var passed: logic;
            assign passed = recurse::<2>(feedback);
            assign feedback = passed;
            assign o = feedback;
        }
        "#,
    );
    let actual = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert!(
        actual,
        "finite generic recursion retains the specialized feedthrough: {errors:?}"
    );
}

#[test]
fn function_summary_shift_fanout_stays_structural() {
    const DEPTH: usize = 14;
    const WIDTH: usize = 1 << DEPTH;
    let mut functions =
        format!("function f0 (x: input logic<{WIDTH}>) -> logic<{WIDTH}> {{ return x; }}\n");
    for depth in 1..=DEPTH {
        let previous = depth - 1;
        let shift = 1usize << previous;
        functions.push_str(&format!(
            "function f{depth} (x: input logic<{WIDTH}>) -> logic<{WIDTH}> {{ return f{previous}(x) | (f{previous}(x) << {shift}); }}\n"
        ));
    }
    let code = format!(
        "module Top (i: input logic<{WIDTH}>, o: output logic<{WIDTH}>) {{ {functions} assign o = f{DEPTH}(i); }}"
    );
    crate::comb_loop_detect::reset_function_evaluation_count();
    assert!(
        analyze(&code)
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. }))
    );
    assert!(crate::comb_loop_detect::function_summary_graph_node_count() < 100);
}

#[test]
fn function_summary_converted_input_fanout_stays_structural() {
    // Copy-in conversion creates separate SSA versions for each call. The
    // dependency graph must recognize equivalent conversions before importing
    // the callee, while preserving different actuals and their positions.
    const DEPTH: usize = 15;
    for (left, right) in [
        ("x", "x"),
        ("x as u8", "x as u8"),
        ("x as i8", "x as i8"),
        ("x", "x << 1"),
    ] {
        let mut functions = String::from("function f0(x: input i16) -> i16 { return x; }\n");
        for depth in 1..=DEPTH {
            let previous = depth - 1;
            functions.push_str(&format!(
                "function f{depth}(x: input i16) -> i16 {{ return f{previous}({left}) | f{previous}({right}); }}\n"
            ));
        }
        let code = format!(
            "module Top(i: input i16, o: output i16) {{ {functions} assign o = f{DEPTH}(i); }}"
        );
        crate::comb_loop_detect::reset_function_evaluation_count();
        let errors = analyze(&code);
        assert!(errors.is_empty(), "{left}, {right}: {errors:?}");
        let nodes = crate::comb_loop_detect::function_summary_graph_node_count();
        assert!(
            nodes < DEPTH * DEPTH * 4,
            "converted or shifted actuals must retain sharing: {left}, {right}: {nodes} nodes"
        );
    }
}

#[test]
fn function_summary_shared_conversions_preserve_distinct_actuals() {
    for output_argument in [false, true] {
        for runtime in [false, true] {
            for selected in ["first", "second"] {
                for bit in [0, 15] {
                    let (function, first, second) = if output_argument {
                        (
                            "function wrap(x: input i16, y: output i16) { y = id(x as i8) | id(x as i8); }",
                            "wrap(source as i8, first);",
                            "wrap(source as i8, second);",
                        )
                    } else {
                        (
                            "function wrap(x: input i16) -> i16 { return id(x as i8) | id(x as i8); }",
                            "first = wrap(source as i8);",
                            "second = wrap(source as i8);",
                        )
                    };
                    let body = format!(
                        "source = {{feedback, 7'b0}} as i8; {first} source = {{external, 7'b0}} as i8; {second}"
                    );
                    let body = if runtime {
                        format!("for _index in 0..n {{ {body} }}")
                    } else {
                        body
                    };
                    let input = if runtime { "n: input u32," } else { "" };
                    let code = format!(
                        "module Top({input} external: input logic, o: output i16) {{
                            function id(x: input i16) -> i16 {{ return x; }}
                            {function}
                            var source: i8;
                            var first: i16;
                            var second: i16;
                            var feedback: logic;
                            always_comb {{
                                source = 0; first = 0; second = 0;
                                {body}
                            }}
                            assign feedback = {selected}[{bit}];
                            assign o = first | second;
                        }}"
                    );
                    // The first call samples feedback; the second samples a
                    // new SSA value. Only their sign-extension bits depend on
                    // those inputs. Reusing the callee must not merge them.
                    let errors = analyze(&code);
                    assert!(
                        errors
                            .iter()
                            .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
                    );
                    let has_loop = !errors.is_empty();
                    assert_eq!(
                        has_loop,
                        selected == "first" && bit == 15,
                        "output_argument={output_argument}, runtime={runtime}, {selected}[{bit}]: {errors:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn function_summary_rejects_a_cycle_whose_intermediate_shift_is_out_of_range() {
    assert_comb_loop(
        "opposite offsets do not form a loop when the intermediate value is outside the vector",
        r#"
        module Top (o: output logic) {
            function left (x: input logic<2>) -> logic<2> { return x << 1; }
            function right (x: input logic<2>) -> logic<2> { return x >> 1; }
            var a: logic<2>;
            var b: logic<2>;
            var c: logic<2>;
            assign a = left(b);
            assign c = right(a);
            assign b[1] = c[1];
            assign b[0] = 0;
            assign o = a[0] | b[0] | c[0];
        }
        "#,
        false,
    );
}

#[test]
fn function_summary_retains_a_cycle_with_feasible_intermediate_shifts() {
    assert_comb_loop(
        "opposite offsets retain the low-bit path that stays inside the vector",
        r#"
        module Top (o: output logic) {
            function left (x: input logic<2>) -> logic<2> { return x << 1; }
            function right (x: input logic<2>) -> logic<2> { return x >> 1; }
            var a: logic<2>;
            var b: logic<2>;
            var c: logic<2>;
            assign a = left(b);
            assign c = right(a);
            assign b[0] = c[0];
            assign b[1] = 0;
            assign o = a[0] | b[0] | c[0];
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_short_circuits_instance_actual_side_effects_a_constant_dead_instance_actual_branch_has_no_function_side_effect()
 {
    assert_comb_loop(
        "a constant-dead instance actual branch has no function side effect",
        r#"
        module Sink (
            i: input logic,
        ) {}
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function touch (
                a: input logic,
            ) -> logic {
                x = a;
                return 0;
            }
            inst u: Sink (
                i: if 1'b1 ? 0 : touch(o),
            );
            assign o = x;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_short_circuits_instance_actual_side_effects_a_constant_taken_instance_actual_branch_retains_its_function_side_effect()
 {
    assert_comb_loop(
        "a constant-taken instance actual branch retains its function side effect",
        r#"
        module Sink (
            i: input logic,
        ) {}
        module Top (
            o: output logic,
        ) {
            var x: logic;
            function touch (
                a: input logic,
            ) -> logic {
                x = a;
                return 0;
            }
            inst u: Sink (
                i: if 1'b0 ? 0 : touch(o),
            );
            assign o = x;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_ternary_positions_across_boundaries_a_function_ternary_actual_keeps_a_disjoint_bit_loop_free()
 {
    assert_comb_loop(
        "a function ternary actual keeps a disjoint bit loop-free",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            function low (
                x: input logic<2>,
            ) -> logic {
                return x[0];
            }
            var value: logic<2>;
            assign o = low(if sel ? value : 0);
            assign value[0] = 0;
            assign value[1] = o;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_ternary_positions_across_boundaries_a_function_ternary_actual_detects_its_corresponding_bit_loop()
 {
    assert_comb_loop(
        "a function ternary actual detects its corresponding-bit loop",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            function low (
                x: input logic<2>,
            ) -> logic {
                return x[0];
            }
            var value: logic<2>;
            assign o = low(if sel ? value : 0);
            assign value[0] = o;
            assign value[1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_unpacked_function_actual_positions_an_unpacked_function_actual_keeps_disjoint_elements_loop_free()
 {
    assert_comb_loop(
        "an unpacked function actual keeps disjoint elements loop-free",
        r#"
        module Top (
            o: output logic,
        ) {
            function high (
                x: input logic [2],
            ) -> logic {
                return x[1];
            }
            var value: logic [2];
            assign o = high(value);
            assign value[0] = o;
            assign value[1] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_unpacked_function_actual_positions_an_unpacked_function_actual_detects_same_element_feedback()
 {
    assert_comb_loop(
        "an unpacked function actual detects same-element feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            function high (
                x: input logic [2],
            ) -> logic {
                return x[1];
            }
            var value: logic [2];
            assign o = high(value);
            assign value[0] = 0;
            assign value[1] = o;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_function_actual_shift_positions_a_function_actual_left_shift_keeps_its_inserted_bit_loop_free()
 {
    assert_comb_loop(
        "a function actual left shift keeps its inserted bit loop-free",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (
                x: input logic<4>,
            ) -> logic {
                return x[0];
            }
            var value: logic<4>;
            assign o = low(value << 1);
            assign value[0] = 0;
            assign value[1] = o;
            assign value[3:2] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_function_actual_shift_positions_a_function_actual_left_shift_detects_its_live_shifted_bit()
 {
    assert_comb_loop(
        "a function actual left shift detects its live shifted bit",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_one (
                x: input logic<4>,
            ) -> logic {
                return x[1];
            }
            var value: logic<4>;
            assign o = bit_one(value << 1);
            assign value[0] = o;
            assign value[3:1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_function_concat_output_positions_a_concatenated_function_output_keeps_a_disjoint_bit_loop_free()
 {
    assert_comb_loop(
        "a concatenated function output keeps a disjoint bit loop-free",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            function copy (
                x: input  logic<2>,
                y: output logic<2>,
            ) {
                y = x;
            }
            var value: logic<2>;
            always_comb {
                copy(value, {o[1], o[0]});
            }
            assign value[0] = 0;
            assign value[1] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_function_concat_output_positions_a_concatenated_function_output_detects_its_corresponding_bit_loop()
 {
    assert_comb_loop(
        "a concatenated function output detects its corresponding-bit loop",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            function copy (
                x: input  logic<2>,
                y: output logic<2>,
            ) {
                y = x;
            }
            var value: logic<2>;
            always_comb {
                copy(value, {o[1], o[0]});
            }
            assign value[0] = o[0];
            assign value[1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_function_local_partial_writes_are_ordered()
 {
    assert_comb_loop(
        "function local partial writes are ordered",
        r#"
        module Top (
            d: input  logic<8>,
            q: output logic<8>,
        ) {
            function swap_nibbles (
                x: input logic<8>,
            ) -> logic<8> {
                var tmp: logic<8>;
                tmp[7:4] = x[3:0];
                tmp[3:0] = x[7:4];
                return tmp;
            }
            assign q = swap_nibbles(d);
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_nested_function_summary_preserves_a_real_cycle()
 {
    assert_comb_loop(
        "nested function summary preserves a real cycle",
        r#"
        module Top (
            o: output logic,
        ) {
            function inner (x: input logic) -> logic {
                return x;
            }
            function outer (x: input logic) -> logic {
                return inner(x);
            }
            assign o = outer(o);
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_blocking_assignment_chain_uses_the_immediately_preceding_definition()
 {
    assert_comb_loop(
        "blocking assignment chain uses the immediately preceding definition",
        r#"
        module Top (
            a: input  logic<8>,
            o: output logic<8>,
        ) {
            var tmp: logic<8>;
            always_comb {
                tmp = a;
                tmp = tmp + 8'd1;
                tmp = tmp << 1;
                o = tmp;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_a_later_full_overwrite_kills_an_earlier_partial_entry_read()
 {
    assert_comb_loop(
        "a later full overwrite kills an earlier partial entry read",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            always_comb {
                o[0] = o[1];
                o = 0;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_a_full_overwrite_dominates_a_later_partial_read()
 {
    assert_comb_loop(
        "a full overwrite dominates a later partial read",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            always_comb {
                o = 0;
                o[0] = o[1];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_both_branch_arms_define_the_value_consumed_after_the_merge()
 {
    assert_comb_loop(
        "both branch arms define the value consumed after the merge",
        r#"
        module Top (
            sel: input  logic,
            a  : input  logic,
            b  : input  logic,
            o  : output logic,
        ) {
            var selected: logic;
            always_comb {
                if sel {
                    selected = a;
                } else {
                    selected = b;
                }
                o = selected;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_function_local_copy_retains_bit_precision_at_the_return()
 {
    assert_comb_loop(
        "function local copy retains bit precision at the return",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (x: input logic<8>) -> logic {
                var tmp: logic<8>;
                tmp = x;
                return tmp[0];
            }
            var value: logic<8>;
            assign o = low(value);
            assign value[7] = o;
            assign value[6:0] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_function_branch_condition_is_a_control_dependency_of_its_return()
 {
    assert_comb_loop(
        "function branch condition is a control dependency of its return",
        r#"
        module Top (
            o: output logic,
        ) {
            function gated (x: input logic<8>) -> logic {
                if x[7] {
                    return x[0];
                } else {
                    return 0;
                }
            }
            var value: logic<8>;
            assign o = gated(value);
            assign value[7] = o;
            assign value[6:0] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_function_branch_ignores_a_bit_absent_from_value_and_control_flow()
 {
    assert_comb_loop(
        "function branch ignores a bit absent from value and control flow",
        r#"
        module Top (
            o: output logic,
        ) {
            function gated (x: input logic<8>) -> logic {
                if x[7] {
                    return x[0];
                } else {
                    return 0;
                }
            }
            var value: logic<8>;
            assign o = gated(value);
            assign value[6] = o;
            assign value[7] = 0;
            assign value[5:0] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_function_output_writeback_participates_in_procedural_order()
 {
    assert_comb_loop(
        "function output writeback participates in procedural order",
        r#"
        module Top (
            o: output logic,
        ) {
            function copy (
                x: input  logic,
                y: output logic,
            ) {
                y = x;
            }
            var tmp: logic;
            always_comb {
                copy(o, tmp);
                o = tmp;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_static_array_elements_remain_distinct_regions()
{
    assert_comb_loop(
        "static array elements remain distinct regions",
        r#"
        module Top (
            a: input  logic<8>,
            o: output logic<8>,
        ) {
            var mem: logic<8> [2];
            always_comb {
                mem[0] = a;
                mem[1] = mem[0];
                o = mem[1];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_read_before_write_across_static_array_elements_is_a_real_loop()
 {
    assert_comb_loop(
        "read-before-write across static array elements is a real loop",
        r#"
        module Top (
            o: output logic<8>,
        ) {
            var mem: logic<8> [2];
            always_comb {
                mem[0] = mem[1];
                mem[1] = mem[0];
                o = mem[1];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_static_struct_members_remain_distinct_regions()
{
    assert_comb_loop(
        "static struct members remain distinct regions",
        r#"
        module Top (
            a: input  logic<8>,
            o: output logic<8>,
        ) {
            struct Pair {
                low : logic<8>,
                high: logic<8>,
            }
            var pair: Pair;
            always_comb {
                pair.low = a;
                pair.high = pair.low;
                o = pair.high;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_sparse_accesses_do_not_scale_with_a_huge_declared_width()
 {
    assert_comb_loop(
        "sparse accesses do not scale with a huge declared width",
        r#"
        module Top (
            a: input  logic,
            o: output logic,
        ) {
            var huge: logic<1000000>;
            always_comb {
                huge[0] = a;
                o = huge[999999];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_region_and_function_mapping_regressions_dynamic_same_object_aliasing_uses_the_whole_longest_static_prefix()
 {
    assert_comb_loop(
        "dynamic same-object aliasing uses the whole longest static prefix",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic,
        ) {
            var values: logic [4];
            always_comb {
                values[index] = o;
                o = values[0];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_function_bit_select_must_not_taint_a_disjoint_actual_bit()
 {
    assert_comb_loop(
        "function bit-select must not taint a disjoint actual bit",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_zero (x: input logic<8>) -> logic {
                return x[0];
            }
            var value: logic<8>;
            assign o = bit_zero(value);
            assign value[0] = 0;
            assign value[7] = o;
            assign value[6:1] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_function_bit_select_must_retain_same_bit_feedback()
{
    assert_comb_loop(
        "function bit-select must retain same-bit feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_zero (x: input logic<8>) -> logic {
                return x[0];
            }
            var value: logic<8>;
            assign o = bit_zero(value);
            assign value[0] = o;
            assign value[7:1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_function_bit_select_through_concatenation_ignores_high_operands()
 {
    assert_comb_loop(
        "function bit-select through concatenation ignores high operands",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_zero (x: input logic<8>) -> logic {
                return x[0];
            }
            var value: logic<7>;
            assign o = bit_zero({value, 1'b0});
            assign value[6] = o;
            assign value[5:0] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_function_bit_select_through_concatenation_retains_low_operand()
 {
    assert_comb_loop(
        "function bit-select through concatenation retains low operand",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_zero (x: input logic<8>) -> logic {
                return x[0];
            }
            assign o = bit_zero({7'b0, o});
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_function_bit_select_through_an_actual_slice_uses_its_low_bit()
 {
    assert_comb_loop(
        "function bit-select through an actual slice uses its low bit",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_zero (x: input logic<8>) -> logic {
                return x[0];
            }
            var value: logic<16>;
            assign o = bit_zero(value[15:8]);
            assign value[15] = o;
            assign value[14:0] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_function_bit_select_through_an_actual_slice_retains_its_low_bit_feedback()
 {
    assert_comb_loop(
        "function bit-select through an actual slice retains its low-bit feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_zero (x: input logic<8>) -> logic {
                return x[0];
            }
            var value: logic<16>;
            assign o = bit_zero(value[15:8]);
            assign value[8] = o;
            assign value[15:9] = 0;
            assign value[7:0] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_function_region_crossing_a_concatenation_boundary_retains_both_operands()
 {
    assert_comb_loop(
        "function region crossing a concatenation boundary retains both operands",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            function middle (x: input logic<8>) -> logic<2> {
                return x[4:3];
            }
            var high: logic<4>;
            var low : logic<4>;
            assign o = middle({high, low});
            assign high[0] = o[1];
            assign high[3:1] = 0;
            assign low = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_function_capture_coverage_obeys_caller_order() {
    // Why this case exists: a function's module-scope write is part of its
    // caller procedure. A dominating default or a later full write supplies
    // every preserved bit, so function-local weak-write coverage must not be
    // finalized before the caller MemorySSA reaches its exit.
    for body in [
        "value = 0; write_selected(index);",
        "write_selected(index); value = 0;",
    ] {
        let errors = analyze(&format!(
            r#"
            module Top (
                index: input  logic<2>,
                o    : output logic,
            ) {{
                var value: logic<4>;
                function write_selected (
                    index: input logic<2>,
                ) {{
                    value[index] = 1;
                }}
                always_comb {{
                    {body}
                    o = value[0];
                }}
            }}
            "#
        ));
        assert!(errors.is_empty(), "caller ordering is valid: {errors:#?}");
    }
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: captured function write"]
fn comb_loop_function_capture_without_default_retains_coverage() {
    // Why this case exists: the caller-order kill controls above need a
    // positive control. Without a caller default, a captured dynamic write
    // still leaves unselected bits unassigned at the always_comb exit.
    assert_incomplete_assignment_without_comb_loop(
        "a captured weak write without a caller default remains incomplete",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic,
        ) {
            var value: logic<4>;
            function write_selected (
                index: input logic<2>,
            ) {
                value[index] = 1;
            }
            always_comb {
                write_selected(index);
                o = value[0];
            }
        }
        "#,
    );
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: uncalled function output"]
fn comb_loop_uncalled_function_still_checks_output_coverage() {
    // Why this case exists: output-argument completeness is a property of the
    // function definition, not of whether an always_comb happens to call it.
    // A runtime loop may execute zero times and leave the output unassigned.
    assert_incomplete_assignment_without_comb_loop(
        "an uncalled function still has to assign its output on every path",
        r#"
        module Top (
            o: output logic,
        ) {
            function maybe_write (
                n    : input  logic<32>,
                value: output logic,
            ) {
                for _index in 0..n {
                    value = 1;
                }
            }
            assign o = 0;
        }
        "#,
    );
}

#[test]
fn comb_loop_runtime_vector_copy_preserves_packed_positions() {
    for bound in ["none", "1", "n"] {
        for operation in [
            "result = source;",
            "result = identity(source);",
            "copy(source, result);",
            "result[0] = source[0]; result[1] = source[1];",
        ] {
            for bit in [0, 1] {
                let body = if bound == "none" {
                    operation.to_owned()
                } else {
                    format!("for _index in 0..{bound} {{ {operation} }}")
                };
                let code = format!(
                    r#"
                    module Top(n: input u32, external: input logic, o: output logic<2>) {{
                        var feedback: logic;
                        var source: logic<2>;
                        var result: logic<2>;
                        function identity(x: input logic<2>) -> logic<2> {{ return x; }}
                        function copy(x: input logic<2>, y: output logic<2>) {{ y = x; }}
                        assign source = {{feedback, external}};
                        always_comb {{
                            result = 0;
                            {body}
                        }}
                        assign feedback = result[{bit}];
                        assign o = result;
                    }}
                    "#
                );
                let errors = analyze(&code);
                assert!(
                    errors.iter().all(|error| matches!(
                        error,
                        AnalyzerError::CombinationalLoop { .. }
                            | AnalyzerError::UnusedVariable { .. }
                    )),
                    "{errors:#?}"
                );
                assert_eq!(
                    errors
                        .iter()
                        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                    bit == 1,
                    "bound={bound}, operation={operation}, bit={bit}: {errors:#?}",
                );
                assert!(comb_loop_analysis_is_complete(&code));
            }
        }
    }
}

#[test]
fn comb_loop_runtime_positional_dependencies_cross_iterations() {
    for (initialize, operation, feedback_bit) in [
        ("middle = 0;", "result = middle >> 1; middle = source;", 2),
        ("middle = 0;", "result = shift(middle); middle = source;", 2),
        (
            "middle = 0;",
            "shift_out(middle, result); middle = source;",
            2,
        ),
        (
            "middle = source;",
            "result = identity(middle); middle = result;",
            3,
        ),
        (
            "middle = source;",
            "copy(middle, result); middle = result;",
            3,
        ),
    ] {
        for bit in [0, feedback_bit] {
            let code = format!(
                r#"
                module Top(n: input u32, external: input logic, o: output logic<4>) {{
                    var feedback: logic;
                    var source: logic<4>;
                    var middle: logic<4>;
                    var result: logic<4>;
                    function identity(x: input logic<4>) -> logic<4> {{ return x; }}
                    function copy(x: input logic<4>, y: output logic<4>) {{ y = x; }}
                    function shift(x: input logic<4>) -> logic<4> {{ return x >> 1; }}
                    function shift_out(x: input logic<4>, y: output logic<4>) {{ y = x >> 1; }}
                    assign source = {{feedback, external repeat 3}};
                    always_comb {{
                        {initialize}
                        result = 0;
                        for _index in 0..n {{ {operation} }}
                    }}
                    assign feedback = result[{bit}];
                    assign o = result;
                }}
                "#
            );
            let errors = analyze(&code);
            assert!(
                errors
                    .iter()
                    .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                "{errors:#?}"
            );
            assert_eq!(
                errors
                    .iter()
                    .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                bit == feedback_bit,
                "operation={operation}, bit={bit}: {errors:#?}",
            );
            assert!(comb_loop_analysis_is_complete(&code));
        }
    }
}

#[test]
fn comb_loop_runtime_function_copy_preserves_array_positions() {
    for operation in ["result = identity(source);", "copy(source, result);"] {
        for element in [0, 1] {
            let code = format!(
                r#"
                module Top(n: input u32, external: input logic, o: output logic[2]) {{
                    type Pair = logic[2];
                    var feedback: logic;
                    var source: Pair;
                    var result: Pair;
                    function identity(x: input Pair) -> Pair {{ return x; }}
                    function copy(x: input Pair, y: output Pair) {{ y = x; }}
                    assign source = '{{external, feedback}};
                    always_comb {{
                        result = '{{default: 0}};
                        for _index in 0..n {{ {operation} }}
                    }}
                    assign feedback = result[{element}];
                    assign o = result;
                }}
                "#
            );
            let errors = analyze(&code);
            assert!(
                errors
                    .iter()
                    .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                "{errors:#?}"
            );
            assert_eq!(
                errors
                    .iter()
                    .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                element == 1,
                "operation={operation}, element={element}: {errors:#?}",
            );
            assert!(comb_loop_analysis_is_complete(&code));
        }
    }
}

#[test]
fn comb_loop_runtime_dynamic_array_write_retains_packed_positions() {
    for row in 0..4 {
        for bit in [0, 1] {
            let code = format!(
                r#"
                module Top(n: input u32, external: input logic, o: output logic) {{
                    var value: logic<2>[4, 2];
                    var feedback: logic;
                    assign feedback = value[{row}][0][{bit}];
                    always_comb {{
                        value = '{{default: 0}};
                        for index in 0..n {{ value[index][0] = {{feedback, external}}; }}
                        o = feedback;
                    }}
                }}
                "#
            );
            let errors = analyze(&code);
            assert!(
                errors
                    .iter()
                    .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                "{errors:#?}"
            );
            assert_eq!(
                errors
                    .iter()
                    .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                bit == 1,
                "row={row}, bit={bit}: {errors:#?}",
            );
            assert!(comb_loop_analysis_is_complete(&code));
        }
    }
}

#[test]
fn comb_loop_runtime_function_outputs_remain_independent() {
    for bound in ["none", "1", "n"] {
        for called in [false, true] {
            for source in ["external", "feedback"] {
                let operation = if called {
                    format!("split(feedback, {source}, scratch, result);")
                } else {
                    format!("scratch = feedback; result = {source};")
                };
                let body = if bound == "none" {
                    operation
                } else {
                    format!("for _index in 0..{bound} {{ {operation} }}")
                };
                let code = format!(
                    r#"
                    module Top(n: input u32, external: input logic, o: output logic) {{
                        var feedback: logic;
                        var result: logic;
                        var scratch: logic;
                        function split(a: input logic, b: input logic,
                                       first: output logic, second: output logic) {{
                            first = a;
                            second = b;
                        }}
                        always_comb {{
                            result = 0;
                            scratch = 0;
                            {body}
                        }}
                        assign feedback = result;
                        assign o = scratch;
                    }}
                    "#
                );
                let errors = analyze(&code);
                assert!(
                    errors.iter().all(|error| matches!(
                        error,
                        AnalyzerError::CombinationalLoop { .. }
                            | AnalyzerError::UnusedVariable { .. }
                    )),
                    "{errors:#?}"
                );
                assert_eq!(
                    errors
                        .iter()
                        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                    source == "feedback",
                    "bound={bound}, called={called}, source={source}: {errors:#?}",
                );
                assert!(comb_loop_analysis_is_complete(&code));
            }
        }
    }
}

#[test]
fn comb_loop_runtime_function_return_excludes_unrelated_captured_write() {
    for bound in ["none", "1", "n"] {
        for source in ["external", "feedback", "1'b0"] {
            let operation = format!("result = split(feedback, {source});");
            let body = if bound == "none" {
                operation
            } else {
                format!("for _index in 0..{bound} {{ {operation} }}")
            };
            let code = format!(
                r#"
                module Top(n: input u32, external: input logic, o: output logic) {{
                    var feedback: logic;
                    var result: logic;
                    var scratch: logic;
                    function split(a: input logic, b: input logic) -> logic {{
                        scratch = a;
                        return b;
                    }}
                    always_comb {{
                        result = 0;
                        scratch = 0;
                        {body}
                    }}
                    assign feedback = result;
                    assign o = scratch;
                }}
                "#
            );
            let errors = analyze(&code);
            assert!(
                errors.iter().all(|error| matches!(
                    error,
                    AnalyzerError::CombinationalLoop { .. } | AnalyzerError::UnusedVariable { .. }
                )),
                "{errors:#?}"
            );
            assert_eq!(
                errors
                    .iter()
                    .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                source == "feedback",
                "bound={bound}, source={source}: {errors:#?}",
            );
            assert!(comb_loop_analysis_is_complete(&code));
        }
    }
}

#[test]
fn comb_loop_function_summary_fanout_is_memoized() {
    // Why this case exists: each function calls the previous specialization
    // twice, so per-call recursive analysis grows as 2^N. The source contains
    // only N unique function bodies and must be analyzed in O(N) summaries.
    let mut functions = String::from(
        r#"
        function f0 (
            x: input logic,
        ) -> logic {
            return x;
        }
        "#,
    );
    for depth in 1..=14 {
        functions.push_str(&format!(
            r#"
            function f{depth} (
                x: input logic,
            ) -> logic {{
                return f{previous}(x) ^ f{previous}(x);
            }}
            "#,
            previous = depth - 1,
        ));
    }
    crate::comb_loop_detect::reset_function_evaluation_count();
    let errors = analyze(&format!(
        r#"
        module Top (
            i: input  logic,
            o: output logic,
        ) {{
            {functions}
            assign o = f14(i);
        }}
        "#
    ));
    assert!(
        errors.is_empty(),
        "acyclic function fanout is valid: {errors:#?}"
    );
    assert_eq!(crate::comb_loop_detect::function_evaluation_count(), 29);
}

#[test]
fn function_summaries_reuse_module_metadata() {
    const COUNT: usize = 16;
    let padding = (0..COUNT)
        .map(|index| format!("var padding_{index}: logic;"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut functions = String::new();
    let mut calls = String::new();
    for index in 0..COUNT {
        functions.push_str(&format!(
            "function function_{index} (value: input logic) -> logic {{ return value; }}\n"
        ));
        calls.push_str(&format!("padding_{index} = function_{index}(seed);\n"));
    }
    let code = format!(
        r#"
        module Top (seed: input logic, o: output logic) {{
            {padding}
            {functions}
            always_comb {{
                {calls}
                o = padding_0;
            }}
        }}
        "#
    );

    crate::comb_loop_detect::reset_module_context_entries();
    let errors = analyze(&code);
    assert!(
        errors.is_empty(),
        "independent calls are acyclic: {errors:#?}"
    );
    assert!(
        crate::comb_loop_detect::module_context_entries() <= COUNT * 6 + 4,
        "function summaries must share their module metadata: {}",
        crate::comb_loop_detect::module_context_entries(),
    );
}

#[test]
fn comb_loop_early_return_excludes_unreachable_function_dependency() {
    assert_comb_loop(
        "a return makes the following function dependency unreachable",
        r#"
        module Top (
            o: output logic,
        ) {
            function choose (
                feedback: input logic,
            ) -> logic {
                return 0;
                return feedback;
            }
            var feedback: logic;
            assign o = choose(feedback);
            assign feedback = o;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_early_branch_return_preserves_its_reachable_dependency() {
    assert_comb_loop(
        "an early branch return remains an alternative to the fallback return",
        r#"
        module Top (
            select: input  logic,
            o     : output logic,
        ) {
            function choose (
                select  : input logic,
                feedback: input logic,
            ) -> logic {
                if select {
                    return feedback;
                }
                return 0;
            }
            var feedback: logic;
            assign o = choose(select, feedback);
            assign feedback = o;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_all_branch_returns_exclude_following_dependency() {
    assert_comb_loop(
        "no path reaches a statement after both branches return",
        r#"
        module Top (
            select: input  logic,
            o     : output logic,
        ) {
            function choose (
                select  : input logic,
                feedback: input logic,
            ) -> logic {
                if select {
                    return 0;
                } else {
                    return 0;
                }
                return feedback;
            }
            var feedback: logic;
            assign o = choose(select, feedback);
            assign feedback = o;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_branch_returns_preserve_reachable_function_dependency() {
    assert_comb_loop(
        "a reachable branch return preserves its feedback dependency",
        r#"
        module Top (
            select: input  logic,
            o     : output logic,
        ) {
            function choose (
                select  : input logic,
                feedback: input logic,
            ) -> logic {
                if select {
                    return feedback;
                } else {
                    return 0;
                }
            }
            var feedback: logic;
            assign o = choose(select, feedback);
            assign feedback = o;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_function_formal_high_bit_ignores_short_unsigned_actual() {
    assert_comb_loop(
        "a function formal high bit does not read an unsigned short actual",
        r#"
        module Top (o: output logic) {
            var value: logic<2>;
            function high (i: input logic<4>) -> logic { return i[3]; }
            assign o = high(value);
            assign value[0] = o;
            assign value[1] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_function_if_expression_arms_are_mutually_exclusive() {
    // Each function invocation selects one feed-forward equation. Flattening
    // both result alternatives into one source set invents the reverse edge.
    assert_comb_loop(
        "a function summary must not combine mutually exclusive expression arms",
        r#"
        module Identity (
            i: input  logic<2>,
            o: output logic<2>,
        ) {
            assign o = i;
        }
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            function choose (
                state: input logic<2>,
                sel  : input logic,
            ) -> logic<2> {
                return if sel ? {state[0], 1'b0} : {1'b0, state[1]};
            }
            var state: logic<2>;
            inst passthrough: Identity (
                i: choose(state, sel),
                o: state,
            );
            assign o = |state;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_function_onehot_runtime_input_detects_feedback() {
    // `$onehot(value)` is synthesized from the runtime value. Therefore the
    // two assignments form value -> o -> value feedback through the function.
    assert_comb_loop(
        "a runtime system function in a user function carries its input dependency",
        r#"
        module Top (
            o: output logic,
        ) {
            function exactly_one (
                value: input logic<2>,
            ) -> logic {
                return $onehot(value);
            }
            var value: logic<2>;
            assign o = exactly_one(value);
            assign value[0] = o;
            assign value[1] = 1'b0;
        }
        "#,
        true,
    );
}

#[test]
fn repeated_calls_reuse_the_function_write_footprint() {
    const WIDTH: usize = 128;
    let writes = (0..WIDTH)
        .map(|bit| format!("state[{bit}] = 0;"))
        .collect::<Vec<_>>()
        .join("\n");
    let calls = (0..WIDTH)
        .map(|_| "clear();")
        .collect::<Vec<_>>()
        .join("\n");
    let code = format!(
        r#"
        module Top (o: output logic) {{
            var state: logic<{WIDTH}>;
            function clear () {{
                {writes}
            }}
            always_comb {{
                {calls}
            }}
            assign o = |state;
        }}
        "#
    );

    crate::comb_loop_detect::reset_function_evaluation_count();
    let errors = analyze(&code);
    assert!(
        errors.is_empty(),
        "repeated constant writes are acyclic: {errors:#?}"
    );
    let visits = crate::comb_loop_detect::write_footprint_statement_visits();
    assert!(
        visits <= WIDTH * 4 + 8,
        "a shared function body must be walked once, not once per call site: {visits}"
    );
}

#[test]
fn recursive_function_summary_contexts_clone_only_referenced_metadata() {
    const COUNT: usize = 16;
    let padding = (0..COUNT)
        .map(|index| format!("var padding_{index}: logic;"))
        .collect::<Vec<_>>()
        .join("\n");
    let padding_assignments = (0..COUNT)
        .map(|index| format!("padding_{index} = seed;"))
        .collect::<Vec<_>>()
        .join("\n");
    let padding_uses = (0..COUNT)
        .map(|index| format!("padding_{index}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let mut functions = String::new();
    for index in (0..COUNT).rev() {
        let value = if index + 1 == COUNT {
            "value".to_owned()
        } else {
            format!("function_{}(value)", index + 1)
        };
        functions.push_str(&format!(
            "function function_{index} (value: input logic) -> logic {{ return {value}; }}\n"
        ));
    }
    let code = format!(
        r#"
        module Top (seed: input logic, o: output logic) {{
            {padding}
            {functions}
            always_comb {{
                {padding_assignments}
                o = function_0(seed) | {padding_uses};
            }}
        }}
        "#
    );

    crate::comb_loop_detect::reset_module_context_entries();
    let errors = analyze(&code);
    assert!(
        errors.is_empty(),
        "the function chain is acyclic: {errors:#?}"
    );
    assert!(
        crate::comb_loop_detect::module_context_entries() <= COUNT * 12,
        "each summary depth must clone only its local metadata: {}",
        crate::comb_loop_detect::module_context_entries(),
    );
}

#[test]
fn comb_loop_function_copyout_is_delayed_until_return() {
    assert_comb_loop(
        "function copy-out preserves call boundaries",
        r#"
    module Top (o: output logic) {
        function update (dst: output logic) -> logic {
            dst = 0;
            return o;
        }
        always_comb { o = update(o); }
    }
    "#,
        true,
    );
}

#[test]
fn comb_loop_function_copyout_overwrites_captured_write() {
    assert_comb_loop(
        "function copy-out preserves call boundaries",
        r#"
    module Top (o: output logic) {
        function update (dst: output logic) {
            dst = o;
            o = 0;
        }
        always_comb { update(o); }
    }
    "#,
        true,
    );
}

#[test]
fn comb_loop_function_copyout_actual_aliases_input() {
    assert_comb_loop(
        "function copy-out preserves call boundaries",
        r#"
    module Top (o: output logic) {
        function update (src: input logic, dst: output logic) {
            dst = 0;
            dst = src;
        }
        always_comb { update(o, o); }
    }
    "#,
        true,
    );
}

#[test]
fn comb_loop_function_copyout_dynamic_selectors() {
    for (ty, initial) in [("logic[2]", "'{default: 0}"), ("logic<2>", "0")] {
        for (index, expected) in [("feedback", true), ("external", false)] {
            let code = format!(
                r#"
                module Top (external: input logic, o: output logic) {{
                    var values: {ty};
                    var feedback: logic;
                    function set (value: output logic) {{ value = 1; }}
                    always_comb {{
                        values = {initial};
                        set(values[{index}]);
                        feedback = values[0];
                        o = feedback;
                    }}
                }}
            "#
            );
            assert_comb_loop(
                "copy-out reads both array and packed selectors",
                &code,
                expected,
            );
        }
    }
}

#[test]
fn comb_loop_function_copyout_converts_formal_type() {
    for (formal, actual, value, bit, expected) in [
        ("i8", "i16", "{feedback, 7'b0} as i8", 15, true),
        ("u8", "u16", "{feedback, 7'b0} as u8", 15, false),
        ("i8", "i16", "{feedback, 7'b0} as i8", 0, false),
        ("i16", "i8", "{feedback, 15'b0} as i16", 7, false),
        ("i16", "i8", "{8'b0, feedback, 7'b0} as i16", 7, true),
    ] {
        let code = format!(
            r#"
            module Top (o: output logic) {{
                var value: {actual};
                var feedback: logic;
                function set (src: input {formal}, dst: output {formal}) {{ dst = src; }}
                always_comb {{ set({value}, value); }}
                assign feedback = value[{bit}];
                assign o = feedback;
            }}
        "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
            "{formal} -> {actual}: {errors:#?}"
        );
        assert_eq!(
            !errors.is_empty(),
            expected,
            "{formal} -> {actual}, bit {bit}: {errors:#?}"
        );
    }
}

#[test]
fn comb_loop_function_copyout_sign_extension_into_concatenation() {
    for (read, expected) in [("high[7]", true), ("low[0]", false)] {
        assert_comb_loop(
            "sign extension is applied before splitting output actuals",
            &format!(
                r#"
            module Top (o: output logic) {{
                var high: logic<8>;
                var low: logic<8>;
                var feedback: logic;
                function set (src: input logic, dst: output i8) {{
                    dst = {{src, 7'b0}} as i8;
                }}
                always_comb {{ set(feedback, {{high, low}}); }}
                assign feedback = {read};
                assign o = feedback;
            }}
        "#
            ),
            expected,
        );
    }
}

#[test]
fn comb_loop_function_copyout_evaluates_selector_once_before_region_writes() {
    assert_comb_loop(
        "a side-effecting selector is sampled before every copied region",
        r#"
        module Top (o: output logic) {
            var values: logic[2];
            var feedback: logic;
            function select () -> logic {
                let previous: logic = feedback;
                feedback = 0;
                return previous;
            }
            function set (value: output logic) { value = 1; }
            always_comb {
                values = '{default: 0};
                set(values[select()]);
                feedback = values[1];
                o = feedback | values[0];
            }
        }
    "#,
        true,
    );
}

#[test]
fn comb_loop_function_copyout_dynamic_array_copies_value_to_each_candidate() {
    assert_comb_loop(
        "dynamic output actuals may receive the formal at a nonzero index",
        r#"
        module Top (external: input logic, o: output logic) {
            var values: logic[2];
            var feedback: logic;
            function set (src: input logic, dst: output logic) { dst = src; }
            always_comb {
                values = '{default: 0};
                set(feedback, values[external]);
                feedback = values[1];
                o = feedback;
            }
        }
    "#,
        true,
    );
}

#[test]
fn comb_loop_function_copyout_dynamic_array_preserves_packed_position() {
    for index in ["external", "0"] {
        for element in [0, 1] {
            for bit in [0, 1] {
                let code = format!(
                    r#"
                    module Top (external: input logic, o: output logic) {{
                        var values: logic<2>[2];
                        var feedback: logic;
                        function set (src: input logic, dst: output logic<2>) {{
                            dst = {{src, 1'b0}};
                        }}
                        always_comb {{
                            values = '{{default: 0}};
                            set(feedback, values[{index}]);
                        }}
                        assign feedback = values[{element}][{bit}];
                        assign o = feedback;
                    }}
                    "#
                );
                assert_comb_loop(
                    &format!("copy-out to [{index}], reading [{element}][{bit}]"),
                    &code,
                    bit == 1 && (index == "external" || element == 0),
                );
                assert!(comb_loop_analysis_is_complete(&code));
            }
        }
    }
}

#[test]
fn comb_loop_function_copyin_converts_actual_type() {
    for (actual, formal, source_bit, result_bit, expected) in [
        ("i8", "i16", 7, 15, true),
        ("i8", "u16", 7, 15, true),
        ("u8", "i16", 7, 15, false),
        ("u8", "u16", 7, 15, false),
        ("i8", "i16", 0, 15, false),
        ("i8", "i16", 7, 0, false),
        ("i16", "i8", 15, 7, false),
        ("i16", "i8", 7, 7, true),
    ] {
        let code = format!(
            r#"
            module Top (o: output logic) {{
                var feedback: logic;
                var value: {actual};
                function read_bit (x: input {formal}) -> logic {{ return x[{result_bit}]; }}
                assign value = (feedback as {actual}) << {source_bit};
                assign feedback = read_bit(value);
                assign o = feedback;
            }}
            "#
        );
        assert_comb_loop(
            &format!("copy-in {actual}[{source_bit}] -> {formal}[{result_bit}]"),
            &code,
            expected,
        );
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn comb_loop_function_copyin_context_determined_expressions() {
    for (actual, expected) in [
        ("value << 1", true),
        ("{value << 1}", false),
        ("value as 16 << 1", true),
        ("(value << 1) as 8", false),
        ("(value as 8) << 1", true),
        ("$signed(value << 1)", false),
        ("$signed(value) << 1", true),
        ("$unsigned(value << 1)", false),
        ("$unsigned(value) << 1", true),
        ("+(value << 1)", true),
        ("~(value << 1)", true),
        ("(value << 1) | 8'b0", true),
        ("if external ? value << 1 : 8'b0", true),
        ("(value << 1) == 8'b0", false),
        ("value + value", true),
    ] {
        let code = format!(
            r#"
            module Top(external: input logic, o: output logic) {{
                var feedback: logic;
                var value: logic<8>;
                function high(x: input logic<16>) -> logic {{ return x[8]; }}
                assign value = {{feedback, 7'b0}};
                assign feedback = high({actual});
                assign o = feedback;
            }}
            "#
        );
        assert_comb_loop(&format!("copy-in context for {actual}"), &code, expected);
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn comb_loop_function_copyin_extends_before_shifting() {
    for (r#type, actual, source_bit, result_bit, expected) in [
        ("i8", "value << 1", 7, 15, true),
        ("i8", "value << 1", 0, 15, false),
        ("u8", "value << 1", 7, 15, false),
        ("i8", "value >> 1", 7, 14, true),
        ("i8", "value >> 1", 7, 15, false),
        ("i8", "value >>> 1", 7, 15, true),
        ("i8", "value >>> 1", 0, 15, false),
        ("u8", "value >> 1", 7, 14, false),
        ("i8", "(value >> 1) | 8'b0", 7, 14, false),
        ("i8", "(value >>> 1) | 8'b0", 7, 15, false),
    ] {
        let code = format!(
            r#"
            module Top(o: output logic) {{
                var feedback: logic;
                var value: {type};
                function high(x: input logic<16>) -> logic {{ return x[{result_bit}]; }}
                assign value = (feedback as {type}) << {source_bit};
                assign feedback = high({actual});
                assign o = feedback;
            }}
            "#
        );
        assert_comb_loop(
            &format!("copy-in {type}[{source_bit}], {actual}, reading bit {result_bit}"),
            &code,
            expected,
        );
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn comb_loop_function_copyin_dynamic_array_preserves_packed_position() {
    for index in ["external", "0", "1"] {
        for bit in [0, 7] {
            let code = format!(
                r#"
                module Top(external: input logic, o: output logic) {{
                    var feedback: logic;
                    var values: i8[2];
                    function high(x: input i16) -> logic {{ return x[15]; }}
                    assign values[0] = (feedback as i8) << {bit};
                    assign values[1] = 0;
                    assign feedback = high(values[{index}]);
                    assign o = feedback;
                }}
                "#
            );
            assert_comb_loop(
                &format!("copy-in values[{index}], feedback in bit {bit}"),
                &code,
                bit == 7 && index != "1",
            );
            assert!(comb_loop_analysis_is_complete(&code));
        }
    }
}

#[test]
fn comb_loop_function_copyin_dynamic_array_preserves_packed_slice() {
    for actual in [
        "values[external][9:2] as i8",
        "$signed(values[external][9:2])",
    ] {
        for bit in [2, 9] {
            let code = format!(
                r#"
                module Top(external: input logic, o: output logic) {{
                    var feedback: logic;
                    var values: logic<10>[2];
                    function high(x: input i16) -> logic {{ return x[15]; }}
                    assign values[0] = (feedback as 10) << {bit};
                    assign values[1] = 0;
                    assign feedback = high({actual});
                    assign o = feedback;
                }}
                "#
            );
            assert_comb_loop(
                &format!("copy-in {actual}, feedback in bit {bit}"),
                &code,
                bit == 9,
            );
            assert!(comb_loop_analysis_is_complete(&code));
        }
    }
}

#[test]
fn comb_loop_function_copyin_sign_extension_preserves_array_elements() {
    for element in [0, 1] {
        let code = format!(
            r#"
            module Top (o: output logic) {{
                var feedback: logic;
                var values: i8[2];
                function high (x: input i16[2]) -> logic {{ return x[{element}][15]; }}
                assign values[0] = {{feedback, 7'b0}} as i8;
                assign values[1] = 0;
                assign feedback = high(values);
                assign o = feedback;
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
            "{errors:#?}"
        );
        assert_eq!(!errors.is_empty(), element == 0);
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn comb_loop_function_copyout_dynamic_array_converts_before_slicing() {
    for (actual, read_bit, expected) in [
        ("values[external][17:2]", 0, false),
        ("values[external][17:2]", 2, false),
        ("values[external][17:2]", 9, true),
        ("values[external][17:2]", 17, true),
        ("{values[external][15:8], values[external][7:0]}", 0, false),
        ("{values[external][15:8], values[external][7:0]}", 7, true),
        ("{values[external][15:8], values[external][7:0]}", 15, true),
    ] {
        let code = format!(
            r#"
            module Top (external: input logic, o: output logic) {{
                var feedback: logic;
                var values: logic<18>[2];
                function set (src: input logic, dst: output i8) {{
                    dst = {{src, 7'b0}} as i8;
                }}
                always_comb {{
                    values = '{{default: 0}};
                    set(feedback, {actual});
                }}
                assign feedback = values[1][{read_bit}];
                assign o = feedback;
            }}
            "#
        );
        assert_comb_loop(
            &format!("signed copy-out to {actual}, bit {read_bit}"),
            &code,
            expected,
        );
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn comb_loop_function_copyout_sign_extension_preserves_array_elements() {
    for element in [0, 1] {
        let code = format!(
            r#"
            module Top (o: output logic) {{
                var feedback: logic;
                var values: i16[2];
                function set (src: input logic, dst: output i8[2]) {{
                    dst[0] = {{src, 7'b0}} as i8;
                    dst[1] = 0;
                }}
                always_comb {{
                    values = '{{default: 0}};
                    set(feedback, values);
                }}
                assign feedback = values[{element}][15];
                assign o = feedback;
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
            "{errors:#?}"
        );
        assert_eq!(!errors.is_empty(), element == 0);
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn comb_loop_function_copyin_samples_side_effecting_actual_once() {
    for actual in ["sample()", "sample() << 1"] {
        assert_comb_loop(
            "copy-in widening must reuse the sampled value across formal regions",
            &format!(
                r#"
                module Top (o: output logic) {{
                    var feedback: logic;
                    function sample () -> i8 {{
                        let previous: logic = feedback;
                        feedback = 0;
                        return {{previous, 7'b0}} as i8;
                    }}
                    function high (x: input i16) -> logic {{ return x[15] | x[0]; }}
                    always_comb {{
                        feedback = high({actual});
                        o = feedback;
                    }}
                }}
                "#
            ),
            true,
        );
    }
}

#[test]
fn function_summary_instance_actual_preserves_shift_fanout_dag() {
    const STAGES: usize = 18;
    const WIDTH: usize = 1 << (STAGES + 1);
    let statements = (0..STAGES)
        .map(|i| format!("v = (v << {}) | v;", 1usize << i))
        .collect::<Vec<_>>()
        .join("\n");
    let code = format!(
        r#"
        module Sink(i: input logic<{WIDTH}>, o: output logic) {{ assign o = i[0]; }}
        module Top(i: input logic<{WIDTH}>, o: output logic) {{
            function spread(x: input logic<{WIDTH}>) -> logic<{WIDTH}> {{
                var v: logic<{WIDTH}>;
                v = x;
                {statements}
                return v;
            }}
            inst sink: Sink(i: spread(i), o);
        }}
    "#
    );
    crate::comb_loop_detect::reset_source_walk_visits();
    assert!(analyze(&code).is_empty());
    assert!(
        crate::comb_loop_detect::source_walk_visits() < STAGES * 10,
        "an instance actual must not enumerate subset sums of shifts: {}",
        crate::comb_loop_detect::source_walk_visits()
    );
}

#[test]
fn comb_loop_function_copyout_freezes_concatenated_actual_selectors() {
    assert_comb_loop(
        "all selectors precede the concatenation's first copied value",
        r#"
        module Top (o: output logic) {
            var values: logic[2];
            var index: logic;
            function set (dst: output logic<2>) { dst = 2'b11; }
            always_comb {
                values = '{default: 0};
                set({index, values[index]});
                index = values[0];
                o = index;
            }
        }
    "#,
        true,
    );
}

#[test]
fn function_summary_instance_actual_keeps_shifted_bit_dependencies() {
    for (bit, expected) in [(17, true), (40, false)] {
        let statements = (0..5)
            .map(|i| format!("v = (v << {}) | v;", 1 << i))
            .collect::<Vec<_>>()
            .join("\n");
        let code = format!(
            r#"
            module Sink(i: input logic<64>, o: output logic) {{ assign o = i[{bit}]; }}
            module Top(o: output logic) {{
                var input_value: logic<64>;
                var feedback: logic;
                function spread(x: input logic<64>) -> logic<64> {{
                    var v: logic<64>; v = x; {statements} return v;
                }}
                assign input_value = {{63'b0, feedback}};
                inst sink: Sink(i: spread(input_value), o: feedback);
                assign o = feedback;
            }}
        "#
        );
        assert_comb_loop(
            "only reachable shifted bits close the instance feedback",
            &code,
            expected,
        );
        assert!(comb_loop_analysis_is_complete(&code));
    }
}
