use super::*;

#[test]
fn wide_four_state_bitwise_results_are_canonical() {
    for width in [129usize, 256, 512] {
        let code = format!(
            r#"
        module Top (
            clk: input clock, rst: input reset, a: input logic<{width}>, b: input logic<{width}>,
            xo: output logic<{width}>, xn: output logic<{width}>,
            inv: output logic<{width}>, neg: output logic<{width}>,
        ) {{
            always_ff {{
                if_reset {{xo = 0; xn = 0; inv = 0; neg = 0;}}
                else {{xo = a ^ b; xn = a ~^ b; inv = ~a; neg = -a;}}
            }}
        }}
        "#
        );
        let digits: String = (0..width)
            .map(|i| ['0', '1', 'x', 'z'][(i + i / 64) % 4])
            .collect();
        let direct: String = digits
            .chars()
            .map(|c| if c == 'z' { 'x' } else { c })
            .collect();
        let inverted: String = digits
            .chars()
            .map(|c| match c {
                '0' => '1',
                '1' => '0',
                _ => 'x',
            })
            .collect();
        for use_jit in [false, true] {
            let config = Config {
                use_4state: true,
                use_jit,
                ..Default::default()
            };
            let ir = analyze(&code, &config);
            if use_jit {
                let (_, _, compiled, interpreted) = ir.detailed_stats();
                assert!(compiled > 0);
                assert_eq!(interpreted, 0);
            }
            let mut sim = Simulator::new(ir, None);
            let clock = sim.get_clock("clk").unwrap();
            let reset = sim.get_reset("rst").unwrap();
            sim.step_reset(&clock, &reset);
            sim.set("a", Value::from_str(&format!("{width}'b{digits}")).unwrap());
            sim.set("b", Value::new(0, width, false));
            sim.step(&clock);
            for (port, bits) in [
                ("xo", direct.as_str()),
                ("xn", inverted.as_str()),
                ("inv", inverted.as_str()),
                ("neg", &"x".repeat(width)),
            ] {
                assert_eq!(
                    sim.get(port),
                    Some(Value::from_str(&format!("{width}'b{bits}")).unwrap()),
                    "{port}, width={width}, jit={use_jit}"
                );
            }
        }
    }
}

#[test]
fn wide_four_state_shifts_keep_known_bits() {
    let code = r#"
    module Top (
        clk: input clock, rst: input reset, a: input logic<160>, b: input logic<8>,
        shr: output logic<160>, shl: output logic<160>, shb: output logic<160>,
    ) {
        always_ff {
            if_reset {shr = 0; shl = 0; shb = 0;}
            else {shr = a >> 8; shl = a << 8; shb = a >> b;}
        }
    }
    "#;
    let known: String = (0..128).map(|i| ['0', '1'][(i / 3) % 2]).collect();
    let digits = format!("{}{known}", "x".repeat(32));
    let shifted_right = format!("{}{}", "0".repeat(8), &digits[..152]);
    let shifted_left = format!("{}{}", &digits[8..], "0".repeat(8));
    for use_jit in [false, true] {
        let config = Config {
            use_4state: true,
            use_jit,
            ..Default::default()
        };
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clock = sim.get_clock("clk").unwrap();
        let reset = sim.get_reset("rst").unwrap();
        sim.step_reset(&clock, &reset);
        sim.set("a", Value::from_str(&format!("160'b{digits}")).unwrap());
        sim.set("b", Value::from_str("8'bx").unwrap());
        sim.step(&clock);
        for (port, bits) in [
            ("shr", shifted_right.as_str()),
            ("shl", shifted_left.as_str()),
            ("shb", &"x".repeat(160)),
        ] {
            assert_eq!(
                sim.get(port),
                Some(Value::from_str(&format!("160'b{bits}")).unwrap()),
                "{port}, jit={use_jit}"
            );
        }
    }
}
