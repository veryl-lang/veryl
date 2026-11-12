use super::*;

fn configurations() -> Vec<Config> {
    let mut configs: Vec<_> = Config::all().into_iter().filter(|c| c.use_4state).collect();
    #[cfg(not(target_family = "wasm"))]
    if crate::backend::aot_c::cc_available() {
        configs.push(Config {
            use_4state: true,
            use_jit: true,
            aot_c: true,
            aot_c_async: false,
            aot_c_validate: true,
            ..Default::default()
        });
    }
    configs
}

fn expected_equality(a: &str, b: &str, invert: bool) -> Value {
    let mismatch = a
        .bytes()
        .zip(b.bytes())
        .any(|(a, b)| matches!(a, b'0' | b'1') && matches!(b, b'0' | b'1') && a != b);
    let unknown = a.bytes().chain(b.bytes()).any(|x| matches!(x, b'x' | b'z'));
    if !mismatch && unknown {
        "1'bx".parse().unwrap()
    } else {
        Value::new((mismatch == invert) as u64, 1, false)
    }
}

#[test]
fn equality_uses_only_bits_known_in_both_operands() {
    for width in [1usize, 8, 64, 65, 128, 129, 256] {
        let code = format!(
            r#"
        module Top (a: input logic<{width}>, b: input logic<{width}>,
                    eq: output logic, ne: output logic) {{
            assign eq = a == b;
            assign ne = a != b;
        }}
        "#
        );
        let mut patterns: Vec<String> = ['0', '1', 'x', 'z']
            .into_iter()
            .map(|c| c.to_string().repeat(width))
            .collect();
        for offset in 0..4 {
            patterns.push(
                (0..width)
                    .map(|i| ['0', '1', 'x', 'z'][(i + offset) % 4])
                    .collect(),
            );
        }
        for config in configurations() {
            let mut sim = Simulator::new(analyze(&code, &config), None);
            for a in &patterns {
                for b in &patterns {
                    sim.set("a", format!("{width}'b{a}").parse().unwrap());
                    sim.set("b", format!("{width}'b{b}").parse().unwrap());
                    sim.step(&Event::clock(VarId::SYNTHETIC));
                    for (port, invert) in [("eq", false), ("ne", true)] {
                        assert_eq!(
                            sim.get(port),
                            Some(expected_equality(a, b, invert)),
                            "{port} width={width} a={a} b={b} config={config:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn equality_unknown_against_constant_reaches_derived_condition() {
    let code = r#"
    module Top (id: input logic<8>, mask: output logic<4>, condition: output logic,
                reverse: output logic, unequal: output logic) {
        assign mask = {3'b0, id == 7};
        assign condition = (mask & (mask - 1)) == 0;
        assign reverse = 7 == id;
        assign unequal = id != 7;
    }
    "#;
    for config in configurations() {
        let mut sim = Simulator::new(analyze(code, &config), None);
        for (id, mask, condition, reverse, unequal) in [
            ("8'h07", "4'b0001", "1'b1", "1'b1", "1'b0"),
            ("8'hxx", "4'b000x", "1'bx", "1'bx", "1'bx"),
            ("8'hzz", "4'b000x", "1'bx", "1'bx", "1'bx"),
            ("8'b1xxxxxxx", "4'b0000", "1'b1", "1'b0", "1'b1"),
            ("8'h07", "4'b0001", "1'b1", "1'b1", "1'b0"),
        ] {
            sim.set("id", id.parse().unwrap());
            sim.step(&Event::clock(VarId::SYNTHETIC));
            for (port, value) in [
                ("mask", mask),
                ("condition", condition),
                ("reverse", reverse),
                ("unequal", unequal),
            ] {
                assert_eq!(
                    sim.get(port),
                    Some(value.parse().unwrap()),
                    "{port} id={id} config={config:?}"
                );
            }
        }
    }
}
