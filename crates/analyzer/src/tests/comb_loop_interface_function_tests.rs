use super::*;

fn assert_interface_function_comb_loop(code: &str, expected: bool) {
    let errors = analyze(code);
    let detected = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert_eq!(detected, expected, "{errors:#?}");
}

#[test]
fn comb_loop_interface_function_read_detects_member_feedback() {
    assert_interface_function_comb_loop(
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

fn imported_interface_read_code(top_assignments: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var observed : logic;
            var unrelated: logic;
            function read_observed () -> logic {{
                return observed;
            }}
            modport monitor {{
                read_observed: import,
            }}
        }}
        module Observer (
            bus: modport Bus::monitor,
            o  : output logic,
        ) {{
            assign o = bus.read_observed();
        }}
        module Top {{
            inst bus: Bus;
            var observed: logic;
            inst observer: Observer (
                bus: bus,
                o  : observed,
            );
            {top_assignments}
        }}
        "#
    )
}

#[test]
#[ignore = "comb-loop migration: false negative; imported interface function member read"]
fn comb_loop_imported_interface_read_detects_member_feedback() {
    assert_interface_function_comb_loop(
        &imported_interface_read_code("assign bus.observed = observed; assign bus.unrelated = 0;"),
        true,
    );
}

#[test]
fn comb_loop_imported_interface_read_keeps_unrelated_member_independent() {
    assert_interface_function_comb_loop(
        &imported_interface_read_code("assign bus.observed = 0; assign bus.unrelated = observed;"),
        false,
    );
}

fn imported_interface_write_code(writer_input: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var written  : logic;
            var unrelated: logic;
            function write_written (value: input logic) {{
                written = value;
            }}
            modport target {{
                write_written: import,
            }}
        }}
        module Writer (
            i  : input logic,
            bus: modport Bus::target,
        ) {{
            always_comb {{
                bus.write_written(i);
            }}
        }}
        module Top {{
            inst bus: Bus;
            inst writer: Writer (
                i  : {writer_input},
                bus: bus,
            );
            assign bus.unrelated = 0;
        }}
        "#
    )
}

#[test]
#[ignore = "comb-loop migration: false negative; imported interface function member write"]
fn comb_loop_imported_interface_write_detects_member_feedback() {
    assert_interface_function_comb_loop(&imported_interface_write_code("bus.written"), true);
}

#[test]
fn comb_loop_imported_interface_write_keeps_unrelated_member_independent() {
    assert_interface_function_comb_loop(&imported_interface_write_code("bus.unrelated"), false);
}

const EXTERNAL_INTERFACE_API: &str = r#"
    interface Bus {
        var value: logic;
        function get () -> logic {
            return value;
        }
        function put (next: input logic) {
            value = next;
        }
        modport reader {
            get: import,
        }
        modport writer {
            put: import,
        }
    }
"#;

fn connected_interface_function_code(top_declarations: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var forward : logic;
            var backward: logic;
            function get_forward () -> logic {{
                return forward;
            }}
            function put_backward (value: input logic) {{
                backward = value;
            }}
            modport source {{
                forward     : output,
                backward    : input,
                get_forward : import,
                put_backward: import,
            }}
            modport sink {{
                get_forward : import,
                put_backward: import,
                ..converse(source)
            }}
        }}
        module Top {{
            inst source: Bus;
            inst sink  : Bus;
            connect source.source <> sink.sink;
            {top_declarations}
        }}
        "#
    )
}

#[test]
fn comb_loop_connect_detects_interface_function_member_read_feedback() {
    assert_interface_function_comb_loop(
        &connected_interface_function_code(
            r#"assign sink.forward = source.get_forward();
            assign source.backward = 0;"#,
        ),
        true,
    );
}

#[test]
fn comb_loop_connect_keeps_interface_function_member_read_one_way() {
    assert_interface_function_comb_loop(
        &connected_interface_function_code(
            r#"var observed: logic;
            assign sink.forward = source.backward;
            assign source.backward = 0;
            assign observed = source.get_forward();"#,
        ),
        false,
    );
}

#[test]
fn comb_loop_connect_detects_interface_function_member_write_feedback() {
    assert_interface_function_comb_loop(
        &connected_interface_function_code(
            r#"always_comb {
                source.put_backward(sink.forward);
            }
            assign sink.forward = sink.backward;"#,
        ),
        true,
    );
}

#[test]
fn comb_loop_connect_keeps_interface_function_member_write_one_way() {
    assert_interface_function_comb_loop(
        &connected_interface_function_code(
            r#"always_comb {
                source.put_backward(sink.forward);
            }
            assign sink.forward = 0;"#,
        ),
        false,
    );
}

#[test]
fn comb_loop_external_interface_get_to_put_detects_same_receiver_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst bus: Bus;
            always_comb {{
                bus.put(bus.get());
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, true);
}

#[test]
#[ignore = "comb-loop follow-up: false negative; disjoint interface member writes require positional procedure SSA"]
fn comb_loop_external_interface_disjoint_followup_write_preserves_same_bit_feedback() {
    assert_interface_function_comb_loop(
        r#"
        interface Bus {
            var value: logic[2];
            function get0 () -> logic { return value[0]; }
            function put0 (next: input logic) { value[0] = next; }
            function put1 (next: input logic) { value[1] = next; }
        }
        module Top {
            inst bus: Bus;
            always_comb {
                bus.put0(bus.get0());
                bus.put1(0);
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_external_interface_get_to_put_detects_cross_receiver_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst first : Bus;
            inst second: Bus;
            always_comb {{
                first.put(second.get());
                second.put(first.get());
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_external_interface_get_to_put_keeps_one_way_receivers_loop_free() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst source     : Bus;
            inst destination: Bus;
            always_comb {{
                source.put(0);
                destination.put(source.get());
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_external_interface_read_preserves_receiver_identity() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst source     : Bus;
            inst destination: Bus;
            assign source.value      = 0;
            assign destination.value = source.get();
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_interface_function_array_detects_same_element_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst bus: Bus[2];
            always_comb {{
                bus[0].put(bus[0].get());
                bus[1].put(0);
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_interface_function_array_keeps_one_way_elements_loop_free() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst bus: Bus[2];
            always_comb {{
                bus[0].put(0);
                bus[1].put(bus[0].get());
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_interface_function_array_detects_cross_element_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst bus: Bus[2];
            always_comb {{
                bus[0].put(bus[1].get());
                bus[1].put(bus[0].get());
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_large_interface_function_array_specializes_only_the_called_receiver() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst bus: Bus[1000000];
            always_comb {{
                bus[999999].put(bus[999999].get());
            }}
        }}
        "#
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn interface_array_const_function_reads_the_selected_receiver_return() {
    let code = r#"
        interface Bus {
            function one () -> u32 {
                return 1;
            }
        }
        module Top (o: output logic) {
            inst bus: Bus[2];
            const SECOND: u32 = bus[1].one();
            if SECOND == 1 :g_ok {
                assign o = 0;
            } else {
                assign o = o;
            }
        }
    "#;
    assert_interface_function_comb_loop(code, false);
}

#[test]
fn interface_array_const_function_returns_remain_receiver_independent() {
    let code = r#"
        interface Bus {
            function copy (value: input u32) -> u32 {
                return value;
            }
        }
        module Top (o: output logic) {
            inst bus: Bus[2];
            const FIRST : u32 = bus[0].copy(3);
            const SECOND: u32 = bus[1].copy(5);
            if FIRST == 3 && SECOND == 5 :g_ok {
                assign o = 0;
            } else {
                assign o = o;
            }
        }
    "#;
    assert_interface_function_comb_loop(code, false);
}

fn interface_array_member_function_code(body: &str, statements: &str) -> String {
    format!(
        r#"
        interface Bus {{
            var data: logic [2];
            {body}
        }}
        module Top (o: output logic) {{
            inst bus: Bus [1];
            {statements}
        }}
        "#
    )
}

#[test]
fn comb_loop_interface_array_function_read_keeps_member_elements_independent() {
    let code = interface_array_member_function_code(
        "function read_second () -> logic { return data[1]; }",
        r#"
        assign bus[0].data[0] = o;
        assign bus[0].data[1] = 0;
        assign o = bus[0].read_second();
        "#,
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_interface_array_function_write_preserves_other_member_feedback() {
    let code = interface_array_member_function_code(
        "function clear_second () { data[1] = 0; }",
        r#"
        always_comb {
            bus[0].data[0] = o;
            bus[0].clear_second();
        }
        assign o = bus[0].data[0];
        "#,
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_interface_array_function_keeps_scalar_formal_receiver_independent() {
    let code = interface_array_member_function_code(
        "function gated_second (enable: input logic) -> logic { return data[1] & enable; }",
        r#"
        assign bus[0].data[0] = o;
        assign bus[0].data[1] = 0;
        assign o = bus[0].gated_second(1);
        "#,
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_interface_array_nested_function_preserves_receiver() {
    let code = r#"
        interface Bus {
            var data: logic [2];
            function read_second () -> logic {
                return data[1];
            }
            function forward_second () -> logic {
                return read_second();
            }
        }
        module Top (o: output logic) {
            inst bus: Bus [2];
            assign bus[0].data[0] = 0;
            assign bus[0].data[1] = 0;
            assign bus[1].data[0] = 0;
            assign bus[1].data[1] = o;
            assign o = bus[1].forward_second();
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_interface_function_comb_loop(code, true);
}

#[test]
fn comb_loop_interface_array_nested_output_actual_clears_selected_member() {
    let code = r#"
        interface Bus {
            var data: logic [2];
            function clear (value: output logic) {
                value = 0;
            }
            function clear_second () {
                clear(data[1]);
            }
        }
        module Top (o: output logic) {
            inst bus: Bus [2];
            always_comb {
                bus[0].data[0] = 0;
                bus[0].data[1] = 0;
                bus[1].data[0] = 0;
                bus[1].data[1] = o;
                bus[1].clear_second();
            }
            assign o = bus[1].data[1];
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_interface_function_comb_loop(code, false);
}

#[test]
fn comb_loop_external_interface_function_control_detects_member_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst bus: Bus;
            always_comb {{
                if bus.get() {{
                    bus.value = 0;
                }} else {{
                    bus.value = 1;
                }}
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_system_function_argument_preserves_interface_receiver_identity() {
    let interface = r#"
        interface Bus {
            var source     : logic;
            var destination: logic;
            function transfer () -> logic {
                destination = source;
                return destination;
            }
        }
    "#;
    let one_way = format!(
        r#"
        {interface}
        module Top {{
            inst bus: Bus [2];
            always_comb {{
                $display("value=%d", bus[0].transfer());
                bus[1].source = bus[1].destination;
            }}
            assign bus[0].source      = 0;
            assign bus[1].destination = 0;
        }}
        "#
    );
    assert!(comb_loop_analysis_is_complete(&one_way));
    assert_interface_function_comb_loop(&one_way, false);

    let feedback = format!(
        r#"
        {interface}
        module Top {{
            inst bus: Bus [2];
            always_comb {{
                $display("value=%d", bus[0].transfer());
            }}
            assign bus[0].source      = bus[0].destination;
            assign bus[1].source      = 0;
            assign bus[1].destination = 0;
        }}
        "#
    );
    assert!(comb_loop_analysis_is_complete(&feedback));
    assert_interface_function_comb_loop(&feedback, true);
}

#[test]
fn comb_loop_external_interface_function_instance_actual_detects_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Pass (
            i: input  logic,
            o: output logic,
        ) {{
            assign o = i;
        }}
        module Top {{
            inst bus: Bus;
            inst pass: Pass (
                i: bus.get(),
                o: bus.value,
            );
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, true);
}

fn nested_interface_receiver_code(assignments: &str) -> String {
    format!(
        r#"
        interface Inner {{
            var value: logic;
            function get () -> logic {{
                return value;
            }}
        }}
        interface Outer {{
            inst inner: Inner[4];
            function read_last () -> logic {{
                return inner[3].get();
            }}
            function write_last (next: input logic) {{
                inner[3].value = next;
            }}
            function write_first (next: input logic) {{
                inner[0].value = next;
            }}
        }}
        module Top (o: output logic) {{
            inst outer: Outer[2];
            {assignments}
            assign o = outer[0].read_last();
        }}
        "#
    )
}

#[test]
fn comb_loop_nested_interface_method_composes_outer_and_inner_receivers() {
    let code = nested_interface_receiver_code(
        r#"
        always_comb {
            outer[0].write_last(o);
            outer[0].write_first(0);
            outer[1].write_last(0);
        }
        "#,
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_nested_interface_method_keeps_outer_and_inner_receivers_disjoint() {
    let code = nested_interface_receiver_code(
        r#"
        always_comb {
            outer[0].write_last(0);
            outer[0].write_first(o);
            outer[1].write_last(o);
        }
        "#,
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, false);
}

fn nested_scalar_interface_receiver_code(assignments: &str) -> String {
    format!(
        r#"
        interface Inner {{
            var value: logic;
            function get () -> logic {{
                return value;
            }}
        }}
        interface Outer {{
            inst inner: Inner;
            function read () -> logic {{
                return inner.get();
            }}
            function write (next: input logic) {{
                inner.value = next;
            }}
        }}
        module Top (o: output logic) {{
            inst outer: Outer[2];
            {assignments}
            assign o = outer[0].read();
        }}
        "#
    )
}

#[test]
fn comb_loop_nested_scalar_interface_method_inherits_outer_receiver() {
    let code = nested_scalar_interface_receiver_code(
        r#"
        always_comb {
            outer[0].write(o);
            outer[1].write(0);
        }
        "#,
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_nested_scalar_interface_method_keeps_outer_receivers_disjoint() {
    let code = nested_scalar_interface_receiver_code(
        r#"
        always_comb {
            outer[0].write(0);
            outer[1].write(o);
        }
        "#,
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, false);
}

fn nested_dynamic_interface_receiver_code(select_assignments: &str) -> String {
    format!(
        r#"
        interface Inner {{
            var value: logic;
            function get () -> logic {{
                return value;
            }}
        }}
        interface Outer {{
            var select: u32;
            inst inner: Inner[2];
            function read () -> logic {{
                return inner[select].get();
            }}
            function write_first (next: input logic) {{
                inner[0].value = next;
            }}
            function write_last (next: input logic) {{
                inner[1].value = next;
            }}
        }}
        module Top (o: output logic) {{
            inst outer: Outer[2];
            always_comb {{
                outer[0].write_first(0);
                outer[0].write_last(1);
                outer[1].write_first(0);
                outer[1].write_last(1);
            }}
            {select_assignments}
            assign o = outer[0].read();
        }}
        "#
    )
}

#[test]
fn comb_loop_nested_dynamic_receiver_uses_the_selected_outer_receiver() {
    let code = nested_dynamic_interface_receiver_code(
        "assign outer[0].select = o as u32; assign outer[1].select = 0;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_nested_dynamic_receiver_does_not_read_another_outer_receiver() {
    let code = nested_dynamic_interface_receiver_code(
        "assign outer[0].select = 0; assign outer[1].select = o as u32;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, false);
}

fn interface_method_global_helper_code(assignments: &str) -> String {
    format!(
        r#"
        package Helpers {{
            function pass (value: input logic) -> logic {{
                return value;
            }}
        }}
        interface Outer {{
            var value: logic;
            function read () -> logic {{
                return Helpers::pass(value);
            }}
        }}
        module Top (o: output logic) {{
            inst outer: Outer[2];
            {assignments}
            assign o = outer[0].read();
        }}
        "#
    )
}

#[test]
fn comb_loop_interface_method_global_helper_preserves_owned_member_feedback() {
    let code = interface_method_global_helper_code(
        "assign outer[0].value = o; assign outer[1].value = 0;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_interface_method_does_not_prefix_global_helper_formals() {
    let code = interface_method_global_helper_code(
        "assign outer[0].value = 0; assign outer[1].value = o;",
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_dynamic_interface_receiver_conservatively_detects_possible_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top (
            index: input  u32,
            o    : output logic,
        ) {{
            inst bus: Bus[2];
            assign bus[0].value = 0;
            assign bus[1].value = o;
            assign o = bus[index].get();
        }}
        "#
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_dynamic_interface_receiver_keeps_one_way_read_loop_free() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top (
            index: input  u32,
            o    : output logic,
        ) {{
            inst bus: Bus[2];
            assign bus[0].value = 0;
            assign bus[1].value = 1;
            assign o = bus[index].get();
        }}
        "#
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_dynamic_interface_receiver_preserves_selector_dependency() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top (o: output logic) {{
            var index: u32;
            inst bus: Bus[2];
            assign bus[0].value = 0;
            assign bus[1].value = 1;
            assign index = o;
            assign o = bus[index].get();
        }}
        "#
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_second_dynamic_receiver_keeps_its_selector_dependency() {
    let code = r#"
        interface Bus {
            var value: logic;
            function get () -> logic {
                return value;
            }
        }
        module Top (
            external: input  u32,
            o       : output logic,
        ) {
            var feedback: u32;
            inst bus: Bus[2];
            assign bus[0].value = 0;
            assign bus[1].value = 1;
            assign feedback = o as u32;
            assign o = bus[external].get() | bus[feedback].get();
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_interface_function_comb_loop(code, true);
}

#[test]
fn comb_loop_dynamic_receiver_summary_does_not_import_another_call_sites_selector() {
    let code = r#"
        interface Bus {
            var value: logic;
            function get () -> logic {
                return value;
            }
        }
        module Top (
            external: input  u32,
            o       : output logic,
        ) {
            var feedback: u32;
            var ignored: logic;
            inst bus: Bus[2];
            assign bus[0].value = 0;
            assign bus[1].value = 1;
            assign feedback = o as u32;
            assign ignored = bus[feedback].get();
            assign o = bus[external].get();
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_interface_function_comb_loop(code, false);
}

#[test]
fn comb_loop_large_dynamic_interface_receiver_stays_sparse() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top (
            index: input  u32,
            o    : output logic,
        ) {{
            inst bus: Bus[1000000];
            assign bus[999999].value = o;
            assign o = bus[index].get();
        }}
        "#
    );
    assert!(comb_loop_analysis_is_complete(&code));
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_modport_array_function_formal_is_rejected_before_comb_loop_analysis() {
    let code = r#"
        interface Bus {
            var value: logic;
            function get () -> logic {
                return value;
            }
            modport reader {
                get: import,
            }
        }
        module Top {
            function read_last (
                source: modport Bus::reader [4],
            ) -> logic {
                return source[3].get();
            }
        }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::UnexpandableModport { .. })),
        "modport-array function formals must be rejected before comb-loop analysis: {errors:#?}"
    );
}

#[test]
#[ignore = "comb-loop migration: false negative; modport formal interface function transfer"]
fn comb_loop_modport_formal_get_to_put_detects_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Copy (
            source     : modport Bus::reader,
            destination: modport Bus::writer,
        ) {{
            always_comb {{
                destination.put(source.get());
            }}
        }}
        module Top {{
            inst source     : Bus;
            inst destination: Bus;
            inst copy: Copy (
                source     : source,
                destination: destination,
            );
            assign source.value = destination.value;
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, true);
}

#[test]
fn comb_loop_interface_function_output_detects_captured_member_feedback() {
    assert_interface_function_comb_loop(
        r#"
        interface Bus {
            var value: logic;
            function sample (copy: output logic) -> logic {
                copy = value;
                return value;
            }
        }
        module Top {
            inst bus: Bus;
            var copy      : logic;
            var discarded : logic;
            always_comb {
                discarded = bus.sample(copy);
                bus.value = copy;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_later_member_write_kills_external_interface_call_feedback() {
    let code = format!(
        r#"
        {EXTERNAL_INTERFACE_API}
        module Top {{
            inst bus: Bus;
            always_comb {{
                bus.put(bus.get());
                bus.value = 0;
            }}
        }}
        "#
    );
    assert_interface_function_comb_loop(&code, false);
}

#[test]
fn comb_loop_uncalled_interface_function_adds_no_member_dependency() {
    assert_interface_function_comb_loop(
        r#"
        interface Bus {
            var source     : logic;
            var destination: logic;
            function transfer () {
                destination = source;
            }
        }
        module Top {
            inst bus: Bus;
            assign bus.source = bus.destination;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_mixed_interface_function_detects_inherited_member_feedback() {
    assert_interface_function_comb_loop(
        r#"
        interface BaseBus {
            var value: logic;
            function get () -> logic {
                return value;
            }
        }
        interface Bus {
            mixin BaseBus;
        }
        module Top {
            inst bus: Bus;
            assign bus.value = bus.get();
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_nested_interface_function_detects_transitive_member_read() {
    assert_interface_function_comb_loop(
        r#"
        interface Bus {
            var value: logic;
            function raw () -> logic {
                return value;
            }
            function wrapped () -> logic {
                return raw();
            }
        }
        module Top {
            inst bus: Bus;
            assign bus.value = bus.wrapped();
        }
        "#,
        true,
    );
}

fn specialized_interface_function_code(enabled: bool) -> String {
    format!(
        r#"
        interface Bus::<ENABLED: bbool> {{
            var value: logic;
            function get () -> logic {{
                if ENABLED {{
                    return value;
                }} else {{
                    return 0;
                }}
            }}
        }}
        module Top {{
            inst bus: Bus::<{enabled}>;
            assign bus.value = bus.get();
        }}
        "#
    )
}

#[test]
fn comb_loop_enabled_interface_function_specialization_reads_member() {
    assert_interface_function_comb_loop(&specialized_interface_function_code(true), true);
}

#[test]
fn comb_loop_disabled_interface_function_specialization_ignores_member() {
    assert_interface_function_comb_loop(&specialized_interface_function_code(false), false);
}

#[test]
fn comb_loop_false_positive_imported_interface_function_widens_output_region() {
    assert_interface_function_comb_loop(
        r#"
        interface Bus {
            var a: logic;
            var b: logic;
            function choose (sel: input logic) -> logic<2> {
                return if sel ? {a, 1'b0} : {1'b0, b};
            }
            modport monitor {
                choose: import,
            }
        }
        module Observer (
            bus: modport Bus::monitor,
            sel: input  logic,
            o  : output logic<2>,
        ) {
            assign o = bus.choose(sel);
        }
        module Top (
            sel: input logic,
        ) {
            inst bus: Bus;
            var passed: logic<2>;
            inst observer: Observer (
                bus: bus,
                sel: sel,
                o  : passed,
            );
            assign bus.b = passed[1];
            assign bus.a = passed[0];
        }
        "#,
        false,
    );
}

#[test]
#[ignore = "requires imported modport function effects"]
fn comb_loop_imported_interface_function_retains_matching_output_region_feedback() {
    assert_interface_function_comb_loop(
        r#"
        interface Bus {
            var a: logic;
            var b: logic;
            function choose (sel: input logic) -> logic<2> {
                return if sel ? {a, 1'b0} : {1'b0, b};
            }
            modport monitor {
                choose: import,
            }
        }
        module Observer (
            bus: modport Bus::monitor,
            sel: input  logic,
            o  : output logic<2>,
        ) {
            assign o = bus.choose(sel);
        }
        module Top (
            sel: input logic,
        ) {
            inst bus: Bus;
            var passed: logic<2>;
            inst observer: Observer (
                bus: bus,
                sel: sel,
                o  : passed,
            );
            assign bus.a = passed[1];
            assign bus.b = 1'b0;
        }
        "#,
        true,
    );
}
