use super::*;

#[test]
fn dynamic_packed_member_read_does_not_depend_on_adjacent_result() {
    let code = r#"
        module Top (index: input logic<2>, o: output logic) {
            struct Packet {
                payload: logic<4>,
                result: logic,
            }
            var packet: Packet;
            assign packet.payload = 0;
            assign packet.result = packet.payload[index];
            assign o = packet.result;
        }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(comb_loop_analysis_is_complete(code));
}

#[test]
fn dynamic_packed_member_read_preserves_feedback() {
    let code = r#"
        module Top (index: input logic<2>, o: output logic) {
            struct Packet {
                payload: logic<4>,
                result: logic,
            }
            var packet: Packet;
            assign packet.payload = {3'b0, packet.result};
            assign packet.result = packet.payload[index];
            assign o = packet.result;
        }
    "#;
    let errors = analyze(code);
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
    assert!(comb_loop_analysis_is_complete(code));
}

#[test]
fn dynamic_packed_member_writes_in_separate_processes_do_not_conflict() {
    let errors = analyze(
        r#"
        module Top (index: input logic<2>, o: output logic<8>) {
            struct Pair {
                selected: logic<4>,
                other: logic<4>,
            }
            var pair: Pair;
            always_comb {
                pair.selected = 0;
                pair.selected[index] = 1;
            }
            always_comb {
                pair.other = 0;
            }
            assign o = {pair.selected, pair.other};
        }
    "#,
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn dynamic_packed_member_read_before_assignment_is_reported() {
    let errors = analyze(
        r#"
        module Top (index: input logic<2>, o: output logic) {
            struct Pair {
                selected: logic<4>,
                other: logic<4>,
            }
            var pair: Pair;
            always_comb {
                pair.other = 0;
                o = pair.selected[index];
                pair.selected = 0;
            }
        }
    "#,
    );
    assert!(
        matches!(errors.as_slice(), [AnalyzerError::UnassignVariable { identifier, .. }] if identifier == "pair"),
        "{errors:#?}"
    );
}

#[test]
fn dynamic_packed_member_read_after_assignment_ignores_unwritten_neighbor() {
    let errors = analyze(
        r#"
        module Top (index: input logic<2>, o: output logic) {
            struct Pair {
                selected: logic<4>,
                other: logic<4>,
            }
            var pair: Pair;
            always_comb {
                pair.selected = 0;
                o = pair.selected[index];
                pair.other = 0;
            }
        }
    "#,
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn dynamic_packed_member_writes_preserve_same_member_conflicts() {
    let errors = analyze(
        r#"
        module Top (index: input logic<2>) {
            struct Pair {
                selected: logic<4>,
                other: logic<4>,
            }
            var pair: Pair;
            always_comb {
                pair.selected = 0;
                pair.selected[index] = 1;
            }
            always_comb {
                pair.selected = 0;
                pair.other = 0;
            }
        }
    "#,
    );
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .all(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
        "{errors:#?}"
    );
}

#[test]
fn dynamic_packed_member_read_through_union_preserves_aliases() {
    for (source, feedback) in [("{4'b0, o}", false), ("{3'b0, o, 1'b0}", true)] {
        let code = format!(
            r#"
            module Top (index: input logic<2>, o: output logic) {{
                struct Packet {{
                    payload: logic<4>,
                    result: logic,
                }}
                union Overlay {{
                    pair: Packet,
                    raw: logic<5>,
                }}
                var packet: Overlay;
                assign packet.raw = {source};
                assign o = packet.pair.payload[index];
            }}
        "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
            "{code}\n{errors:#?}"
        );
        assert_eq!(!errors.is_empty(), feedback, "{code}\n{errors:#?}");
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn dynamic_packed_member_read_keeps_constant_outer_coordinates() {
    for (row, feedback) in [(1, false), (0, true)] {
        let code = format!(
            r#"
            module Top (index: input logic<2>, o: output logic) {{
                struct Packet {{
                    payload: logic<2, 4>,
                    result: logic,
                }}
                var packet: Packet;
                assign packet.payload[0] = {{3'b0, packet.result}};
                assign packet.payload[1] = 0;
                assign packet.result = packet.payload[{row}][index];
                assign o = packet.result;
            }}
        "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
            "{code}\n{errors:#?}"
        );
        assert_eq!(!errors.is_empty(), feedback, "{code}\n{errors:#?}");
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn dynamic_packed_member_read_with_dynamic_outer_coordinate_keeps_feedback() {
    let code = r#"
        module Top (lane: input logic, index: input logic<2>, o: output logic) {
            struct Packet {
                payload: logic<4>,
                result: logic,
            }
            var packet: Packet<2>;
            assign packet[0].payload = 0;
            assign packet[0].result = 0;
            assign packet[1].payload = {3'b0, o};
            assign packet[1].result = 0;
            assign o = packet[lane].payload[index];
        }
    "#;
    let errors = analyze(code);
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
    assert!(comb_loop_analysis_is_complete(code));
}

#[test]
fn dynamic_packed_member_part_select_stays_in_its_member() {
    for select in ["index +: 2", "index -: 2"] {
        let code = format!(
            r#"
            module Top (index: input logic<2>, o: output logic<2>) {{
                struct Packet {{
                    payload: logic<4>,
                    result: logic<2>,
                }}
                var packet: Packet;
                assign packet.payload = 0;
                assign packet.result = packet.payload[{select}];
                assign o = packet.result;
            }}
        "#
        );
        let errors = analyze(&code);
        assert!(errors.is_empty(), "{select}: {errors:#?}");
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn dynamic_packed_member_read_through_module_input() {
    let code = r#"
        module Copy (i: input logic, o: output logic) {
            assign o = i;
        }
        module Top (index: input logic<2>, o: output logic) {
            struct Packet {
                payload: logic<4>,
                result: logic,
            }
            var packet: Packet;
            assign packet.payload = 0;
            inst copy: Copy (i: packet.payload[index], o: packet.result);
            assign o = packet.result;
        }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(comb_loop_analysis_is_complete(code));
}

#[test]
fn dynamic_packed_member_read_through_function_summary() {
    for (member, feedback) in [("payload", false), ("result", true)] {
        let code = format!(
            r#"
            module Top (index: input logic<2>, o: output logic) {{
                struct Packet {{
                    payload: logic<4>,
                    result: logic<4>,
                }}
                function pick (p: input Packet, index: input logic<2>) -> logic {{
                    return p.{member}[index];
                }}
                var left: Packet;
                var right: Packet;
                assign left = Packet'{{payload: 0, result: right.result}};
                assign right = Packet'{{payload: 0, result: pick(left, index)}};
                assign o = right.result[0];
            }}
        "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .all(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
            "{code}\n{errors:#?}"
        );
        assert_eq!(!errors.is_empty(), feedback, "{code}\n{errors:#?}");
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn dynamic_packed_member_function_output_stays_in_its_member() {
    let errors = analyze(
        r#"
        module Top (index: input logic<2>, o: output logic<8>) {
            struct Pair {
                selected: logic<4>,
                other: logic<4>,
            }
            function put (value: output logic) {
                value = 1;
            }
            var pair: Pair;
            always_comb {
                pair.selected = 0;
                put(pair.selected[index]);
            }
            always_comb {
                pair.other = 0;
            }
            assign o = {pair.selected, pair.other};
        }
    "#,
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn dynamic_packed_member_union_writes_still_conflict() {
    let errors = analyze(
        r#"
        module Top (index: input logic<2>) {
            struct Pair {
                selected: logic<4>,
                other: logic<4>,
            }
            union Overlay {
                pair: Pair,
                raw: logic<8>,
            }
            var data: Overlay;
            always_comb {
                data.pair.selected = 0;
                data.pair.selected[index] = 1;
            }
            always_comb {
                data.raw = 0;
            }
        }
    "#,
    );
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .all(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
        "{errors:#?}"
    );
}
