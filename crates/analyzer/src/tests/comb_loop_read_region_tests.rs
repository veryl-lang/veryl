use super::*;
use crate::comb_loop_detect::{analysis_size, reset_analysis_size};

fn assert_exact_diagnostic(code: &str, expected_loop: bool) {
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}\n{code}"
    );
    assert_eq!(!errors.is_empty(), expected_loop, "{errors:#?}\n{code}");
    assert!(comb_loop_analysis_is_complete(code), "{code}");
}

#[test]
fn demanded_read_views_do_not_form_a_cartesian_storage_partition() {
    for count in [8, 64, 256] {
        for copy in [false, true] {
            let (input, storage) = if copy {
                (
                    "data",
                    format!("var mem: logic<{count}>[{count}]; assign mem = data;"),
                )
            } else {
                ("mem", String::new())
            };
            let reads = (0..count)
                .map(|i| format!("assign o[{i}] = mem[{i}][{i}] ^ mem[index][{i}];"))
                .collect::<Vec<_>>()
                .join("\n");
            let code = format!(
                "module Views(index: input u32, {input}: input logic<{count}>[{count}], o: output logic<{count}>) {{ {storage} {reads} }}"
            );
            reset_analysis_size();
            let errors = analyze(&code);
            assert!(errors.is_empty(), "count={count}, copy={copy}: {errors:?}");
            let (atoms, nodes, edges) = analysis_size();
            assert_eq!(
                atoms,
                count + 2 + usize::from(copy),
                "count={count}, copy={copy}"
            );
            assert!(
                nodes <= 16 * count + 8,
                "count={count}, copy={copy}: {nodes} nodes"
            );
            assert!(
                edges <= 20 * count + 8,
                "count={count}, copy={copy}: {edges} edges"
            );
            assert!(
                comb_loop_analysis_is_complete(&code),
                "count={count}, copy={copy}"
            );
        }
    }
}

#[test]
fn demanded_read_regions_clip_reductions_of_whole_storage_writes() {
    for bit in 0..4 {
        for function in [false, true] {
            let (declaration, read) = if function {
                (
                    "function reduce(x: input logic<4>) -> logic { return |x[1:0]; }",
                    "reduce(word)",
                )
            } else {
                ("", "|word[1:0]")
            };
            let code = format!(
                "module Top(o: output logic) {{
                var word: logic<4>;
                {declaration}
                assign word = (o as 4) << {bit};
                assign o = {read};
            }}"
            );
            assert_exact_diagnostic(&code, bit < 2);
        }
    }
}

#[test]
fn demanded_read_regions_survive_module_summary_projection() {
    for bit in 0..4 {
        let code = format!("module Reduce(i: input logic<4>, o: output logic) {{ assign o = |i[1:0]; }}
            module Wrapper(i: input logic<4>, o: output logic) {{ inst reduce: Reduce(i: i, o: o); }}
            module Top(o: output logic) {{
                var word: logic<4>;
                assign word = (o as 4) << {bit};
                inst wrapper: Wrapper(i: word, o: o);
            }}");
        assert_exact_diagnostic(&code, bit < 2);
    }
}

#[test]
fn demanded_read_regions_keep_array_and_packed_axes_distinct() {
    for row in 0..2 {
        for bit in 0..4 {
            for dynamic in [false, true] {
                let read = if dynamic {
                    "mem[index][1:0]"
                } else {
                    "mem[0][1:0]"
                };
                let code = format!(
                    "module Top(index: input bit, o: output logic) {{
                    var mem: logic<4>[2];
                    assign mem[0] = {zero};
                    assign mem[1] = {one};
                    assign o = |{read};
                }}",
                    zero = if row == 0 {
                        format!("(o as 4) << {bit}")
                    } else {
                        "0".into()
                    },
                    one = if row == 1 {
                        format!("(o as 4) << {bit}")
                    } else {
                        "0".into()
                    }
                );
                assert_exact_diagnostic(&code, bit < 2 && (dynamic || row == 0));
            }
        }
    }
}

#[test]
fn demanded_read_regions_preserve_partial_overwrite_boundaries() {
    for overwrite in [false, true] {
        let write = if overwrite {
            "word[1:0] = 0;"
        } else {
            "word[3:2] = 0;"
        };
        let code = format!(
            "module Top(o: output logic) {{
            var word: logic<4>;
            var feedback: logic;
            assign feedback = o;
            always_comb {{
                word = {{feedback repeat 4}};
                {write}
                o = |word[1:0];
            }}
        }}"
        );
        assert_exact_diagnostic(&code, !overwrite);
    }
}

#[test]
fn demanded_read_regions_share_translations_in_large_array_rotations() {
    for count in [64, 256] {
        for wrap in [false, true] {
            let mut assignments = String::new();
            for index in 0..count {
                let target = (index + 3) % count;
                let source = if index + 3 < count || wrap { "a" } else { "i" };
                assignments += &format!("assign b[{index}] = {source}[{target}];\n");
            }
            let code = format!(
                "module Top(i: input logic[{count}], o: output logic[{count}]) {{
                var a: logic[{count}];
                var b: logic[{count}];
                assign a = b;
                assign o = a;
                {assignments}
            }}"
            );
            assert_exact_diagnostic(&code, wrap);
        }
    }
}

#[test]
fn demanded_read_regions_keep_selector_dependencies_across_instances() {
    for feedback in [false, true] {
        let index = if feedback { "o" } else { "selector" };
        let code = format!(
            "module Select(index: input logic, data: input logic[2], o: output logic) {{
                assign o = data[index];
            }}
            module Top(selector: input logic, data: input logic[2], o: output logic) {{
                inst pick: Select(index: {index}, data: data, o: o);
            }}"
        );
        assert_exact_diagnostic(&code, feedback);
    }
}

#[test]
fn demanded_read_regions_bound_actuals_before_whole_value_child_dependencies() {
    for bit in 0..16 {
        let code = format!(
            "module Add(i: input logic<8>, o: output logic<8>) {{
                assign o = i + 8'd1;
            }}
            module Top(o: output logic<8>) {{
                var word: logic<16>;
                assign word = (o as 16) << {bit};
                inst add: Add(i: word[7:0], o: o);
            }}"
        );
        assert_exact_diagnostic(&code, bit < 8);
    }
}

#[test]
fn demanded_read_regions_bound_expanded_unpacked_input_slices() {
    for row in 0..4 {
        for bit in 0..4 {
            let elements = (0..4)
                .map(|i| {
                    if i == row {
                        format!("(o as 4) << {bit}")
                    } else {
                        "4'b0".to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let code = format!(
                "module Reduce(i: input logic<4>[2], o: output logic) {{
                    assign o = |i[0][1:0];
                }}
                module Top(o: output logic) {{
                    var data: logic<4>[4];
                    assign data = '{{{elements}}};
                    inst reduce: Reduce(i: data[1+:2], o: o);
                }}"
            );
            assert_exact_diagnostic(&code, row == 1 && bit < 2);
        }
    }
}
