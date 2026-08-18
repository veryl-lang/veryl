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

fn mixed_interface_code(target_assignment: &str, top_assignment: &str) -> String {
    format!(
        r#"
        interface RequestIf::<W: u32> {{
            var request: logic<W>;
            modport target_request {{
                request: input,
            }}
        }}
        interface ResponseIf::<W: u32> {{
            var response: logic<W>;
            var status  : logic<W>;
            modport target_response {{
                response: output,
                status  : input,
            }}
        }}
        interface Bus::<W: u32> {{
            mixin RequestIf::<W>;
            mixin ResponseIf::<W>;
            modport target {{
                ..same(target_request, target_response)
            }}
        }}
        module Target (
            bus: modport Bus::<2>::target,
        ) {{
            {target_assignment}
        }}
        module Top {{
            inst bus: Bus::<2>;
            inst target: Target (
                bus: bus,
            );
            {top_assignment}
        }}
        "#
    )
}

#[test]
fn comb_loop_mixed_interface_composite_modport_detects_feedback() {
    assert_interface_comb_loop(
        &mixed_interface_code(
            "assign bus.response = bus.request;",
            "assign bus.request = bus.response; assign bus.status = '0;",
        ),
        true,
    );
}

#[test]
fn comb_loop_mixed_interface_keeps_unrelated_members_out_of_feedthrough() {
    assert_interface_comb_loop(
        &mixed_interface_code(
            "assign bus.response = bus.request;",
            "assign bus.request = bus.status; assign bus.status = '0;",
        ),
        false,
    );
}

#[test]
fn comb_loop_input_default_modport_detects_observed_member_feedback() {
    assert_interface_comb_loop(
        r#"
        interface Bus {
            var value: logic;
            modport monitor {
                ..input
            }
        }
        module Observer (
            bus: modport Bus::monitor,
            o  : output logic,
        ) {
            assign o = bus.value;
        }
        module Top {
            inst bus: Bus;
            var observed: logic;
            inst observer: Observer (
                bus: bus,
                o  : observed,
            );
            assign bus.value = observed;
        }
        "#,
        true,
    );
}

fn modport_array_code(top_assignments: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var request : logic;
            var response: logic;
            modport target {{
                request : input,
                response: output,
            }}
        }}
        module Targets (
            bus: modport Bus::target[2],
        ) {{
            assign bus[0].response = bus[0].request;
            assign bus[1].response = bus[1].request;
        }}
        module Top {{
            inst bus: Bus[2];
            inst targets: Targets (
                bus: bus,
            );
            {top_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_modport_array_detects_same_element_feedback() {
    assert_interface_comb_loop(
        &modport_array_code("assign bus[0].request = bus[0].response; assign bus[1].request = 0;"),
        true,
    );
}

#[test]
fn comb_loop_modport_array_keeps_one_way_cross_element_flow_loop_free() {
    assert_interface_comb_loop(
        &modport_array_code("assign bus[0].request = bus[1].response; assign bus[1].request = 0;"),
        false,
    );
}

#[test]
fn comb_loop_modport_array_detects_cross_element_feedback() {
    assert_interface_comb_loop(
        &modport_array_code(
            "assign bus[0].request = bus[1].response; assign bus[1].request = bus[0].response;",
        ),
        true,
    );
}

fn modport_array_range_code(top_assignments: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var request : logic;
            var response: logic;
            modport target {{
                request : input,
                response: output,
            }}
        }}
        module Targets (
            bus: modport Bus::target[2],
        ) {{
            assign bus[0].response = bus[0].request;
            assign bus[1].response = bus[1].request;
        }}
        module Top {{
            inst bus: Bus[2];
            inst targets: Targets (
                bus: bus[0:1],
            );
            {top_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_modport_array_range_detects_selected_element_feedback() {
    let code = modport_array_range_code(
        "assign bus[0].request = 0; assign bus[1].request = bus[1].response;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, true);
}

#[test]
fn comb_loop_modport_array_range_keeps_selected_elements_independent() {
    let code = modport_array_range_code(
        "assign bus[0].request = bus[1].response; assign bus[1].request = 0;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, false);
}

fn modport_array_nonzero_range_code(top_assignments: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var request : logic;
            var response: logic;
            modport target {{
                request : input,
                response: output,
            }}
        }}
        module Targets (
            bus: modport Bus::target[2],
        ) {{
            assign bus[0].response = bus[0].request;
            assign bus[1].response = bus[1].request;
        }}
        module Top {{
            inst bus: Bus[4];
            inst targets: Targets (
                bus: bus[1:2],
            );
            assign bus[0].request = 0;
            assign bus[0].response = 0;
            assign bus[3].request = 0;
            assign bus[3].response = 0;
            {top_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_modport_array_nonzero_range_maps_second_selected_element() {
    let code = modport_array_nonzero_range_code(
        "assign bus[1].request = 0; assign bus[2].request = bus[2].response;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, true);
}

#[test]
fn comb_loop_modport_array_nonzero_range_keeps_selected_elements_independent() {
    let code = modport_array_nonzero_range_code(
        "assign bus[1].request = bus[2].response; assign bus[2].request = 0;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, false);
}

fn modport_array_multidimensional_range_code(top_assignments: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var request : logic;
            var response: logic;
            modport target {{
                request : input,
                response: output,
            }}
        }}
        module Targets (
            bus: modport Bus::target[2],
        ) {{
            assign bus[0].response = bus[0].request;
            assign bus[1].response = bus[1].request;
        }}
        module Top {{
            inst bus: Bus[1, 4];
            inst targets: Targets (
                bus: bus[0][1:2],
            );
            assign bus[0][0].request = 0;
            assign bus[0][0].response = 0;
            assign bus[0][3].request = 0;
            assign bus[0][3].response = 0;
            {top_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_modport_multidimensional_range_maps_second_selected_element() {
    let code = modport_array_multidimensional_range_code(
        "assign bus[0][1].request = 0; assign bus[0][2].request = bus[0][2].response;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, true);
}

#[test]
fn comb_loop_modport_multidimensional_range_keeps_selected_elements_independent() {
    let code = modport_array_multidimensional_range_code(
        "assign bus[0][1].request = bus[0][2].response; assign bus[0][2].request = 0;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, false);
}

fn module_input_range_code(top_assignments: &str) -> String {
    format!(
        r#"
        module Pass (
            source: input  logic [2],
            sink  : output logic [2],
        ) {{
            assign sink[0] = source[0];
            assign sink[1] = source[1];
        }}
        module Top {{
            var source: logic [4];
            var sink  : logic [2];
            inst pass: Pass (
                source: source[1:2],
                sink  : sink,
            );
            assign source[0] = 0;
            assign source[3] = 0;
            {top_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_module_input_nonzero_range_maps_every_selected_element() {
    let code = module_input_range_code("assign source[1] = 0; assign source[2] = sink[1];");
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, true);
}

#[test]
fn comb_loop_module_input_nonzero_range_keeps_elements_independent() {
    let code = module_input_range_code("assign source[1] = sink[1]; assign source[2] = 0;");
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_comb_loop(&code, false);
}

#[test]
fn comb_loop_module_input_multidimensional_range_preserves_flat_position() {
    let code = r#"
        module Pass (
            source: input  logic [2],
            sink  : output logic [2],
        ) {
            assign sink[0] = source[0];
            assign sink[1] = source[1];
        }
        module Top {
            var source: logic [1, 4];
            var sink  : logic [2];
            inst pass: Pass (
                source: source[0][1:2],
                sink  : sink,
            );
            assign source[0][0] = 0;
            assign source[0][1] = 0;
            assign source[0][2] = sink[1];
            assign source[0][3] = 0;
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_interface_comb_loop(code, true);
}

#[test]
fn comb_loop_module_input_large_range_stays_sparse() {
    let code = r#"
        module Pass (
            source: input  logic [1000000],
            sink  : output logic,
        ) {
            assign sink = source[999999];
        }
        module Top {
            var source: logic [1000000];
            var sink  : logic;
            inst pass: Pass (
                source: source[0:999999],
                sink  : sink,
            );
            assign source[999999] = sink;
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_interface_comb_loop(code, true);
}

fn specialized_interface_code(width: u32) -> String {
    format!(
        r#"
        interface Bus::<W: u32> {{
            var request : logic<W>;
            var response: logic<W>;
            modport target {{
                request : input,
                response: output,
            }}
        }}
        module Target::<W: u32> (
            bus: modport Bus::<W>::target,
        ) {{
            if W == 1 :g_enabled {{
                assign bus.response = bus.request;
            }} else {{
                assign bus.response = '0;
            }}
        }}
        module Top {{
            inst bus: Bus::<{width}>;
            inst target: Target::<{width}> (
                bus: bus,
            );
            assign bus.request = bus.response;
        }}
        "#
    )
}

#[test]
fn comb_loop_generic_interface_enabled_specialization_retains_feedthrough() {
    assert_interface_comb_loop(&specialized_interface_code(1), true);
}

#[test]
fn comb_loop_generic_interface_disabled_specialization_has_no_feedthrough() {
    assert_interface_comb_loop(&specialized_interface_code(2), false);
}

fn formal_modport_connect_code(source_assignments: &str) -> String {
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
        module Bridge (
            producer: modport Bus::initiator,
            consumer: modport Bus::target,
        ) {{
            connect producer <> consumer;
        }}
        module Top {{
            inst producer: Bus;
            inst consumer: Bus;
            inst bridge: Bridge (
                producer: producer,
                consumer: consumer,
            );
            {source_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_formal_modport_connect_detects_feedback() {
    assert_interface_comb_loop(
        &formal_modport_connect_code(
            r#"assign consumer.request = consumer.response;
            assign producer.response = producer.request;"#,
        ),
        true,
    );
}

#[test]
fn comb_loop_formal_modport_connect_with_constant_sources_has_no_feedback() {
    assert_interface_comb_loop(
        &formal_modport_connect_code(
            r#"assign consumer.request = 0;
            assign producer.response = 0;"#,
        ),
        false,
    );
}

fn procedural_modport_connect_code(overrides: &str) -> String {
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
            always_comb {{
                producer.initiator <> consumer.target;
                {overrides}
            }}
            assign consumer.request = consumer.response;
            assign producer.response = producer.request;
        }}
        "#
    )
}

#[test]
fn comb_loop_procedural_modport_connect_detects_feedback() {
    assert_interface_comb_loop(&procedural_modport_connect_code(""), true);
}

#[test]
fn comb_loop_procedural_modport_connect_overrides_kill_feedback() {
    assert_interface_comb_loop(
        &procedural_modport_connect_code(
            r#"producer.request = 0;
            consumer.response = 0;"#,
        ),
        false,
    );
}

fn partial_modport_connect_code(source_assignments: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var request : logic;
            var response: logic;
            modport initiator {{
                request : output,
                response: input,
            }}
            modport target_request {{
                request: input,
            }}
        }}
        module RequestBridge (
            producer: modport Bus::initiator,
            consumer: modport Bus::target_request,
        ) {{
            connect producer <> consumer;
        }}
        module Top {{
            inst producer: Bus;
            inst consumer: Bus;
            inst bridge: RequestBridge (
                producer: producer,
                consumer: consumer,
            );
            {source_assignments}
        }}
        "#
    )
}

#[test]
fn comb_loop_partial_modport_connect_detects_shared_member_feedback() {
    assert_interface_comb_loop(
        &partial_modport_connect_code(
            r#"assign consumer.request = producer.response;
            assign producer.response = producer.request;
            assign consumer.response = 0;"#,
        ),
        true,
    );
}

#[test]
fn comb_loop_partial_modport_connect_does_not_connect_absent_member() {
    assert_interface_comb_loop(
        &partial_modport_connect_code(
            r#"assign consumer.request = 0;
            assign consumer.response = producer.request;
            assign producer.response = consumer.response;"#,
        ),
        false,
    );
}
