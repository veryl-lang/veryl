use super::*;

#[test]
fn output_array_slice_preserves_element_positions() {
    for feedback in [false, true] {
        let read = if feedback { 1 } else { 2 };
        let code = format!(
            "module Forward(i: input logic, o: output logic[2]) {{
                assign o[0] = i;
                assign o[1] = 0;
            }}
            module Top(o: output logic) {{
                var returned: logic[4];
                inst forward: Forward(i: o, o: returned[1+:2]);
                assign returned[0] = 0;
                assign returned[3] = 0;
                assign o = returned[{read}];
            }}"
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
            "{errors:?}\n{code}"
        );
        assert_eq!(!errors.is_empty(), feedback, "{errors:?}\n{code}");
        assert!(comb_loop_analysis_is_complete(&code), "{code}");
    }
}

#[test]
fn output_multidimensional_slice_preserves_rows_columns_and_bits() {
    for row in 0..4 {
        for column in 0..2 {
            for bit in 0..4 {
                let code = format!(
                    "module Scatter(i: input logic, o: output logic<4>[2, 2]) {{
                        assign o = '{{'{{4'b0, (i as 4) << 1}}, '{{4'b0, 4'b0}}}};
                    }}
                    module Top(o: output logic) {{
                        var returned: logic<4>[4, 2];
                        inst scatter: Scatter(i: o, o: returned[1+:2]);
                        assign returned[0] = '{{default: 4'b0}};
                        assign returned[3] = '{{default: 4'b0}};
                        assign o = returned[{row}][{column}][{bit}];
                    }}"
                );
                let errors = analyze(&code);
                assert!(
                    errors
                        .iter()
                        .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
                    "{errors:?}\n{code}"
                );
                assert_eq!(
                    !errors.is_empty(),
                    row == 1 && column == 1 && bit == 1,
                    "{errors:?}\n{code}"
                );
                assert!(comb_loop_analysis_is_complete(&code), "{code}");
            }
        }
    }
}

#[test]
fn output_concatenation_preserves_fragment_order_and_selected_offsets() {
    let reads = [
        "word[0]", "word[1]", "spare[1]", "spare[2]", "word[4]", "word[5]", "word[6]",
    ];
    for wrapper in [false, true] {
        let target = if wrapper { "Wrapper" } else { "Scatter" };
        for driven in 0..7 {
            for (read_bit, read) in reads.iter().enumerate() {
                let code = format!(
                    "module Scatter(i: input logic, o: output logic<7>) {{
                        assign o = (i as 7) << {driven};
                    }}
                    module Wrapper(i: input logic, o: output logic<7>) {{
                        inst scatter: Scatter(i: i, o: {{o[6:4], o[3:2], o[1:0]}});
                    }}
                    module Top(o: output logic) {{
                        var word: logic<7>;
                        var spare: logic<4>;
                        inst scatter: {target}(i: o, o: {{word[6:4], spare[2:1], word[1:0]}});
                        assign word[3:2] = 0;
                        assign spare[3] = 0;
                        assign spare[0] = 0;
                        assign o = {read};
                    }}"
                );
                let errors = analyze(&code);
                assert!(
                    errors
                        .iter()
                        .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
                    "{errors:?}\n{code}"
                );
                assert_eq!(!errors.is_empty(), read_bit == driven, "{errors:?}\n{code}");
                assert!(comb_loop_analysis_is_complete(&code), "{code}");
            }
        }
    }
}

#[test]
fn modport_output_slice_preserves_element_positions() {
    for multidimensional in [false, true] {
        for offset in [0, 1] {
            for feedback in [false, true] {
                let (shape, prefix) = if multidimensional {
                    ("[1, 4]", "[0]")
                } else {
                    ("[4]", "")
                };
                let read = if feedback { offset } else { offset + 1 };
                let assignments = (0..4)
                    .map(|index| {
                        let request = if index == offset {
                            format!("bus{prefix}[{read}].response")
                        } else {
                            "0".to_owned()
                        };
                        let response = if index < offset || index >= offset + 2 {
                            format!("assign bus{prefix}[{index}].response = 0;")
                        } else {
                            String::new()
                        };
                        format!("assign bus{prefix}[{index}].request = {request}; {response}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let code = format!(
                    "interface Bus {{
                        var request: logic;
                        var response: logic;
                        modport target {{request: input, response: output}}
                    }}
                    module Targets(bus: modport Bus::target[2]) {{
                        assign bus[0].response = bus[0].request;
                        assign bus[1].response = bus[1].request;
                    }}
                    module Top {{
                        inst bus: Bus{shape};
                        inst targets: Targets(bus: bus{prefix}[{offset}+:2]);
                        {assignments}
                    }}"
                );
                let errors = analyze(&code);
                assert!(
                    errors
                        .iter()
                        .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
                    "{errors:?}\n{code}"
                );
                assert_eq!(!errors.is_empty(), feedback, "{errors:?}\n{code}");
                assert!(comb_loop_analysis_is_complete(&code), "{code}");
            }
        }
    }
}

#[test]
fn output_fragments_project_the_corresponding_input_expression() {
    for driven in 0..4 {
        for read in 0..4 {
            let code = format!(
                "module Forward(i: input logic<4>, o: output logic<4>) {{ assign o = i; }}
                module Top(o: output logic) {{
                    var returned: logic<4>;
                    inst forward: Forward(i: (o as 4) << {driven}, o: {{returned[1:0], returned[3:2]}});
                    assign o = returned[{read}];
                }}"
            );
            let errors = analyze(&code);
            assert!(
                errors
                    .iter()
                    .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
                "{errors:?}\n{code}"
            );
            assert_eq!(
                !errors.is_empty(),
                read == (driven + 2) % 4,
                "{errors:?}\n{code}"
            );
            assert!(comb_loop_analysis_is_complete(&code), "{code}");
        }
    }
}

#[test]
fn output_fragments_retain_repeated_dependencies() {
    for array in [false, true] {
        let (shape, repeated, connection) = if array {
            ("logic[4]", "'{i repeat 4}", "returned[1+:4]")
        } else {
            ("logic<4>", "{i repeat 4}", "{returned[4:3], returned[2:1]}")
        };
        let parent_shape = if array { "logic[6]" } else { "logic<6>" };
        for read in 0..6 {
            let code = format!(
                "module Fanout(i: input logic, o: output {shape}) {{ assign o = {repeated}; }}
                module Top(o: output logic) {{
                    var returned: {parent_shape};
                    inst fanout: Fanout(i: o, o: {connection});
                    assign returned[0] = 0;
                    assign returned[5] = 0;
                    assign o = returned[{read}];
                }}"
            );
            let errors = analyze(&code);
            assert!(
                errors
                    .iter()
                    .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
                "{errors:?}\n{code}"
            );
            assert_eq!(
                !errors.is_empty(),
                (1..5).contains(&read),
                "{errors:?}\n{code}"
            );
            assert!(comb_loop_analysis_is_complete(&code), "{code}");
        }
    }
}

#[test]
fn output_array_slice_keeps_independent_lanes_linear() {
    use crate::comb_loop_detect::{analysis_size, reset_analysis_size};

    for count in [64, 256] {
        let code = format!(
            "module Forward(i: input logic[{count}], o: output logic[{count}]) {{ assign o = i; }}
            module Top(i: input logic[{count}], o: output logic[{count}]) {{
                inst forward: Forward(i: i, o: o[0+:{count}]);
            }}"
        );
        reset_analysis_size();
        let errors = analyze(&code);
        assert!(errors.is_empty(), "{errors:?}");
        let (atoms, nodes, edges) = analysis_size();
        assert!(atoms <= count + 4, "{count}: {atoms} atoms");
        assert!(nodes <= 8 * count + 8, "{count}: {nodes} nodes");
        assert!(edges <= 8 * count + 8, "{count}: {edges} edges");
        assert!(comb_loop_analysis_is_complete(&code), "{code}");
    }
}
