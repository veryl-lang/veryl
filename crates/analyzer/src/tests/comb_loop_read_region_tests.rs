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
            // Each variable is one storage region.
            assert_eq!(atoms, 3 + usize::from(copy), "count={count}, copy={copy}");
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

#[test]
fn demanded_read_regions_bound_multidimensional_input_slices() {
    for row in 0..4 {
        for column in 0..2 {
            for bit in 0..4 {
                let assignments = (0..4)
                    .flat_map(|i| {
                        (0..2).map(move |j| {
                            let value = if (i, j) == (row, column) {
                                format!("(o as 4) << {bit}")
                            } else {
                                "4'b0".to_owned()
                            };
                            format!("assign data[{i}][{j}] = {value};")
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let code = format!(
                    "module Reduce(i: input logic<4>[2, 2], o: output logic) {{
                        assign o = |i[0][1][1:0];
                    }}
                    module Top(o: output logic) {{
                        var data: logic<4>[4, 2];
                        {assignments}
                        inst reduce: Reduce(i: data[1+:2], o: o);
                    }}"
                );
                assert_exact_diagnostic(&code, row == 1 && column == 1 && bit < 2);
            }
        }
    }
}

#[test]
fn array_slice_reads_preserve_element_dependencies_in_assignments_and_calls() {
    for connection in [
        "assign selected = data[1+:2];",
        "assign selected = pass(data[1+:2]);",
    ] {
        for position in 0..4 {
            let code = format!(
                "module Top(o: output logic) {{
                    type Pair = logic[2];
                    function pass(i: input logic[2]) -> Pair {{ return i; }}
                    var data: logic[4];
                    var selected: logic[2];
                    assign data = '{{{values}}};
                    {connection}
                    assign o = selected[1];
                }}",
                values = (0..4)
                    .map(|i| if i == position { "o" } else { "1'b0" })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            assert_exact_diagnostic(&code, position == 2);
        }
    }
}

#[test]
fn array_slice_partial_out_of_bounds_preserves_dependency_positions() {
    for selected in 0..2 {
        for feedback in 0..2 {
            for connection in [
                format!(
                    "var selected: logic[2]; assign selected = data[0-:2]; assign o = selected[{selected}];"
                ),
                format!(
                    "var selected: logic[2]; assign selected = pass(data[0-:2]); assign o = selected[{selected}];"
                ),
                "inst u: Pick(i: data[0-:2], o: o);".to_owned(),
            ] {
                let code = format!(
                        "module Pick(i: input logic[2], o: output logic) {{ assign o = i[{selected}]; }}
                        module Top(o: output logic) {{
                            type Pair = logic[2];
                            function pass(i: input logic[2]) -> Pair {{ return i; }}
                            var data: logic[2];
                            assign data = '{{{a}, {b}}};
                            {connection}
                        }}",
                        a = if feedback == 0 { "o" } else { "1'b0" },
                        b = if feedback == 1 { "o" } else { "1'b0" },
                    );
                assert_exact_diagnostic(&code, selected == 1 && feedback == 0);
            }
        }
    }
}

#[test]
fn large_array_slice_reads_stay_compact() {
    for feedback in [0, 1_000_000] {
        let code = format!(
            "module Pick(i: input logic[1000000], o: output logic) {{
                assign o = i[999999];
            }}
            module Top(o: output logic) {{
                var data: logic[1000002];
                assign data[{feedback}] = o;
                inst pick: Pick(i: data[1+:1000000], o: o);
            }}"
        );
        reset_analysis_size();
        assert_exact_diagnostic(&code, feedback == 1_000_000);
        let (atoms, nodes, edges) = analysis_size();
        assert!(
            atoms < 32 && nodes < 64 && edges < 64,
            "{atoms} atoms, {nodes} nodes, {edges} edges"
        );
    }
}

#[test]
fn demanded_read_regions_keep_byte_enabled_descriptor_lookup_compact() {
    // A write-through descriptor table combines per-entry tag comparisons
    // with byte-enabled updates and indexed payload reads. Read boundaries
    // must not split its whole-array next-state copy into entry-by-byte atoms.
    for entries in [64usize, 256, 1024] {
        let address_width = usize::BITS - (entries - 1).leading_zeros();
        let code = format!(
            r#"module DescriptorLookup (
    descriptors: input logic<128>[{entries}],
    write_index: input logic<{address_width}>,
    write_data: input logic<128>,
    byte_enable: input logic<16>,
    search_tag: input logic<32>,
    selected_index: input logic<{address_width}>,
    hit: output logic[{entries}],
    selected_address: output logic<64>,
    selected_length: output logic<16>,
    selected_flags: output logic<16>,
) {{
    var updated: logic<128>[{entries}];
    always_comb {{
        updated = descriptors;
        for lane in 0..16 {{
            if byte_enable[lane] {{
                updated[write_index][lane * 8 +: 8] = write_data[lane * 8 +: 8];
            }}
        }}
    }}
    for row in 0..{entries} :g_match {{
        assign hit[row] = updated[row][31:0] == search_tag;
    }}
    assign selected_address = updated[selected_index][95:32];
    assign selected_length = updated[selected_index][111:96];
    assign selected_flags = updated[selected_index][127:112];
}}
"#
        );
        reset_analysis_size();
        let errors = analyze(&code);
        assert!(errors.is_empty(), "entries={entries}: {errors:#?}");
        // Neither the rows nor the byte lanes cut the storage.
        let (atoms, nodes, edges) = analysis_size();
        assert_eq!(atoms, 11);
        assert!(nodes <= 9 * entries + 256, "{nodes} nodes");
        assert!(edges <= 14 * entries + 256, "{edges} edges");
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn array_slice_in_interface_function_keeps_the_receiver() {
    for position in ["1", "start"] {
        let member = if position == "start" {
            "var start: logic;"
        } else {
            ""
        };
        let starts = if position == "start" {
            "assign bus[0].start = o; assign bus[1].start = 0;"
        } else {
            ""
        };
        for source in 0..2 {
            let code = format!(
                r#"
        interface Bus {{
            var data: logic[3];
            {member}
            function pair(x: input logic[2]) -> logic {{ return x[1]; }}
            function get() -> logic {{ return pair(data[{position}+:2]); }}
        }}
        module Top(o: output logic) {{
            inst bus: Bus[2];
            {starts}
            assign bus[0].data[0] = 0;
            assign bus[0].data[1] = 0;
            assign bus[0].data[2] = o;
            assign bus[1].data = '{{0, 0, 0}};
            assign o = bus[{source}].get();
        }}
        "#
            );
            assert_exact_diagnostic(&code, source == 0);
        }
    }
}
