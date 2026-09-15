use super::*;

#[test]
fn modport_connection_explicit_and_bare_outputs() {
    for connection in ["bus.mp", "bus", "mp"] {
        let code = format!(
            r#"
            interface Bus {{
                var data: logic;
                modport mp {{ data: output, }}
            }}
            module Writer (bus: modport Bus::mp) {{
                assign bus.data = 0;
            }}
            module Top {{
                inst {instance}: Bus;
                inst writer: Writer(bus: {connection});
            }}
            "#,
            instance = if connection == "mp" { "mp" } else { "bus" },
        );
        assert!(analyze(&code).is_empty(), "{connection}");
    }
}

#[test]
fn modport_connection_forwarded_port_named_like_modport() {
    let code = r#"
        interface Bus {
            var data: logic;
            modport mp { data: output, }
        }
        module Writer (bus: modport Bus::mp) {
            assign bus.data = 0;
        }
        module Forward (mp: modport Bus::mp) {
            inst writer: Writer(bus: mp);
        }
        module Top {
            inst bus: Bus;
            inst forward: Forward(mp: bus.mp);
        }
    "#;
    assert!(analyze(code).is_empty());
}

#[test]
fn modport_connection_preserves_unassigned_and_multiple_assignment_checks() {
    let code = r#"
        interface Bus {
            var data: logic;
            #[allow(unused_variable)]
            var undriven: logic;
            modport mp { data: output, }
        }
        module Writer (bus: modport Bus::mp) {
            assign bus.data = 0;
        }
        module Top {
            inst bus: Bus;
            inst writer: Writer(bus: bus.mp);
        }
    "#;
    let errors = analyze(code);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        matches!(&errors[0], AnalyzerError::UnassignVariable { identifier, .. } if identifier == "bus.undriven")
    );
    let errors = analyze(&code.replace("inst writer:", "assign bus.data = 1; inst writer:"));
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
        "{errors:?}"
    );
}

#[test]
fn modport_connection_explicit_input_preserves_feedback() {
    let code = r#"
        interface Bus {
            var request: logic;
            var response: logic;
            modport mp { request: input, response: output, }
        }
        module Target (bus: modport Bus::mp) {
            assign bus.response = bus.request;
        }
        module Top {
            inst bus: Bus;
            inst target: Target(bus: bus.mp);
            assign bus.request = bus.response;
        }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "{errors:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnassignVariable { .. })),
        "{errors:?}"
    );
    assert!(
        analyze(&code.replace(
            "assign bus.request = bus.response;",
            "assign bus.request = 0;"
        ))
        .is_empty()
    );
}

#[test]
fn modport_connection_indexed_interface_preserves_selection() {
    let code = r#"
        interface Bus {
            var data: logic;
            modport mp { data: output, }
        }
        module Writer (bus: modport Bus::mp) {
            assign bus.data = 0;
        }
        module Top {
            inst bus: Bus[2];
            inst first: Writer(bus: bus[0].mp);
            inst second: Writer(bus: bus[1].mp);
        }
    "#;
    assert!(analyze(code).is_empty());
    let errors = analyze(&code.replace("inst second: Writer(bus: bus[1].mp);", ""));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        matches!(&errors[0], AnalyzerError::UnassignVariable { identifier, .. } if identifier == "bus.data[32'h00000001]")
    );
}

#[test]
fn modport_connection_imported_function_preserves_capture_binding() {
    let code = r#"
        interface Bus {
            var data: logic;
            function read () -> logic { return data; }
            modport mp { read: import, }
        }
        module Reader (bus: modport Bus::mp, o: output logic) {
            assign o = bus.read();
        }
        module Top {
            inst bus: Bus;
            var value: logic;
            inst reader: Reader(bus: bus.mp, o: value);
            assign bus.data = value;
        }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "{errors:?}"
    );
    assert!(analyze(&code.replace("assign bus.data = value;", "assign bus.data = 0;")).is_empty());
}
