use super::*;

#[track_caller]
fn assert_select_errors(code: &str, selections: &[&str]) {
    let errors = analyze(code);
    let locations: Vec<_> = errors
        .iter()
        .filter_map(|e| match e {
            AnalyzerError::NonConstantInterfaceInstanceSelect { error_location, .. } => {
                let start = error_location.offset();
                Some(&code[start..start + error_location.len()])
            }
            _ => None,
        })
        .collect();
    assert_eq!(locations, selections, "{errors:#?}");
}

#[test]
fn interface_instance_select_read_write_and_call() {
    for statement in [
        "assign o = bus[i].get();",
        "assign o = bus[i].value;",
        "always_comb { bus[i].value = i; o = 0; }",
        "assign bus[i].value = i; assign o = 0;",
    ] {
        let code = format!(
            r#"
            interface Bus {{
                var value: logic;
                function get() -> logic {{ return value; }}
            }}
            module Top(i: input logic, o: output logic) {{
                inst bus: Bus[2];
                {statement}
            }}
            "#
        );
        assert_select_errors(&code, &["i"]);
    }
}

#[test]
fn interface_instance_select_initial_write() {
    let code = r#"
        interface Bus { var data: logic<8>; }
        module Child (clk: input clock, bus: interface) {
            always_ff { bus.data = bus.data + 8'd1; }
        }
        module Top (clk: input clock, index: input logic, a: output logic<8>, c: output logic<8>) {
            #[allow(initial_assign)]
            inst bus: Bus [3];
            inst u: Child (clk, bus: bus[2]);
            initial { bus[2].data = '0; bus[index].data = 8'h3c; }
            assign a = bus[0].data; assign c = bus[2].data;
        }
    "#;
    assert_select_errors(code, &["index"]);
}

#[test]
fn interface_instance_select_modport() {
    for statement in [
        "assign o = bus[i].get();",
        "assign o = bus[i].value;",
        "always_comb { bus[i].value = i; o = 0; }",
    ] {
        let code = format!(
            r#"
            interface Bus {{
                var value: logic;
                function get() -> logic {{ return value; }}
                modport mp {{ value: inout, get: import, }}
            }}
            module Top(i: input logic, o: output logic, bus: modport Bus::mp[2]) {{
                {statement}
            }}
            "#
        );
        assert_select_errors(&code, &["i"]);
    }
}

#[test]
fn interface_instance_select_procedural_loop() {
    for expression in ["j", "j + 0", "if 1 ? 0 : j"] {
        let code = format!(
            r#"
            interface Bus {{
                var value: logic;
                function get() -> logic {{ return value; }}
            }}
            module Top(o: output logic[2]) {{
                inst bus: Bus[2];
                assign bus[0].value = 0;
                assign bus[1].value = 1;
                always_comb {{
                    for j in 0..2 {{
                        o[j] = bus[{expression}].get();
                    }}
                }}
            }}
            "#
        );
        assert_select_errors(&code, &[expression]);
    }
}

#[test]
fn interface_instance_select_constants_and_dynamic_data() {
    for index in [
        "0",
        "N",
        "p::N",
        "$sv::external_pkg::INDEX",
        "p::EXT",
        "p::EXT + 0",
        "($sv::external_pkg::index(0))",
        "p::identity(0)",
        "p::E::Zero as u32",
        "$bits(i) - 1",
    ] {
        let code = format!(
            r#"
            package p {{
                const N: u32 = 0;
                const EXT: u32 = $sv::external_pkg::INDEX;
                function identity(x: input u32) -> u32 {{ return x; }}
                enum E {{ Zero = 0, One = 1, }}
            }}
            interface Bus {{
                var data: logic<8>[2];
                modport mp {{ data: input, }}
            }}
            module Top(i: input logic, bus: modport Bus::mp[2], o: output logic) {{
                const N: u32 = 0;
                assign o = bus[{index}].data[i][i];
            }}
            "#
        );
        assert!(analyze(&code).is_empty(), "{index}");
    }
}

#[test]
fn interface_instance_select_generate_loop() {
    let code = r#"
        interface Bus {
            var value: logic;
            function get() -> logic { return value; }
        }
        module Top(o: output logic[2]) {
            inst bus: Bus[2];
            for j in 0..2 :g {
                assign bus[j].value = 0;
                assign o[j] = bus[j].get();
            }
        }
    "#;
    assert!(analyze(code).is_empty());
}

#[test]
fn interface_instance_select_multidimensional() {
    let code = r#"
        interface Bus {
            var value: logic;
            modport mp { value: input, }
        }
        module Top(i: input logic, bus: modport Bus::mp[2, 2], o: output logic) {
            assign o = bus[0][i].value;
        }
    "#;
    assert_select_errors(code, &["i"]);
    assert_select_errors(&code.replace("bus[0][i]", "bus[i][0]"), &["i"]);
    assert_select_errors(&code.replace("bus[0][i]", "bus[i][i]"), &["i", "i"]);
}

#[test]
fn interface_instance_select_nested() {
    for statement in [
        "assign o = outer[0].inner[i].value;",
        "assign o = outer[i].inner[0].value;",
        "always_comb { outer[0].inner[i].value = i; o = 0; }",
    ] {
        let code = format!(
            r#"
            interface Bus {{
                var value: logic;
                function get() -> logic {{ return value; }}
            }}
            interface Outer {{ inst inner: Bus[2]; }}
            module Top(i: input logic, o: output logic) {{
                inst outer: Outer[2];
                {statement}
            }}
            "#
        );
        assert_select_errors(&code, &["i"]);
    }
}

#[test]
fn interface_instance_select_runtime_expressions() {
    for index in [
        "i + $sv::external_pkg::INDEX",
        "if 1 ? 0 : i",
        "($sv::external_pkg::index(i))",
    ] {
        let code = format!(
            r#"
            interface Bus {{
                var value: logic;
                modport mp {{ value: input, }}
            }}
            module Top(i: input u32, bus: modport Bus::mp[2], o: output logic) {{
                assign o = bus[{index}].value;
            }}
            "#
        );
        assert_select_errors(&code, &[index]);
    }
}

#[test]
fn interface_instance_select_type_queries_are_constants() {
    let code = r#"
        interface Bus {
            var value: logic;
            modport mp { value: input, }
        }
        module Top(bus: modport Bus::mp[2], o: output logic[2]) {
            always_comb {
                for j in 0..2 {
                    o[j] = bus[$bits(j) - 32].value;
                }
            }
        }
    "#;
    assert!(analyze(code).is_empty());
}

#[test]
fn interface_instance_select_connection() {
    for connection in ["bus[i]", "bus[i].mp"] {
        let code = format!(
            r#"
            interface Bus {{
                var value: logic;
                modport mp {{ value: output, }}
            }}
            module Writer(bus: modport Bus::mp) {{
                assign bus.value = 0;
            }}
            module Top(i: input logic) {{
                inst bus: Bus[2];
                inst writer: Writer(bus: {connection});
            }}
            "#
        );
        assert_select_errors(&code, &["i"]);
    }
}

#[test]
fn interface_instance_select_generic_constants() {
    let code = r#"
        interface Bus::<W: u32> {
            var data: logic<W>;
            modport mp { data: input, }
        }
        module Top::<N: u32>(i: input logic, bus: modport Bus::<8>::mp[2], o: output logic) {
            assign o = bus[N].data[i];
        }
    "#;
    assert!(analyze(code).is_empty());
}

#[test]
fn interface_instance_select_runtime_loop_and_constant_lookup() {
    for upper_bound in ["2", "n"] {
        for index in ["j", "p::INDEX[j]", "p::identity(j)"] {
            let code = format!(
                r#"
                package p {{
                    const INDEX: u32[2] = '{{0, 1}};
                    function identity(x: input u32) -> u32 {{ return x; }}
                }}
                interface Bus {{
                    var value: logic;
                    modport mp {{ value: output, }}
                }}
                module Top(n: input u32, bus: modport Bus::mp[2]) {{
                    always_comb {{
                        for j in 0..{upper_bound} {{
                            bus[({index})].value = 0;
                        }}
                    }}
                }}
                "#
            );
            assert_select_errors(&code, &[&format!("({index})")]);
        }
    }
}
