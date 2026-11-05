use super::*;

#[test]
fn wide_array_slice_invalid_indices_preserve_value_representation() {
    for width in [1, 64, 65, 128, 129, 189, 256] {
        let code = format!(
            r#"
            module Top (index: input logic<3>, result: output logic<{width}>) {{
                var data: logic<320> [4];
                for i in 0..4 :row {{ assign data[i] = '1; }}
                assign result = data[index][{high}:22];
            }}
            "#,
            high = width + 21,
        );
        let ones = Value::from_str(&format!("{width}'b{}", "1".repeat(width))).unwrap();
        for config in Config::all() {
            let ir = analyze(&code, &config);
            let mut sim = Simulator::new(ir, None);
            let default = if config.use_4state {
                Value::new_x(width, false)
            } else {
                Value::new(0, width, false)
            };
            for index in [0, 3, 4, 7, 0] {
                sim.set("index", Value::new(index, 3, false));
                sim.step(&Event::clock(VarId::SYNTHETIC));
                let expected = if index < 4 { &ones } else { &default };
                assert_eq!(
                    &sim.get("result").unwrap(),
                    expected,
                    "width={width}, index={index}, config={config:?}"
                );
            }
            if config.use_4state {
                for index in [Value::new_x(3, false), Value::new_z(3, false)] {
                    sim.set("index", index);
                    sim.step(&Event::clock(VarId::SYNTHETIC));
                    assert_eq!(
                        sim.get("result").unwrap(),
                        default,
                        "width={width}, unknown index, config={config:?}"
                    );
                }
            }
        }
    }
}
