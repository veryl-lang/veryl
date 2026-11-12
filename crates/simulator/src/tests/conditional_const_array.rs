use super::*;

#[test]
fn conditional_const_array_keeps_selected_elements() {
    for select in [0, 1] {
        let code = format!(
            r#"
        package Tables::<S: u32> {{
            const LITERAL: logic<8> [2] = if S == 1 ? '{{8'h0f, 8'hf0}} : '{{8'h33, 8'hcc}};
            const NESTED: logic<8> [2] = if S == 1 ?
                (if S == 1 ? '{{8'h0f, 8'hf0}} : '{{8'h33, 8'hcc}}) : '{{8'h33, 8'hcc}};
            const MATRIX: logic<8> [2, 2] = if S == 1 ?
                '{{'{{8'h0f, 8'hf0}}, '{{8'h33, 8'hcc}}}} : '{{'{{8'h33, 8'hcc}}, '{{8'h0f, 8'hf0}}}};
            function get(index: input u32) -> logic<8> {{ return LITERAL[index]; }}
        }}
        module Top (index: input logic, literal: output logic<8>,
                    nested: output logic<8>, matrix: output logic<8>) {{
            assign literal = Tables::<{select}>::get(index as u32);
            assign nested = Tables::<{select}>::NESTED[index];
            assign matrix = Tables::<{select}>::MATRIX[1][index];
        }}
        "#
        );
        let expected = if select == 1 {
            [0x0f, 0xf0]
        } else {
            [0x33, 0xcc]
        };
        let other = if select == 1 {
            [0x33, 0xcc]
        } else {
            [0x0f, 0xf0]
        };
        for config in Config::all() {
            let mut sim = Simulator::new(analyze(&code, &config), None);
            for index in 0..2 {
                sim.set("index", Value::new(index as u64, 1, false));
                sim.step(&Event::clock(VarId::SYNTHETIC));
                for port in ["literal", "nested"] {
                    assert_eq!(
                        sim.get(port),
                        Some(Value::new(expected[index], 8, false)),
                        "{port} select={select} index={index} config={config:?}"
                    );
                }
                assert_eq!(sim.get("matrix"), Some(Value::new(other[index], 8, false)));
            }
        }
    }
}

#[test]
fn conditional_const_array_merges_unknown_condition_per_element() {
    let code = r#"
    package Tables {
        const X: logic<4> [2] = if 1'bx ? '{4'b1010, 4'b1100} : '{4'b1001, 4'b1100};
        const Z: logic<4> [2] = if 1'bz ? '{4'b1010, 4'b1100} : '{4'b1001, 4'b1100};
    }
    module Top (index: input logic, x: output logic<4>, z: output logic<4>) {
        assign x = Tables::X[index];
        assign z = Tables::Z[index];
    }
    "#;
    for config in Config::all().into_iter().filter(|c| c.use_4state) {
        let mut sim = Simulator::new(analyze(code, &config), None);
        for (index, expected) in [(0, "4'b10xx"), (1, "4'b1100")] {
            sim.set("index", Value::new(index, 1, false));
            sim.step(&Event::clock(VarId::SYNTHETIC));
            for port in ["x", "z"] {
                assert_eq!(sim.get(port), Some(expected.parse().unwrap()), "{config:?}");
            }
        }
    }
}
