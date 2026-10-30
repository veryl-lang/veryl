use super::*;
use crate::BigUint;

#[test]
fn generic_interface_port_dynamic_unknown_direction() {
    for default in ["", " = 1'b0", " = 1'b1"] {
        let code = format!(
            r#"
            interface Bus {{ var data: logic<8>; }}
            module Child (bus: interface) {{ inst u: $sv::Consumer (data: bus.data); }}
            module Pass (bus: interface) {{ inst u: Child (bus); }}
            module Top (index: input logic{default}, data: output logic<8>) {{
                inst bus: Bus [2];
                inst u: Pass (bus: bus[index]);
                assign bus[0].data = 8'h12;
                assign bus[1].data = 8'h34;
                assign data = bus[1].data;
            }}
        "#
        );
        let errors = analyze(&code);
        assert!(
            matches!(
                errors.as_slice(),
                [AnalyzerError::NonConstantInterfaceInstanceSelect { .. }]
            ),
            "default {default}: {errors:?}"
        );
    }
}

#[test]
fn generic_interface_port_aliased_writers() {
    for (body, conflict) in [
        ("assign a.data = 8'h12; assign b.data = 8'h34;", true),
        (
            "assign a.data[3:0] = 4'h2; assign b.data[7:4] = 4'h3;",
            false,
        ),
        (
            "assign a.data[3:0] = 4'h2; assign b.data[4:2] = 3'h3;",
            true,
        ),
        ("always_comb { a.data = 8'h12; b.data = 8'h34; }", false),
        (
            "inst u: $sv::Consumer (data: a.data); assign b.data = 8'h34;",
            false,
        ),
    ] {
        for forwarded in [false, true] {
            let connection = if forwarded {
                "inst u: Pass (a: bus, b: bus);"
            } else {
                "inst u: Child (a: bus, b: bus);"
            };
            let code = format!(
                r#"
                interface Bus {{ var data: logic<8>; }}
                module Child (a: interface, b: interface) {{ {body} }}
                module Pass (a: interface, b: interface) {{ inst u: Child (a, b); }}
                module Top (data: output logic<8>) {{
                    inst bus: Bus;
                    {connection}
                    assign data = bus.data;
                }}
            "#
            );
            let errors = analyze(&code);
            assert_eq!(
                errors
                    .iter()
                    .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
                conflict,
                "{body}, forwarded={forwarded}: {errors:?}"
            );
            if !conflict {
                assert!(errors.is_empty(), "{errors:?}");
            }
        }
    }
}

#[test]
fn generic_interface_port_array_slices() {
    for (slice, base) in [("1:2", 1), ("1+:2", 1), ("2-:2", 1), ("1 step 2", 2)] {
        for conflict in [false, true] {
            let independent = (0..4)
                .filter(|i| *i < base || *i >= base + 2)
                .map(|i| format!("assign bus[{i}].data = '0;"))
                .collect::<Vec<_>>()
                .join("\n");
            let overlap = if conflict {
                format!("assign bus[{base}].data = '0;")
            } else {
                String::new()
            };
            let code = format!(
                r#"
            interface Bus {{ var data: logic; }}
            module Child (bus: interface [2]) {{
                assign bus[0].data = 1'b1;
                assign bus[1].data = 1'b0;
            }}
            module Pass (bus: interface [2]) {{ inst u: Child (bus); }}
            module Top (data: output logic [4]) {{
                inst bus: Bus [4];
                inst u: Pass (bus: bus[{slice}]);
                {independent}
                {overlap}
                for i in 0..4 :g {{ assign data[i] = bus[i].data; }}
            }}
            "#
            );
            let errors = analyze(&code);
            assert_eq!(
                errors
                    .iter()
                    .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
                conflict,
                "{slice}: {errors:?}"
            );
            if !conflict {
                assert!(errors.is_empty(), "{slice}: {errors:?}");
            }
        }
    }
}

#[test]
fn generic_interface_port_initialization() {
    for (runtime, parent, conflict) in [
        ("", "bus.data = 8'hff", false),
        (
            "assign bus.data[3:0] = 4'h5;",
            "bus.data[7:4] = 4'ha",
            false,
        ),
        ("assign bus.data[3:0] = 4'h5;", "bus.data = 8'hff", true),
    ] {
        let code = format!(
            r#"
        interface Bus {{ var data: logic<8>; }}
        module Child (#[allow(initial_assign)] bus: interface) {{
            initial {{ bus.data = '0; }}
            {runtime}
        }}
        module Pass (bus: interface) {{ inst u: Child (bus); }}
        module Top (data: output logic<8>) {{
            inst bus: Bus;
            inst u: Pass (bus);
            assign {parent};
            assign data = bus.data;
        }}
        "#
        );
        let errors = analyze(&code);
        assert_eq!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
            conflict,
            "{errors:?}"
        );
        if !conflict {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}

#[test]
fn generic_interface_port_dynamic_default() {
    for default in ["", " = 1'b0", " = 1'b1"] {
        for (shape, binding, target, independent, conflict) in [
            ("[2]", "bus[index]", "bus[1]", "", true),
            ("[2, 2]", "bus[0][index]", "bus[0][1]", "", true),
            (
                "[2, 2]",
                "bus[0][index]",
                "bus[1][1]",
                "assign bus[1][0].data = '0;",
                false,
            ),
        ] {
            let code = format!(
                r#"
        interface Bus {{ var data: logic; }}
        module Child (bus: interface) {{ assign bus.data = 1'b1; }}
        module Top (index: input logic{default}, data: output logic) {{
            inst bus: Bus {shape};
            inst u: Child (bus: {binding});
            assign {target}.data = 1'b0;
            {independent}
            assign data = {target}.data;
        }}
        "#
            );
            let errors = analyze(&code);
            // A default value does not make an input an elaboration-time constant.
            assert_eq!(
                errors
                    .iter()
                    .filter(|e| {
                        matches!(e, AnalyzerError::NonConstantInterfaceInstanceSelect { .. })
                    })
                    .count(),
                1,
                "{binding}, default {default}: {errors:?}"
            );
            assert_eq!(
                errors
                    .iter()
                    .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
                conflict,
                "{binding}, default {default}: {errors:?}"
            );
            if !conflict {
                assert_eq!(errors.len(), 1, "{errors:?}");
            }
        }
    }
}

#[test]
fn generic_interface_port_disjoint_writes() {
    for (ty, child, parent) in [
        ("logic<8>", "bus.data[3:0] = 4'h5", "bus.data[7:4] = 4'ha"),
        ("logic<8> [2]", "bus.data[0] = 8'h12", "bus.data[1] = 8'h34"),
    ] {
        let code = format!(
            r#"
        interface Bus {{ var data: {ty}; }}
        module Child (bus: interface) {{ assign {child}; }}
        module Pass (bus: interface) {{ inst u: Child (bus); }}
        module Top {{
            inst bus: Bus;
            inst u: Pass (bus);
            assign {parent};
        }}
        "#
        );
        assert!(analyze(&code).is_empty());
    }
}

#[test]
fn generic_interface_port_unknown_sv_direction() {
    for connection in ["data: bus.data", "bus"] {
        let code = format!(
            r#"
        interface Bus {{ var data: logic<8>; }}
        module Child (bus: interface) {{ inst u: $sv::Consumer ({connection}); }}
        module Pass (bus: interface) {{ inst u: Child (bus); }}
        module Top {{
            inst bus: Bus;
            assign bus.data = 8'h12;
            inst u: Pass (bus);
        }}
        "#
        );
        assert!(analyze(&code).is_empty());
    }
}

#[test]
fn generic_interface_port_conflicting_writes() {
    for (ty, child, parent) in [
        ("logic<8>", "bus.data[3:0] = 4'h5", "bus.data[4:2] = 3'h0"),
        ("logic<8> [2]", "bus.data[0] = 8'h12", "bus.data[0] = 8'h34"),
    ] {
        let code = format!(
            r#"
        interface Bus {{ var data: {ty}; }}
        module Child (bus: interface) {{ assign {child}; }}
        module Pass (bus: interface) {{ inst u: Child (bus); }}
        module Top {{
            inst bus: Bus;
            inst u: Pass (bus);
            assign {parent};
        }}
        "#
        );
        assert!(
            analyze(&code)
                .iter()
                .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. }))
        );
    }
}

#[test]
fn generic_interface_port_mixed_sv_and_definite_writes() {
    for (parent, conflict) in [
        ("bus.data[7:4] = 4'ha", false),
        ("bus.data[3:0] = 4'ha", true),
    ] {
        let code = format!(
            r#"
        interface Bus {{ var data: logic<8>; }}
        module Child (bus: interface) {{
            inst sv: $sv::Consumer (data: bus.data);
            assign bus.data[3:0] = 4'h5;
        }}
        module Pass (bus: interface) {{ inst u: Child (bus); }}
        module Top {{
            inst bus: Bus;
            assign {parent};
            inst u: Pass (bus);
        }}
        "#
        );
        let errors = analyze(&code);
        assert_eq!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
            conflict
        );
        if !conflict {
            assert!(errors.is_empty());
        }
    }
}

#[test]
fn generic_interface_port_dynamic_write_conflicts() {
    let code = r#"
    interface Bus { var data: logic<8> [2]; }
    module Child (bus: interface, index: input logic) {
        always_comb { bus.data[index] = 8'h12; }
    }
    module Pass (bus: interface, index: input logic) { inst u: Child (bus, index); }
    module Top (index: input logic) {
        inst bus: Bus;
        inst u: Pass (bus, index);
        assign bus.data[1] = 8'h34;
    }
    "#;
    assert!(
        analyze(code)
            .iter()
            .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. }))
    );
}

#[test]
fn generic_interface_port_mixed_sv_and_dynamic_writes() {
    for (field, conflict) in [("lo", false), ("hi", true)] {
        let code = format!(
            r#"
        package Types {{ struct Packet {{ lo: logic<4>, hi: logic<4>, }} }}
        interface Bus {{ var data: Types::Packet; }}
        module Child (bus: interface) {{
            inst sv: $sv::Consumer (data: bus.data);
            assign bus.data.{field} = 4'h5;
        }}
        module Pass (bus: interface) {{ inst u: Child (bus); }}
        module Top (index: input logic<2>) {{
            inst bus: Bus;
            always_comb {{ bus.data.hi[index] = 1'b1; }}
            inst u: Pass (bus);
        }}
        "#
        );
        let errors = analyze(&code);
        assert_eq!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
            conflict
        );
        if !conflict {
            assert!(errors.is_empty());
        }
    }
}

#[test]
fn generic_interface_port_preserves_feedthrough_dependencies() {
    for (source, feedback) in [("1'b1", false), ("bus.response", true)] {
        let code = format!(
            r#"
        interface Bus {{ var request: logic; var response: logic; }}
        module Child (bus: interface) {{ assign bus.response = bus.request; }}
        module Pass (bus: interface) {{ inst u: Child (bus); }}
        module Top (result: output logic) {{
            inst bus: Bus;
            inst u: Pass (bus);
            assign bus.request = {source};
            assign result = bus.response;
        }}
        "#
        );
        let errors = analyze(&code);
        assert_eq!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
            feedback
        );
        if !feedback {
            assert!(errors.is_empty());
        }
    }
}

const CHILD_LOOP: &str = r#"
interface Bus { var data: logic<8> [4]; }
module Child (bus: interface) {
    always_comb {
        for i in 0..COUNT {
            bus.data[i] = 8'h3c;
        }
    }
}
module Pass (bus: interface) { inst u: Child (bus); }
module Top (out: output logic<8>) {
    inst bus: Bus;
    inst u: Pass (bus);
    PARENT
    assign out = bus.data[1];
}
"#;

#[test]
fn generic_interface_port_loop_writes_only_its_iterations() {
    let code =
        |count: &str, parent: &str| CHILD_LOOP.replace("COUNT", count).replace("PARENT", parent);
    let unassigned = |errors: &[AnalyzerError]| {
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnassignVariable { .. }))
    };

    // `bus.data[1]` is read but only `bus.data[0]` is written.
    let errors = analyze(&code("1", ""));
    assert!(unassigned(&errors), "{errors:?}");
    let errors = analyze(&code("4", ""));
    assert!(!unassigned(&errors), "{errors:?}");

    // The loop still claims the whole array against other drivers.
    let errors = analyze(&code("1", "assign bus.data[3] = 8'h00;"));
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
        "{errors:?}"
    );
}

#[test]
fn generic_interface_port_loop_drives_only_its_iterations() {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let code = CHILD_LOOP.replace("COUNT", "2").replace("PARENT", "");
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir));

    // The specialization instantiated by `Pass`, where `bus.data` is the
    // only member; the top-level `Child` has no actual interface.
    let (child, data) = ir
        .module_postorder()
        .into_iter()
        .filter(|x| x.name.to_string() == "Child")
        .find_map(|x| {
            let data = x
                .variables
                .keys()
                .find(|id| x.is_direction_free_member(**id))?;
            Some((x, *data))
        })
        .unwrap();
    let full = BigUint::from(0xffu32);
    let none = BigUint::default();
    assert_eq!(
        child.write_summary.driven_masks(data, 4),
        vec![full.clone(), full, none.clone(), none]
    );
}
