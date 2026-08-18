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
