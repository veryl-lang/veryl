use super::*;

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
