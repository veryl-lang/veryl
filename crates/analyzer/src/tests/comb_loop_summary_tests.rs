use super::*;
use crate::comb_loop_detect::{module_summary_work, reset_module_summary_work};

fn check(code: &str, expected_loop: bool) {
    reset_module_summary_work();
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{code}\n{errors:?}"
    );
    assert_eq!(!errors.is_empty(), expected_loop, "{code}\n{errors:?}");
    let (input_edges, walked_edges) = module_summary_work();
    assert!(
        walked_edges <= input_edges,
        "{walked_edges} > {input_edges}\n{code}"
    );
    assert!(comb_loop_analysis_is_complete(code), "{code}");
}

const SHIFT_DANGLE: &str = r#"
module ShiftDangle (i: input logic<16>, o: output logic<16>) {
    var s: logic<16>;
    var t: logic<16>;
    assign t[12:0]  = s[15:3];
    assign t[15:13] = i[2:0];
    assign s        = t;
    assign o        = i;
}
"#;

#[test]
fn shifted_dangling_recurrence_terminates_and_is_complete() {
    check(SHIFT_DANGLE, false);
}

#[test]
fn unpacked_shift_recurrences_preserve_live_and_dangling_feedback() {
    for width in [4, 16, 64] {
        for wrap in [false, true] {
            for connected in [false, true] {
                let fill = if wrap { "s" } else { "i" };
                let output = if connected { "s" } else { "i" };
                let mut code = format!(
                    r#"
                    module Top (i: input logic [{width}], o: output logic [{width}]) {{
                        var s: logic [{width}];
                        var t: logic [{width}];
                        assign s = t;
                        assign o = {output};
                "#,
                );
                for index in 0..width {
                    let (source, source_index) = if index < width - 3 {
                        ("s", index + 3)
                    } else {
                        (fill, index + 3 - width)
                    };
                    code.push_str(&format!("assign t[{index}] = {source}[{source_index}];\n"));
                }
                code.push_str("}\n");
                check(&code, wrap);
            }
        }
    }
}

fn shift_code(width: usize, shift: usize, left: bool, connected: bool, wrap: bool) -> String {
    let (target, source, fill, input) = if left {
        (
            format!("{}:{shift}", width - 1),
            format!("{}:0", width - shift - 1),
            format!("{}:0", shift - 1),
            format!("{}:{}", width - 1, width - shift),
        )
    } else {
        (
            format!("{}:0", width - shift - 1),
            format!("{}:{shift}", width - 1),
            format!("{}:{}", width - 1, width - shift),
            format!("{}:0", shift - 1),
        )
    };
    let fill_value = if wrap {
        format!("s[{input}]")
    } else {
        format!("i[{}:0]", shift - 1)
    };
    let output = if connected { "s" } else { "i" };
    format!(
        r#"
        module ShiftDangle (i: input logic<{width}>, o: output logic<{width}>) {{
            var s: logic<{width}>;
            var t: logic<{width}>;
            assign t[{target}] = s[{source}];
            assign t[{fill}] = {fill_value};
            assign s = t;
            assign o = {output};
        }}
    "#
    )
}

#[test]
fn generated_left_and_right_recurrences_keep_live_and_dangling_cycles_distinct() {
    for width in [4, 16, 32] {
        for shift in [1, 3, width - 1] {
            for left in [false, true] {
                for connected in [false, true] {
                    for wrap in [false, true] {
                        check(&shift_code(width, shift, left, connected, wrap), wrap);
                    }
                }
            }
        }
    }
}

#[test]
fn million_bit_dangling_recurrences_do_not_enumerate_declared_bits() {
    for left in [false, true] {
        check(&shift_code(1_000_000, 3, left, false, false), false);
        let (_, wide_work) = module_summary_work();
        check(&shift_code(100, 3, left, false, false), false);
        let (_, narrow_work) = module_summary_work();
        assert_eq!(wide_work, narrow_work);
    }
}

#[test]
fn wide_dangling_rotate_preserves_incomplete_cycle_search_status() {
    // This rotate exceeds the separate compatible-cycle search budget. A
    // completed feedthrough summary must not turn that into a proof of safety.
    let code = shift_code(257, 3, false, false, true);
    reset_module_summary_work();
    assert!(!comb_loop_analysis_is_complete(&code));
    let (input_edges, walked_edges) = module_summary_work();
    assert!(walked_edges <= input_edges);
}

#[test]
fn discarded_branch_dags_do_not_enumerate_positional_path_combinations() {
    const DEPTH: usize = 18;
    let width = 1 << DEPTH;
    let mut code = format!("module Top (i: input logic<{width}>, o: output logic<{width}>) {{\n");
    let mut previous = "i".to_string();
    for index in 0..DEPTH {
        code.push_str(&format!("var value{index}: logic<{width}>;\nassign value{index} = {previous} | ({previous} << {});\n", 1 << index));
        previous = format!("value{index}");
    }
    code.push_str("assign o = i;\n}");
    check(&code, false);
}

#[test]
fn dangling_recurrences_remain_bounded_through_module_hierarchy() {
    let mut code = SHIFT_DANGLE.to_string();
    let mut previous = "ShiftDangle".to_string();
    for depth in 0..32 {
        code.push_str(&format!(
            r#"
            module Wrapper{depth} (i: input logic<16>, o: output logic<16>) {{
                inst child: {previous} (i: i, o: o);
            }}
        "#
        ));
        previous = format!("Wrapper{depth}");
    }
    check(&code, false);
    code.push_str(&format!(
        r#"
        module Top (o: output logic<16>) {{ inst child: {previous} (i: o, o: o); }}
    "#
    ));
    check(&code, true);
}

#[test]
fn wire_hierarchy_summaries_grow_linearly() {
    for ty in ["logic<16>", "logic [16]"] {
        for depth in [4, 8, 24, 32] {
            let mut code =
                format!("module Leaf (i: input {ty}, o: output {ty}) {{ assign o = i; }}\n");
            let mut previous = "Leaf".to_string();
            for level in 0..depth {
                code.push_str(&format!(
                    r#"
                    module Wrapper{level} (i: input {ty}, o: output {ty}) {{
                        var middle: {ty};
                        inst left: {previous} (i: i, o: middle);
                        inst right: {previous} (i: middle, o: o);
                    }}
                    "#
                ));
                previous = format!("Wrapper{level}");
            }
            check(&code, false);
            let (input_edges, _) = module_summary_work();
            assert!(
                input_edges <= 16 * (depth + 1),
                "wire summaries must not expand the instance tree: depth={depth}, edges={input_edges}"
            );

            code.push_str(&format!(
                "module Top (o: output {ty}) {{ inst child: {previous} (i: o, o: o); }}\n"
            ));
            check(&code, true);
            let (input_edges, _) = module_summary_work();
            assert!(input_edges <= 16 * (depth + 2));
        }
    }
}

#[test]
fn wire_summary_contraction_preserves_narrowing() {
    for feedback in ["{z, 15'b0}", "{15'b0, z}"] {
        check(
            &format!(
                r#"
                module Child (i: input logic<16>, o: output logic<16>) {{
                    var narrowed: logic<8>;
                    var copied: logic<8>;
                    assign narrowed = i;
                    assign copied = narrowed;
                    assign o = copied;
                }}
                module Top (z: output logic) {{
                    var value: logic<16>;
                    inst child: Child (i: {feedback}, o: value);
                    assign z = |value;
                }}
                "#
            ),
            feedback == "{15'b0, z}",
        );
    }
}

#[test]
fn unused_local_feedback_is_diagnosed_without_an_input_output_path() {
    check(
        r#"
        module Top (i: input logic<16>) {
            var s: logic<16>;
            var t: logic<16>;
            assign t = {s[14:0], s[15]};
            assign s = t;
        }
    "#,
        true,
    );
}

#[test]
fn procedural_dangling_recurrences_keep_runtime_branch_guards() {
    for loop_arm in ["0", "s[2:0]"] {
        check(
            &format!(
                r#"
            module Top (i: input logic<16>, enable: input logic, o: output logic<16>) {{
                var s: logic<16>;
                var t: logic<16>;
                always_comb {{
                    if enable {{
                        t[12:0] = s[15:3];
                        t[15:13] = {loop_arm};
                    }} else {{
                        t = i;
                    }}
                }}
                assign s = t;
                assign o = i;
            }}
        "#
            ),
            loop_arm != "0",
        );
    }
}

#[test]
fn guarded_function_chains_preserve_feedthrough_across_module_boundaries() {
    for length in [8, 128] {
        let mut code = format!(
            r#"
            module Child (i: input logic, enables: input logic<{length}>, o: output logic) {{
                function gate(x: input logic, enables: input logic<{length}>) -> logic {{
                    var value: logic;
                    value = x;
        "#
        );
        for index in 0..length {
            code.push_str(&format!("if !enables[{index}] {{ value = 0; }}\n"));
        }
        code.push_str("return value;\n}\nassign o = gate(i, enables);\n}\n");
        check(&code, false);
        code.push_str(&format!(
            r#"
            module Top (enables: input logic<{length}>, o: output logic) {{
                inst child: Child (i: o, enables: enables, o: o);
            }}
        "#
        ));
        check(&code, true);
    }
}

// Each vector chooses one shifted source in each branch. Exhaust the four
// independent branch inputs and use an ordinary bit-level topological sort
// as an oracle, independently of the sparse analyzer and module summaries.
fn expanded_network_has_cycle(width: usize, choices: &[[(usize, isize); 2]; 4]) -> bool {
    for valuation in 0..16 {
        let mut outgoing = vec![Vec::new(); width * choices.len()];
        let mut incoming = vec![0; outgoing.len()];
        for (target, arms) in choices.iter().enumerate() {
            let (source, shift) = arms[valuation >> target & 1];
            if source == choices.len() {
                continue;
            }
            for target_bit in 0..width {
                let source_bit = target_bit as isize - shift;
                if (0..width as isize).contains(&source_bit) {
                    let source = source * width + source_bit as usize;
                    let target = target * width + target_bit;
                    outgoing[source].push(target);
                    incoming[target] += 1;
                }
            }
        }
        let mut queue = incoming
            .iter()
            .enumerate()
            .filter_map(|(node, &degree)| (degree == 0).then_some(node))
            .collect::<std::collections::VecDeque<_>>();
        let mut visited = 0;
        while let Some(node) = queue.pop_front() {
            visited += 1;
            for &next in &outgoing[node] {
                incoming[next] -= 1;
                if incoming[next] == 0 {
                    queue.push_back(next);
                }
            }
        }
        if visited < outgoing.len() {
            return true;
        }
    }
    false
}

#[test]
fn generated_guarded_shift_networks_match_expanded_bit_graphs() {
    let mut random_state = 0x3a29_f125u32;
    let mut random = || {
        random_state = random_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (random_state >> 16) as usize
    };
    let mut cyclic = 0;
    for case in 0..128 {
        let width = [4, 8, 16][case % 3];
        let choices = std::array::from_fn(|_| {
            std::array::from_fn(|_| (random() % 5, (random() % 7) as isize - 3))
        });
        let expected_loop = expanded_network_has_cycle(width, &choices);
        cyclic += usize::from(expected_loop);
        let expression = |(source, shift): (usize, isize)| {
            let source = if source == 4 {
                "i".to_string()
            } else {
                format!("v{source}")
            };
            if shift < 0 {
                format!("({source} >> {})", -shift)
            } else {
                format!("({source} << {shift})")
            }
        };
        for connected in [false, true] {
            let mut code = format!(
                "module Network (i: input logic<{width}>, enables: input logic<4>, o: output logic<{width}>) {{\n"
            );
            for index in 0..4 {
                code.push_str(&format!(
                    "var v{index}: logic<{width}>;\nvar next{index}: logic<{width}>;\n"
                ));
            }
            for (index, arms) in choices.iter().enumerate() {
                code.push_str(&format!(
                    "assign next{index} = (if enables[{index}] ? {} : {}) | i;\nassign v{index} = next{index};\n",
                    expression(arms[1]),
                    expression(arms[0])
                ));
            }
            code.push_str(if connected {
                "assign o = v0;\n}\n"
            } else {
                "assign o = i;\n}\n"
            });
            check(&code, expected_loop);
            code.push_str(&format!(r#"
                module Top (i: input logic<{width}>, enables: input logic<4>, o: output logic<{width}>) {{
                    inst child: Network (i: i, enables: enables, o: o);
                }}
            "#));
            check(&code, expected_loop);
        }
    }
    assert!(
        (16..112).contains(&cyclic),
        "both cyclic and acyclic networks must be exercised: {cyclic}"
    );
}
