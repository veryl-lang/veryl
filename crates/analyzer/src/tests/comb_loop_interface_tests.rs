use super::*;

fn assert_interface_comb_loop(code: &str, expected: bool) {
    let errors = analyze(code);
    let detected = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert_eq!(detected, expected, "{errors:#?}");
}

#[test]
fn comb_loop_interface_instance_member_detects_direct_feedback() {
    assert_interface_comb_loop(
        r#"
        interface Bus {
            var value: logic;
        }
        module Top {
            inst bus: Bus;
            assign bus.value = bus.value;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_interface_instance_keeps_distinct_members_independent() {
    assert_interface_comb_loop(
        r#"
        interface Bus {
            var source: logic;
            var sink  : logic;
        }
        module Top {
            inst bus: Bus;
            assign bus.sink = bus.source;
        }
        "#,
        false,
    );
}

#[test]
#[ignore = "comb-loop migration: false negative; interface function member read"]
fn comb_loop_interface_function_read_detects_member_feedback() {
    assert_interface_comb_loop(
        r#"
        interface Bus {
            var value: logic;
            function read () -> logic {
                return value;
            }
        }
        module Top {
            inst bus: Bus;
            assign bus.value = bus.read();
        }
        "#,
        true,
    );
}

fn modport_feedthrough_code(body: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var request : logic<2>;
            var response: logic<2>;
            modport target {{
                request : input,
                response: output,
            }}
        }}
        module Target (
            clk: input clock,
            bus: modport Bus::target,
        ) {{
            {body}
        }}
        module Top (
            clk: input clock,
        ) {{
            inst bus: Bus;
            inst target: Target (
                clk: clk,
                bus: bus,
            );
            assign bus.request = bus.response;
        }}
        "#
    )
}

#[test]
fn comb_loop_modport_feedthrough_detects_feedback() {
    assert_interface_comb_loop(
        &modport_feedthrough_code("assign bus.response = bus.request;"),
        true,
    );
}

#[test]
fn comb_loop_modport_constant_output_does_not_create_feedback() {
    assert_interface_comb_loop(
        &modport_feedthrough_code("assign bus.response = '0;"),
        false,
    );
}

#[test]
fn comb_loop_modport_ff_output_breaks_feedback() {
    assert_interface_comb_loop(
        &modport_feedthrough_code(
            r#"always_ff (clk) {
                bus.response = bus.request;
            }"#,
        ),
        false,
    );
}

#[test]
#[ignore = "comb-loop migration: false positive; disjoint bits through modport feedthrough"]
fn comb_loop_modport_feedthrough_keeps_disjoint_bits_independent() {
    let code = modport_feedthrough_code("assign bus.response[0] = bus.request[0];");
    let code = code.replace(
        "assign bus.request = bus.response;",
        "assign bus.request[1] = bus.response[1];",
    );
    assert_interface_comb_loop(&code, false);
}

#[test]
fn comb_loop_modport_feedthrough_detects_same_bit_feedback() {
    let code = modport_feedthrough_code("assign bus.response[0] = bus.request[0];");
    let code = code.replace(
        "assign bus.request = bus.response;",
        "assign bus.request[0] = bus.response[0];",
    );
    assert_interface_comb_loop(&code, true);
}

fn modport_connect_code(source_assignments: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var request : logic;
            var response: logic;
            modport initiator {{
                request : output,
                response: input,
            }}
            modport target {{
                ..converse(initiator)
            }}
        }}
        module Top {{
            inst producer: Bus;
            inst consumer: Bus;
            connect producer.initiator <> consumer.target;
            {source_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_modport_connect_with_constant_source_has_no_feedback() {
    assert_interface_comb_loop(
        &modport_connect_code(
            r#"assign consumer.request = 0;
            assign producer.response = 0;"#,
        ),
        false,
    );
}

#[test]
fn comb_loop_modport_connect_detects_feedback_to_its_source() {
    assert_interface_comb_loop(
        &modport_connect_code(
            r#"assign consumer.request = consumer.response;
            assign producer.response = producer.request;"#,
        ),
        true,
    );
}
