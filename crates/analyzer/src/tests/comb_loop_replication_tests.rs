use super::*;

#[test]
fn comb_loop_arithmetic_sign_fill_through_whole_copy() {
    let mut mismatches = Vec::new();
    for op in [">>", ">>>"] {
        for shift in [0, 1, 7, 8, 9] {
            for bit in [0, 6, 7] {
                let code = format!(
                    r#"
                    module Top(o: output i8) {{
                        var middle: i8;
                        var feedback: logic;
                        assign middle = ({{feedback, 7'b0}} as i8) {op} {shift};
                        assign o = middle;
                        assign feedback = o[{bit}];
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
                let actual = !errors.is_empty();
                let expected = if op == ">>>" {
                    bit >= 7usize.saturating_sub(shift)
                } else {
                    shift < 8 && bit == 7 - shift
                };
                eprintln!(
                    "SIGNFILL op={op} shift={shift} bit={bit} loop={actual} expected={expected}"
                );
                if actual != expected {
                    mismatches.push((op, shift, bit, actual, expected));
                }
                assert!(comb_loop_analysis_is_complete(&code));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

#[test]
fn comb_loop_repeated_concat_through_whole_copy() {
    let mut mismatches = Vec::new();
    for repeated in [false, true] {
        let expression = if repeated {
            "{{feedback, 7'b0} repeat 2}"
        } else {
            "{feedback, 7'b0, feedback, 7'b0}"
        };
        for bit in [0, 7, 8, 15] {
            let code = format!(
                "module Top(o: output logic<16>) {{ var middle: logic<16>; var feedback: logic; assign middle = {expression}; assign o = middle; assign feedback = o[{bit}]; }}"
            );
            let errors = analyze(&code);
            assert!(
                errors
                    .iter()
                    .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                "{errors:#?}"
            );
            let actual = !errors.is_empty();
            let expected = bit % 8 == 7;
            eprintln!(
                "REPEAT packed repeated={repeated} bit={bit} loop={actual} expected={expected}"
            );
            if actual != expected {
                mismatches.push((repeated, bit, actual, expected));
            }
            assert!(comb_loop_analysis_is_complete(&code));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

#[test]
fn comb_loop_repeated_array_through_whole_copy() {
    let mut mismatches = Vec::new();
    for repeated in [false, true] {
        let expression = if repeated {
            "'{'{feedback, 1'b0} repeat 2}"
        } else {
            "'{'{feedback, 1'b0}, '{feedback, 1'b0}}"
        };
        for row in [0, 1] {
            for column in [0, 1] {
                let code = format!(
                    "module Top(o: output logic[2, 2]) {{ var middle: logic[2, 2]; var feedback: logic; assign middle = {expression}; assign o = middle; assign feedback = o[{row}][{column}]; }}"
                );
                let errors = analyze(&code);
                assert!(
                    errors
                        .iter()
                        .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                    "{errors:#?}"
                );
                let actual = !errors.is_empty();
                let expected = column == 0;
                eprintln!(
                    "REPEAT array repeated={repeated} row={row} column={column} loop={actual} expected={expected}"
                );
                if actual != expected {
                    mismatches.push((repeated, row, column, actual, expected));
                }
                assert!(comb_loop_analysis_is_complete(&code));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

fn assert_replication_feedback(code: &str, expected: bool) {
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{code}\n{errors:#?}"
    );
    assert_eq!(!errors.is_empty(), expected, "{code}\n{errors:#?}");
    assert!(comb_loop_analysis_is_complete(code), "{code}");
}

#[test]
fn comb_loop_replication_survives_calls_modules_and_runtime_transfers() {
    for (width, expression, bits) in [
        (8, "x >>> 1", [(0, false), (5, false), (6, true), (7, true)]),
        (
            24,
            "{x repeat 3}",
            [(0, false), (7, true), (8, false), (23, true)],
        ),
    ] {
        for transfer in [
            "direct",
            "return",
            "output",
            "module",
            "runtime_return",
            "runtime_output",
        ] {
            let assignment = match transfer {
                "direct" => format!("assign middle = {};", expression.replace('x', "source")),
                "return" => "assign middle = transform(source);".into(),
                "output" => "always_comb { copy(source, middle); }".into(),
                "module" => "inst u: Transform(x: source, y: middle);".into(),
                "runtime_return" => {
                    "always_comb { middle = 0; for _i in 0..n { middle = transform(source); } }"
                        .into()
                }
                "runtime_output" => {
                    "always_comb { middle = 0; for _i in 0..n { copy(source, middle); } }".into()
                }
                _ => unreachable!(),
            };
            let input = if transfer.starts_with("runtime") {
                "n: input u32,"
            } else {
                ""
            };
            for (bit, expected) in bits {
                let code = format!(
                    r#"
                    module Transform(x: input i8, y: output signed logic<{width}>) {{ assign y = {expression}; }}
                    module Top({input} o: output signed logic<{width}>) {{
                        function transform(x: input i8) -> signed logic<{width}> {{ return {expression}; }}
                        function copy(x: input i8, y: output signed logic<{width}>) {{ y = {expression}; }}
                        var source: i8;
                        var middle: signed logic<{width}>;
                        var feedback: logic;
                        assign source = {{feedback, 7'b0}} as i8;
                        {assignment}
                        assign o = middle;
                        assign feedback = o[{bit}];
                    }}
                "#
                );
                assert_replication_feedback(&code, expected);
            }
        }
    }
}

#[test]
fn comb_loop_repeat_clips_padding_and_destination_slices() {
    for assignment in [
        "wide = {2'b0, {{feedback, 7'b0} repeat 3}, 2'b0};",
        "wide[25:2] = {{feedback, 7'b0} repeat 3};",
        "{wide[25:14], wide[13:2]} = {{feedback, 7'b0} repeat 3};",
    ] {
        for bit in [0, 2, 9, 10, 17, 25, 26, 27] {
            let code = format!(
                r#"
                module Top(o: output logic<28>) {{
                    var feedback: logic;
                    var wide: logic<28>;
                    always_comb {{ wide = 0; {assignment} }}
                    assign o = wide;
                    assign feedback = o[{bit}];
                }}
            "#
            );
            assert_replication_feedback(&code, [9, 17, 25].contains(&bit));
        }
    }
}

#[test]
fn comb_loop_large_repeat_has_bounded_graph_and_search_work() {
    // Keep the HDL widths below the default constant-evaluation limit.
    for count in [3, 1024, 100_003] {
        let width = count * 8;
        for bit in [0, 7, width - 8, width - 1] {
            let code = format!(
                r#"
                module Top(o: output logic<{width}>) {{
                    function spread(x: input logic<8>) -> logic<{width}> {{ return {{x repeat {count}}}; }}
                    var feedback: logic;
                    var middle: logic<{width}>;
                    assign middle = spread({{feedback, 7'b0}});
                    assign o = middle;
                    assign feedback = o[{bit}];
                }}
            "#
            );
            crate::comb_loop_detect::reset_function_evaluation_count();
            crate::comb_loop_detect::reset_cycle_search_work();
            // A small budget must suffice for the decision even when a
            // concrete diagnostic path would contain millions of self steps.
            let errors = crate::comb_loop_detect::with_cycle_search_limit(5_000, || analyze(&code));
            assert!(
                errors
                    .iter()
                    .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                "{errors:#?}"
            );
            assert_eq!(
                !errors.is_empty(),
                bit % 8 == 7,
                "count={count}, bit={bit}: {errors:#?}"
            );
            let nodes = crate::comb_loop_detect::function_summary_graph_node_count();
            let work = crate::comb_loop_detect::cycle_search_work();
            eprintln!("replication count={count} bit={bit} nodes={nodes} search_work={work}");
            assert!(nodes < 20, "count={count}: {nodes} nodes");
            assert!(work < 10_000, "count={count}: {work} search work");
            assert!(crate::comb_loop_detect::with_cycle_search_limit(
                5_000,
                || comb_loop_analysis_is_complete(&code)
            ));
        }
    }
}

#[test]
fn comb_loop_repeated_function_imports_stay_shared() {
    const COUNT: usize = 32;
    const WIDTH: usize = COUNT * 8;
    for depth in [1, 4, 8] {
        let mut functions = format!(
            "function f0(x: input logic<8>) -> logic<{WIDTH}> {{ return {{x repeat {COUNT}}}; }}\n"
        );
        for level in 1..=depth {
            let previous = level - 1;
            functions.push_str(&format!("function f{level}(x: input logic<8>) -> logic<{WIDTH}> {{ return f{previous}(x) | f{previous}(x); }}\n"));
        }
        let code = format!(
            "module Top(x: input logic<8>, o: output logic<{WIDTH}>) {{ {functions} assign o = f{depth}(x); }}"
        );
        crate::comb_loop_detect::reset_function_evaluation_count();
        let errors = analyze(&code);
        assert!(errors.is_empty(), "{errors:#?}");
        let nodes = crate::comb_loop_detect::function_summary_graph_node_count();
        eprintln!("replication imports depth={depth} nodes={nodes}");
        assert!(nodes < 12 * depth + 4, "depth={depth}: {nodes} nodes");
    }
}

#[test]
fn comb_loop_replication_keeps_dynamic_array_destination_coordinates() {
    for (width, expression, bits) in [
        (8, "narrow[index] >>> 1", [(0, false), (6, true), (7, true)]),
        (
            24,
            "{narrow[index] repeat 3}",
            [(0, false), (7, true), (23, true)],
        ),
    ] {
        for (bit, expected) in bits {
            let code = format!(
                r#"
                module Top(index: input logic, o: output logic) {{
                    var feedback: logic;
                    var narrow: i8[2];
                    var wide: signed logic<{width}>[2];
                    var copied: signed logic<{width}>[2];
                    assign narrow[0] = 0;
                    assign narrow[1] = {{feedback, 7'b0}} as i8;
                    always_comb {{ wide = '{{default: 0}}; wide[index] = {expression}; }}
                    assign copied = wide;
                    assign feedback = copied[1][{bit}];
                    assign o = feedback;
                }}
            "#
            );
            assert_replication_feedback(&code, expected);
        }
    }
}

#[test]
fn comb_loop_nested_replication_is_sparse() {
    for (inner, outer) in [(3, 3), (1024, 101)] {
        let width = 8 * inner * outer;
        for bit in [0, 7, width - 8, width - 1] {
            let code = format!(
                r#"
                module Top(o: output logic<{width}>) {{
                    function spread(x: input logic<8>) -> logic<{width}> {{
                        return {{{{x repeat {inner}}} repeat {outer}}};
                    }}
                    var feedback: logic;
                    var middle: logic<{width}>;
                    assign middle = spread({{feedback, 7'b0}});
                    assign o = middle;
                    assign feedback = o[{bit}];
                }}
            "#
            );
            crate::comb_loop_detect::with_cycle_search_limit(5_000, || {
                assert_replication_feedback(&code, bit % 8 == 7);
            });
        }
    }
}

#[test]
fn comb_loop_replication_samples_function_results_before_copy_out() {
    for (width, expression, bits) in [
        (
            8,
            "take(value, value) >>> 1",
            [(0, false), (6, true), (7, true)],
        ),
        (
            24,
            "{take(value, value) repeat 3}",
            [(0, false), (7, true), (23, true)],
        ),
    ] {
        for (bit, expected) in bits {
            let code = format!(
                r#"
                module Top(o: output signed logic<{width}>) {{
                    function take(x: input i8, y: output i8) -> i8 {{ y = 0; return x; }}
                    var feedback: logic;
                    var middle: signed logic<{width}>;
                    always_comb {{
                        var value: i8;
                        value = {{feedback, 7'b0}} as i8;
                        middle = {expression};
                    }}
                    assign o = middle;
                    assign feedback = o[{bit}];
                }}
            "#
            );
            assert_replication_feedback(&code, expected);
        }
    }
}

#[test]
fn comb_loop_multiple_repeat_terms_preserve_their_own_offsets() {
    for bit in 0..14 {
        let code = format!(
            r#"
            module Top(o: output logic<14>) {{
                var feedback: logic;
                var middle: logic<14>;
                assign middle = {{{{feedback, 3'b0}} repeat 2, {{feedback, 1'b0}} repeat 3}};
                assign o = middle;
                assign feedback = o[{bit}];
            }}
        "#
        );
        assert_replication_feedback(&code, [1, 3, 5, 9, 13].contains(&bit));
    }
}
