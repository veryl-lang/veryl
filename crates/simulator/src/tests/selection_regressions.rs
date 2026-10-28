use super::*;

#[test]
fn wide_ff_part_select_keeps_assignment_order() {
    for width in [129, 200] {
        for array in [false, true] {
            check_ff_part_select_order(width, array);
        }
    }
}

#[test]
fn native_ff_part_select_keeps_assignment_order() {
    for width in [65, 100, 128] {
        check_ff_part_select_order(width, true);
    }
}

fn check_ff_part_select_order(width: usize, array: bool) {
    let declaration = if array {
        format!("var q: logic<{width}> [2];")
    } else {
        format!("var q: logic<{width}>;")
    };
    let target = if array { "q[element]" } else { "q" };
    let reset = if array {
        "q[0] = 0; q[1] = 0;"
    } else {
        "q = 0;"
    };
    let code = format!(
        r#"
        module Top (
            clk: input clock, rst: input reset, element: input logic,
            index: input logic<16>, patch: input logic<8>, reverse: input logic,
            out: output logic<{width}>,
        ) {{
            {declaration}
            always_ff {{
                if_reset {{{reset}}}
                else if reverse {{
                    {target}[index-:8] = patch;
                    {target} = '1;
                }} else {{
                    {target} = '1;
                    {target}[index-:8] = patch;
                }}
            }}
            assign out = {target};
        }}
        "#
    );
    for mut config in Config::all() {
        config.aot_c_min_stmts = 0;
        let mut sim = Simulator::new(analyze(&code, &config), None);
        let clock = sim.get_clock("clk").unwrap();
        let reset = sim.get_reset("rst").unwrap();
        sim.step_reset(&clock, &reset);
        sim.set("patch", Value::new(0, 8, false));
        for element in [0, 1] {
            sim.set("element", Value::new(element, 1, false));
            for index in [1, 7, 67, width - 1, width + 6, width + 7] {
                sim.set("index", Value::new(index as u64, 16, false));
                for reverse in [true, false] {
                    sim.set("reverse", Value::new(u64::from(reverse), 1, false));
                    sim.step(&clock);
                    let bits: String = (0..width)
                        .rev()
                        .map(|bit| {
                            if !reverse
                                && (index as i128 - 7..=index as i128).contains(&(bit as i128))
                            {
                                '0'
                            } else {
                                '1'
                            }
                        })
                        .collect();
                    assert_eq!(
                        sim.get("out"),
                        Some(Value::from_str(&format!("{width}'b{bits}")).unwrap()),
                        "width={width} array={array} element={element} index={index} reverse={reverse} config={config:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn packed_ff_range_write_stays_within_selected_row() {
    let code = r#"
        module Top (
            clk: input clock, rst: input reset, row: input logic<3>,
            index: input signed logic<16>, patch: input logic<4>,
            out: output logic<4, 8>,
        ) {
            var q: logic<4, 8>;
            always_ff {
                if_reset {q = 0;}
                else {q[row][index-:4] = patch;}
            }
            assign out = q;
        }
    "#;
    for mut config in Config::all() {
        config.aot_c_min_stmts = 0;
        let mut sim = Simulator::new(analyze(code, &config), None);
        let clock = sim.get_clock("clk").unwrap();
        let reset = sim.get_reset("rst").unwrap();
        for row in [1, 0, 3, 4] {
            for index in [1i64, 7, 9, -1] {
                sim.step_reset(&clock, &reset);
                sim.set("row", Value::new(row, 3, false));
                sim.set("index", Value::new(index as u64, 16, true));
                sim.set("patch", Value::new(15, 4, false));
                sim.step(&clock);
                let mut expected = 0;
                if row < 4 {
                    for bit in index - 3..=index {
                        if (0..8).contains(&bit) {
                            expected |= 1 << (row * 8 + bit as u64);
                        }
                    }
                }
                assert_eq!(
                    sim.get("out"),
                    Some(Value::new(expected, 32, false)),
                    "row={row} index={index} config={config:?}"
                );
            }
        }
    }
}

#[test]
fn dynamic_array_read_rejects_unknown_index() {
    for width in [8usize, 65, 128, 129, 232] {
        for index_width in [1usize, 16] {
            let code = format!(
                r#"
            module Top (
                a: input logic<{width}>, b: input logic<{width}>,
                index: input logic<{index_width}>, out: output logic<{width}>,
            ) {{
                var arr: logic<{width}> [2];
                always_comb {{arr[0] = a; arr[1] = b;}}
                assign out = arr[index];
            }}
            "#
            );
            for use_jit in [false, true] {
                let config = Config {
                    use_4state: true,
                    use_jit,
                    ..Default::default()
                };
                let ir = analyze(&code, &config);
                if use_jit {
                    let (compiled, interpreted, _, _) = ir.detailed_stats();
                    assert!(compiled > 0);
                    assert_eq!(interpreted, 0);
                }
                let mut sim = Simulator::new(ir, None);
                sim.set("a", Value::new(0, width, false));
                sim.set("b", Value::new(1, width, false));
                for index in [0, 1] {
                    sim.set("index", Value::new(index, index_width, false));
                    assert_eq!(sim.get("out"), Some(Value::new(index, width, false)));
                }
                for bit in ['x', 'z'] {
                    let digits = format!("{}{bit}", "0".repeat(index_width - 1));
                    sim.set(
                        "index",
                        Value::from_str(&format!("{index_width}'b{digits}")).unwrap(),
                    );
                    assert_eq!(
                        sim.get("out"),
                        Some(Value::new_x(width, false)),
                        "width={width} index_width={index_width} jit={use_jit} bit={bit}"
                    );
                }
            }
        }
    }
}

#[test]
fn dynamic_array_write_rejects_unknown_index() {
    for width in [8, 64] {
        for index_width in [1, 16] {
            let code = format!(
                r#"
                module Top (
                    clk: input clock, rst: input reset,
                    index: input logic<{index_width}>, data: input logic<{width}>,
                    o0: output logic<{width}>, o1: output logic<{width}>,
                    previous: output logic<{width}>,
                ) {{
                    var arr: logic<{width}> [2]; var prev: logic<{width}>;
                    always_ff {{
                        if_reset {{arr[0] = 0; arr[1] = 0; prev = 0;}}
                        else {{arr[index] = data; prev = arr[0];}}
                    }}
                    assign o0 = arr[0]; assign o1 = arr[1]; assign previous = prev;
                }}
                "#
            );
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
                let mut expected = [0x12, 0x34];
                for (index, data) in expected.into_iter().enumerate() {
                    sim.set("index", Value::new(index as u64, index_width, false));
                    sim.set("data", Value::new(data, width, false));
                    sim.step(&clock);
                }
                for bit in ['x', 'z'] {
                    let digits = format!("{}{bit}", "0".repeat(index_width - 1));
                    sim.set(
                        "index",
                        Value::from_str(&format!("{index_width}'b{digits}")).unwrap(),
                    );
                    sim.set("data", Value::new(0x56, width, false));
                    sim.step(&clock);
                    for (i, data) in expected.into_iter().enumerate() {
                        assert_eq!(
                            sim.get(&format!("o{i}")),
                            Some(Value::new(data, width, false)),
                            "width={width} index_width={index_width} jit={use_jit} bit={bit}"
                        );
                    }
                    assert_eq!(
                        sim.get("previous"),
                        Some(Value::new(expected[0], width, false))
                    );
                    let old = expected[0];
                    expected[0] += 1;
                    sim.set("index", Value::new(0, index_width, false));
                    sim.set("data", Value::new(expected[0], width, false));
                    sim.step(&clock);
                    assert_eq!(sim.get("o0"), Some(Value::new(expected[0], width, false)));
                    assert_eq!(sim.get("previous"), Some(Value::new(old, width, false)));
                }
            }
        }
    }
}

#[test]
fn packed_select_write_rejects_unknown_index() {
    for (array, width, index_width, window) in [
        (false, 8, 1, 1),
        (false, 8, 1, 8),
        (false, 65, 16, 8),
        (false, 129, 16, 8),
        (true, 8, 1, 1),
        (true, 8, 1, 8),
        (true, 65, 16, 8),
        (true, 129, 16, 8),
    ] {
        let decl = if array {
            format!("var arr: logic<{width}> [2];")
        } else {
            format!("var q: logic<{width}>;")
        };
        let target = if array { "arr[element]" } else { "q" };
        let first = if array { "arr[0]" } else { "q" };
        let select = if window == 1 {
            "[index]".to_string()
        } else {
            format!("[index+:{window}]")
        };
        let reset = if array {
            "arr[0] = 0; arr[1] = 0;"
        } else {
            "q = 0;"
        };
        let code = format!(
            r#"
            module Top (
                clk: input clock, rst: input reset, load: input logic,
                element: input logic, index: input logic<{index_width}>,
                data: input logic<{width}>, patch: input logic<{window}>,
                out: output logic<{width}>, previous: output logic<{width}>, untouched: output logic<{width}>,
            ) {{
                {decl} var prev: logic<{width}>;
                always_ff {{if_reset {{{reset} prev = 0;}}
                    else {{if load {{{target} = data;}} else {{{target}{select} = patch;}} prev = {first};}}
                }}
                assign out = {target}; assign previous = prev; assign untouched = {first};
            }}
        "#
        );
        for use_jit in [false, true] {
            let config = Config {
                use_4state: true,
                use_jit,
                ..Default::default()
            };
            let ir = analyze(&code, &config);
            if use_jit {
                let (_, _, compiled, interpreted) = ir.detailed_stats();
                if (!array && width <= 128) || (array && width <= 64) {
                    assert!(compiled > 0 && interpreted == 0);
                } else {
                    assert!(interpreted > 0);
                }
            }
            let mut sim = Simulator::new(ir, None);
            let clock = sim.get_clock("clk").unwrap();
            let reset = sim.get_reset("rst").unwrap();
            sim.step_reset(&clock, &reset);
            for bit in [0, index_width - 1] {
                for unknown in ['x', 'z'] {
                    sim.set("load", Value::new(1, 1, false));
                    sim.set("data", Value::new(64, width, false));
                    for element in 0..if array { 2 } else { 1 } {
                        sim.set("element", Value::new(element, 1, false));
                        sim.step(&clock);
                    }
                    let mut digits = vec!['0'; index_width];
                    digits[index_width - 1 - bit] = unknown;
                    sim.set(
                        "index",
                        Value::from_str(&format!(
                            "{index_width}'b{}",
                            digits.iter().collect::<String>()
                        ))
                        .unwrap(),
                    );
                    sim.set("load", Value::new(0, 1, false));
                    sim.set("patch", Value::new((1 << window) - 1, window, false));
                    sim.step(&clock);
                    for port in ["out", "previous", "untouched"] {
                        assert_eq!(
                            sim.get(port),
                            Some(Value::new(64, width, false)),
                            "array={array} width={width} jit={use_jit} index_width={index_width} bit={bit} unknown={unknown} port={port}"
                        );
                    }
                    let start = if index_width == 1 { 1 } else { width - 1 };
                    sim.set("index", Value::new(start as u64, index_width, false));
                    sim.step(&clock);
                    let mut digits = vec!['0'; width];
                    digits[width - 1 - 6] = '1';
                    for i in start..(start + window).min(width) {
                        digits[width - 1 - i] = '1';
                    }
                    let expected =
                        Value::from_str(&format!("{width}'b{}", digits.iter().collect::<String>()))
                            .unwrap();
                    assert_eq!(sim.get("out"), Some(expected.clone()));
                    assert_eq!(sim.get("previous"), Some(Value::new(64, width, false)));
                    assert_eq!(
                        sim.get("untouched"),
                        Some(if array {
                            Value::new(64, width, false)
                        } else {
                            expected
                        })
                    );
                }
            }
        }
    }
}

#[test]
fn descending_packed_write_clips_both_boundaries() {
    for width in [8, 65, 129] {
        for array in [false, true] {
            for descending in [false, true] {
                let target = if array { "arr[element]" } else { "q" };
                let first = if array { "arr[0]" } else { "q" };
                let declaration = if array {
                    format!("var arr: logic<{width}> [2];")
                } else {
                    format!("var q: logic<{width}>;")
                };
                let reset = if array {
                    "arr[0] = 0; arr[1] = 0;"
                } else {
                    "q = 0;"
                };
                let select = if descending {
                    "[index-:8]"
                } else {
                    "[index step 8]"
                };
                let code = format!(
                    r#"
                    module Top (
                        clk: input clock, rst: input reset, load: input logic,
                        element: input logic, index: input logic<16>, patch: input logic<8>,
                        out: output logic<{width}>, previous: output logic<{width}>,
                    ) {{
                        {declaration} var prev: logic<{width}>;
                        always_ff {{if_reset {{{reset} prev = 0;}}
                            else {{if load {{{target} = 0;}} else {{{target}{select} = patch;}} prev = {first};}}
                        }}
                        assign out = {target}; assign previous = prev;
                    }}
                "#
                );
                for config in Config::all() {
                    let mut sim = Simulator::new(analyze(&code, &config), None);
                    let clock = sim.get_clock("clk").unwrap();
                    let reset = sim.get_reset("rst").unwrap();
                    sim.step_reset(&clock, &reset);
                    sim.set("element", Value::new(u64::from(array), 1, false));
                    for index in [
                        0usize,
                        1,
                        6,
                        7,
                        8,
                        9,
                        63,
                        64,
                        width - 1,
                        width,
                        width + 1,
                        65535,
                    ] {
                        sim.set("load", Value::new(1, 1, false));
                        sim.step(&clock);
                        sim.set("load", Value::new(0, 1, false));
                        sim.set("index", Value::new(index as u64, 16, false));
                        sim.set("patch", Value::new(0xa6, 8, false));
                        sim.step(&clock);
                        let mut bytes = vec![0u8; width.div_ceil(64) * 8];
                        for bit in 0..8 {
                            let dst = if descending {
                                index as i128 - 7 + bit
                            } else {
                                index as i128 * 8 + bit
                            };
                            if dst >= 0 && dst < width as i128 && 0xa6 & (1 << bit) != 0 {
                                bytes[dst as usize / 8] |= 1 << (dst as usize % 8);
                            }
                        }
                        assert_eq!(
                            sim.get("out"),
                            Some(Value::from_le_bytes(
                                &bytes,
                                &vec![0; bytes.len()],
                                width,
                                false
                            )),
                            "width={width} array={array} descending={descending} index={index} config={config:?}"
                        );
                        assert_eq!(sim.get("previous"), Some(Value::new(0, width, false)));
                    }
                }
            }
        }
    }
}

#[test]
fn signed_packed_write_preserves_negative_positions() {
    for (array, width, bits) in [
        (false, 8usize, 1usize),
        (true, 8, 1),
        (false, 64, 4),
        (true, 64, 4),
        (false, 129, 16),
        (true, 129, 16),
    ] {
        let mut forms = vec![("bit", 1), ("plus", 8), ("minus", 8), ("step", 8)];
        if width >= 64 {
            forms.extend([("plus", 64), ("minus", 64), ("step", 64)]);
        }
        for (form, window) in forms {
            let target = if array { "arr[element]" } else { "q" };
            let first = if array { "arr[0]" } else { "q" };
            let declaration = if array {
                format!("var arr: logic<{width}> [2];")
            } else {
                format!("var q: logic<{width}>;")
            };
            let reset = if array {
                "arr[0] = 0; arr[1] = 0;"
            } else {
                "q = 0;"
            };
            let select = match form {
                "bit" => "[index]".to_string(),
                "plus" => format!("[index+:{window}]"),
                "minus" => format!("[index-:{window}]"),
                _ => format!("[index step {window}]"),
            };
            let code = format!(
                r#"
                module Top (
                    clk: input clock, rst: input reset, load: input logic, element: input logic,
                    index: input signed logic<{bits}>, patch: input logic<{window}>,
                    out: output logic<{width}>, previous: output logic<{width}>,
                ) {{
                    {declaration} var prev: logic<{width}>;
                    always_ff {{if_reset {{{reset} prev = 0;}}
                        else {{if load {{{target} = 0;}} else {{{target}{select} = patch;}} prev = {first};}}
                    }}
                    assign out = {target}; assign previous = prev;
                }}
            "#
            );
            for config in Config::all() {
                let mut sim = Simulator::new(analyze(&code, &config), None);
                let clock = sim.get_clock("clk").unwrap();
                let reset = sim.get_reset("rst").unwrap();
                sim.step_reset(&clock, &reset);
                sim.set("element", Value::new(u64::from(array), 1, false));
                let min = -(1i128 << (bits - 1));
                let max = (1i128 << (bits - 1)) - 1;
                for n in [min, 1 - window as i128, -1, 0, 1, max] {
                    if n < min || n > max {
                        continue;
                    }
                    sim.set("load", Value::new(1, 1, false));
                    sim.step(&clock);
                    sim.set("load", Value::new(0, 1, false));
                    sim.set(
                        "index",
                        Value::new(((n as u128) & ((1u128 << bits) - 1)) as u64, bits, true),
                    );
                    let patch = if window == 64 {
                        u64::MAX
                    } else {
                        (1u64 << window) - 1
                    };
                    sim.set("patch", Value::new(patch, window, false));
                    sim.step(&clock);
                    let mut bytes = vec![0u8; width.div_ceil(64) * 8];
                    for b in 0..window {
                        let dst = match form {
                            "minus" => n - (window as i128 - 1) + b as i128,
                            "step" => n * window as i128 + b as i128,
                            _ => n + b as i128,
                        };
                        if dst >= 0 && dst < width as i128 {
                            bytes[dst as usize / 8] |= 1 << (dst as usize % 8);
                        }
                    }
                    assert_eq!(
                        sim.get("out"),
                        Some(Value::from_le_bytes(
                            &bytes,
                            &vec![0; bytes.len()],
                            width,
                            false
                        )),
                        "array={array} width={width} bits={bits} form={form} window={window} index={n} config={config:?}"
                    );
                    assert_eq!(sim.get("previous"), Some(Value::new(0, width, false)));
                }
            }
        }
    }
}

#[test]
fn wide_packed_write_does_not_wrap() {
    for (array, width, bits, signed) in [
        (false, 8usize, 32usize, false),
        (true, 8, 32, false),
        (false, 64, 64, true),
        (true, 64, 64, true),
        (false, 129, 64, false),
        (true, 129, 64, true),
    ] {
        let mut forms = vec![
            ("bit", 1),
            ("plus", 3),
            ("minus", 3),
            ("step", 3),
            ("plus", 8),
            ("minus", 8),
            ("step", 8),
        ];
        if width >= 64 {
            forms.extend([("plus", 64), ("minus", 64), ("step", 64)]);
        }
        for (form, window) in forms {
            let target = if array { "arr[element]" } else { "q" };
            let first = if array { "arr[0]" } else { "q" };
            let declaration = if array {
                format!("var arr: logic<{width}> [2];")
            } else {
                format!("var q: logic<{width}>;")
            };
            let reset = if array {
                "arr[0] = 0; arr[1] = 0;"
            } else {
                "q = 0;"
            };
            let sign = if signed { "signed " } else { "" };
            let select = match form {
                "bit" => "[index]".to_string(),
                "plus" => format!("[index+:{window}]"),
                "minus" => format!("[index-:{window}]"),
                _ => format!("[index step {window}]"),
            };
            let code = format!(
                r#"
                module Top (
                    clk: input clock, rst: input reset, load: input logic, element: input logic,
                    index: input {sign}logic<{bits}>, patch: input logic<{window}>,
                    out: output logic<{width}>, previous: output logic<{width}>,
                ) {{
                    {declaration} var prev: logic<{width}>;
                    always_ff {{if_reset {{{reset} prev = 0;}}
                        else {{if load {{{target} = 0;}} else {{{target}{select} = patch;}} prev = {first};}}
                    }}
                    assign out = {target}; assign previous = prev;
                }}
            "#
            );
            let min = if signed { -(1i128 << (bits - 1)) } else { 0 };
            let max = (1i128 << (bits - usize::from(signed))) - 1;
            let mut indices = vec![
                min,
                min + 1,
                -(window as i128),
                -1,
                0,
                1,
                width as i128 - 1,
                width as i128,
                width as i128 + 1,
                max - 1,
                max,
            ];
            for power in [32, 64] {
                let q = (1i128 << power) / window as i128;
                indices.extend([q - 1, q, q + 1, -q - 1, -q, -q + 1]);
            }
            indices.retain(|n| *n >= min && *n <= max);
            indices.sort();
            indices.dedup();
            for config in Config::all() {
                let mut sim = Simulator::new(analyze(&code, &config), None);
                let clock = sim.get_clock("clk").unwrap();
                let reset = sim.get_reset("rst").unwrap();
                sim.step_reset(&clock, &reset);
                sim.set("element", Value::new(u64::from(array), 1, false));
                for &n in &indices {
                    sim.set("load", Value::new(1, 1, false));
                    sim.step(&clock);
                    sim.set("load", Value::new(0, 1, false));
                    sim.set(
                        "index",
                        Value::new(((n as u128) & ((1u128 << bits) - 1)) as u64, bits, signed),
                    );
                    let patch = if window == 64 {
                        u64::MAX
                    } else {
                        (1u64 << window) - 1
                    };
                    sim.set("patch", Value::new(patch, window, false));
                    sim.step(&clock);
                    let mut bytes = vec![0u8; width.div_ceil(64) * 8];
                    for b in 0..window {
                        let dst = match form {
                            "minus" => n - (window as i128 - 1) + b as i128,
                            "step" => n * window as i128 + b as i128,
                            _ => n + b as i128,
                        };
                        if dst >= 0 && dst < width as i128 {
                            bytes[dst as usize / 8] |= 1 << (dst as usize % 8);
                        }
                    }
                    assert_eq!(
                        sim.get("out"),
                        Some(Value::from_le_bytes(
                            &bytes,
                            &vec![0; bytes.len()],
                            width,
                            false
                        )),
                        "array={array} width={width} bits={bits} signed={signed} form={form} window={window} index={n} config={config:?}"
                    );
                    assert_eq!(sim.get("previous"), Some(Value::new(0, width, false)));
                }
            }
        }
    }
}

#[test]
fn packed_read_preserves_partial_boundaries() {
    check_packed_read_boundaries(false);
}

#[test]
fn wide_packed_read_preserves_partial_boundaries() {
    check_packed_read_boundaries(true);
}

fn check_packed_read_boundaries(wide: bool) {
    let configurations = if wide {
        vec![
            (false, 65usize, 1usize, false),
            (true, 65, 4, true),
            (false, 129, 64, true),
            (true, 129, 64, false),
            (false, 512, 64, false),
            (true, 512, 64, true),
        ]
    } else {
        vec![
            (false, 8usize, 1usize, false),
            (true, 8, 1, true),
            (false, 65, 4, true),
            (true, 65, 4, true),
            (false, 129, 64, true),
            (true, 129, 64, false),
        ]
    };
    for (array, width, bits, signed) in configurations {
        let mut forms = vec![
            ("bit", 1),
            ("plus", 3),
            ("minus", 3),
            ("step", 3),
            ("plus", 8),
            ("minus", 8),
            ("step", 8),
        ];
        if width >= 64 {
            forms.extend([("plus", 64), ("minus", 64), ("step", 64)]);
        }
        if wide {
            forms = [65, 128, 129, 255, 512]
                .into_iter()
                .filter(|&w| w <= width)
                .flat_map(|w| [("plus", w), ("minus", w), ("step", w)])
                .collect();
        }
        for (form, window) in forms {
            let select = match form {
                "bit" => "[index]".to_string(),
                "plus" => format!("[index+:{window}]"),
                "minus" => format!("[index-:{window}]"),
                _ => format!("[index step {window}]"),
            };
            let sign = if signed { "signed " } else { "" };
            let target = if array { "arr[element]" } else { "q" };
            let declarations = if array {
                format!("var arr: logic<{width}> [2]; assign arr[0] = 0; assign arr[1] = data;")
            } else {
                format!("var q: logic<{width}>; assign q = data;")
            };
            let code = format!(
                r#"
                module Top(data: input logic<{width}>, index: input {sign}logic<{bits}>, element: input logic,
                           out: output logic<{window}>, mirror: output logic<{width}>) {{
                    {declarations} assign out = {target}{select}; assign mirror = data;
                }}
            "#
            );
            for config in Config::all() {
                let mut sim = Simulator::new(analyze(&code, &config), None);
                let data: Vec<char> = (0..width)
                    .map(|b| {
                        if config.use_4state {
                            ['0', '1', 'x', 'z'][(b + b / 64) % 4]
                        } else {
                            ['0', '1'][(b + b / 64) % 2]
                        }
                    })
                    .collect();
                let digits: String = data.iter().rev().collect();
                let data_value = Value::from_str(&format!("{width}'b{digits}")).unwrap();
                sim.set("data", data_value.clone());
                sim.set("element", Value::new(u64::from(array), 1, false));
                let min = if signed { -(1i128 << (bits - 1)) } else { 0 };
                let max = (1i128 << (bits - usize::from(signed))) - 1;
                let mut indices = vec![
                    min,
                    min + 1,
                    -65,
                    -8,
                    -3,
                    -1,
                    0,
                    1,
                    2,
                    7,
                    8,
                    63,
                    64,
                    width as i128 - 1,
                    width as i128,
                    width as i128 + 1,
                    max,
                ];
                for power in [32, 64] {
                    let q = (1i128 << power) / window as i128;
                    indices.extend([q, q + 1, -q, -q - 1]);
                }
                indices.retain(|n| *n >= min && *n <= max);
                indices.sort();
                indices.dedup();
                for n in indices {
                    sim.set(
                        "index",
                        Value::new(((n as u128) & ((1u128 << bits) - 1)) as u64, bits, signed),
                    );
                    let mut expected = vec![if config.use_4state { 'x' } else { '0' }; window];
                    for (b, v) in expected.iter_mut().enumerate() {
                        let src = match form {
                            "minus" => n - (window as i128 - 1) + b as i128,
                            "step" => n * window as i128 + b as i128,
                            _ => n + b as i128,
                        };
                        if src >= 0 && src < width as i128 {
                            *v = data[src as usize];
                        }
                    }
                    let expected: String = expected.iter().rev().collect();
                    assert_eq!(
                        sim.get("out"),
                        Some(Value::from_str(&format!("{window}'b{expected}")).unwrap()),
                        "array={array} width={width} bits={bits} signed={signed} form={form} window={window} index={n} config={config:?}"
                    );
                    assert_eq!(sim.get("mirror"), Some(data_value.clone()));
                }
                for state in ['x', 'z'] {
                    let digits = format!("{}{state}", "0".repeat(bits - 1));
                    sim.set(
                        "index",
                        Value::from_str(&format!("{bits}'b{digits}")).unwrap(),
                    );
                    if config.use_4state {
                        assert_eq!(sim.get("out"), Some(Value::new_x(window, false)));
                    }
                }
            }
        }
    }
}

#[test]
fn constant_packed_read_uses_selected_width() {
    for width in [129usize, 512] {
        for window in [1usize, 8, 65, 128, 129] {
            let starts = [0, 63, width - 1, width];
            let mut ports = String::new();
            let mut assignments = String::new();
            for (i, start) in starts.iter().enumerate() {
                ports += &format!("o{i}: output logic<{window}>, ");
                let select = if window == 1 {
                    "[j]".to_owned()
                } else {
                    format!("[j+:{window}]")
                };
                assignments += &format!(
                    "always_comb {{for j in {start}..{} {{o{i} = data{select};}}}}",
                    start + 1
                );
            }
            let source =
                format!("module Top(data: input logic<{width}>, {ports}) {{{assignments}}}");
            for config in Config::all() {
                let mut sim = Simulator::new(analyze(&source, &config), None);
                let data: Vec<_> = (0..width)
                    .map(|b| {
                        if config.use_4state {
                            ['0', '1', 'x', 'z'][(b + b / 64) % 4]
                        } else {
                            ['0', '1'][(b + b / 64) % 2]
                        }
                    })
                    .collect();
                let digits: String = data.iter().rev().collect();
                sim.set(
                    "data",
                    Value::from_str(&format!("{width}'b{digits}")).unwrap(),
                );
                for (i, start) in starts.iter().enumerate() {
                    let expected: String = (0..window)
                        .rev()
                        .map(|b| {
                            data.get(start + b)
                                .copied()
                                .unwrap_or(if config.use_4state { 'x' } else { '0' })
                        })
                        .collect();
                    assert_eq!(
                        sim.get(&format!("o{i}")),
                        Some(Value::from_str(&format!("{window}'b{expected}")).unwrap()),
                        "width={width} window={window} start={start} config={config:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn constant_packed_edges_preserve_unknowns_and_lower_overlap() {
    for width in [8usize, 65, 129] {
        for window in [1usize, 3, 65].into_iter().filter(|&w| w <= width) {
            let forms = if window == 1 {
                vec!["bit"]
            } else {
                vec!["plus", "minus", "step"]
            };
            for form in forms {
                let mut cases = vec![
                    ("32'b0000000000000000000000000000000x".to_string(), None, 0),
                    ("32'b0000000000000000000000000000000z".to_string(), None, 1),
                    ("((j as i64) + 64'sd0)".to_string(), Some(0i128), 0),
                ];
                if matches!(form, "plus" | "step") {
                    cases.push(("((j as i64) - 64'sd1)".into(), Some(-1), -1));
                }
                let mut ports = String::new();
                let mut declarations = String::new();
                for (i, (expr, coordinate, _)) in cases.iter().enumerate() {
                    ports +=
                        &format!("rd_{i}: output logic<{window}>, wr_{i}: output logic<{width}>, ");
                    let select = match form {
                        "bit" => format!("[{expr}]"),
                        "plus" => format!("[{expr}+:{window}]"),
                        "minus" => format!("[{expr}-:{window}]"),
                        _ => format!("[{expr} step {window}]"),
                    };
                    let body = format!("rd_{i} = data{select}; q_{i}{select} = patch;");
                    let body = if coordinate.is_some() {
                        format!("for j in 0..1 {{{body}}}")
                    } else {
                        body
                    };
                    declarations += &format!(
                        "var q_{i}: logic<{width}>;always_comb {{q_{i} = data;{body}}} assign wr_{i} = q_{i};"
                    );
                }
                let source = format!(
                    "module Top(data: input logic<{width}>, patch: input logic<{window}>, {ports}) {{{declarations}}}"
                );
                for config in Config::all() {
                    let mut sim = Simulator::new(analyze(&source, &config), None);
                    let data: Vec<_> = (0..width)
                        .map(|b| {
                            if config.use_4state {
                                ['0', '1', 'x', 'z'][(b + b / 64) % 4]
                            } else {
                                ['0', '1'][(b + b / 64) % 2]
                            }
                        })
                        .collect();
                    let patch: Vec<_> = (0..window)
                        .map(|b| {
                            if config.use_4state {
                                ['1', 'z', '0', 'x'][b % 4]
                            } else {
                                ['1', '0'][b % 2]
                            }
                        })
                        .collect();
                    let parse = |bits: &[char]| {
                        let digits: String = bits.iter().rev().collect();
                        Value::from_str(&format!("{}'b{digits}", bits.len())).unwrap()
                    };
                    sim.set("data", parse(&data));
                    sim.set("patch", parse(&patch));
                    for (i, (_, coordinate, payload)) in cases.iter().enumerate() {
                        let coordinate = if config.use_4state {
                            *coordinate
                        } else {
                            Some(*payload)
                        };
                        let start = coordinate.map(|n| match form {
                            "minus" => n - window as i128 + 1,
                            "step" => n * window as i128,
                            _ => n,
                        });
                        let default = if config.use_4state { 'x' } else { '0' };
                        let rd: Vec<_> = (0..window)
                            .map(|b| {
                                start
                                    .and_then(|s| usize::try_from(s + b as i128).ok())
                                    .and_then(|s| data.get(s).copied())
                                    .unwrap_or(default)
                            })
                            .collect();
                        let mut wr = data.clone();
                        if let Some(start) = start {
                            for (b, v) in patch.iter().enumerate() {
                                if let Ok(bit) = usize::try_from(start + b as i128)
                                    && let Some(dst) = wr.get_mut(bit)
                                {
                                    *dst = *v;
                                }
                            }
                        }
                        assert_eq!(
                            sim.get(&format!("rd_{i}")),
                            Some(parse(&rd)),
                            "read width={width} window={window} form={form} case={i} config={config:?}"
                        );
                        assert_eq!(
                            sim.get(&format!("wr_{i}")),
                            Some(parse(&wr)),
                            "write width={width} window={window} form={form} case={i} config={config:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn packed_multidimensional_select_checks_each_coordinate() {
    for shape in [vec![3usize, 16], vec![2, 3, 16], vec![2, 3, 65]] {
        let full: usize = shape.iter().product();
        let dimensions = shape
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut ports = String::new();
        let mut statements = String::new();
        for axis in 0..shape.len() {
            ports += &format!("idx_{axis}: input signed logic<64>, ");
        }
        for depth in 1..=shape.len() {
            let window: usize = shape[depth..].iter().product();
            let select = (0..depth).map(|a| format!("[idx_{a}]")).collect::<String>();
            ports += &format!(
                "patch_{depth}: input logic<{window}>, rd_{depth}: output logic<{window}>, wr_{depth}: output logic<{full}>, "
            );
            statements += &format!(
                "var q_{depth}: logic<{dimensions}>;always_comb {{q_{depth} = data;q_{depth}{select} = patch_{depth};}}assign rd_{depth} = data{select};assign wr_{depth} = q_{depth};"
            );
        }
        let source =
            format!("module Top(data: input logic<{dimensions}>, {ports}) {{{statements}}}");
        for config in Config::all() {
            let mut sim = Simulator::new(analyze(&source, &config), None);
            let data: Vec<_> = (0..full)
                .map(|b| {
                    if config.use_4state {
                        ['0', '1', 'x', 'z'][(b + b / 64) % 4]
                    } else {
                        ['0', '1'][(b + b / 64) % 2]
                    }
                })
                .collect();
            let parse = |bits: &[char]| {
                let digits: String = bits.iter().rev().collect();
                Value::from_str(&format!("{}'b{digits}", bits.len())).unwrap()
            };
            sim.set("data", parse(&data));
            let mut patches = Vec::new();
            for depth in 1..=shape.len() {
                let window = shape[depth..].iter().product();
                let patch: Vec<_> = (0..window)
                    .map(|b| {
                        if config.use_4state {
                            ['1', 'z', '0', 'x'][b % 4]
                        } else {
                            ['1', '0'][b % 2]
                        }
                    })
                    .collect();
                sim.set(&format!("patch_{depth}"), parse(&patch));
                patches.push(patch);
            }
            let mut cases = vec![
                (vec![0i128; shape.len()], None),
                (shape.iter().map(|&d| d as i128 - 1).collect(), None),
            ];
            for axis in 0..shape.len() {
                for n in [
                    -1,
                    shape[axis] as i128,
                    shape[axis] as i128 + 1,
                    1i128 << 32,
                    (1i128 << 63) - 1,
                ] {
                    let mut c = vec![0; shape.len()];
                    c[axis] = n;
                    cases.push((c, None));
                }
                for bit in [0usize, 63] {
                    for state in ['x', 'z'] {
                        cases.push((vec![0; shape.len()], Some((axis, bit, state))));
                    }
                }
            }
            for (coordinates, unknown) in cases {
                let mut decoded: Vec<_> = coordinates.iter().copied().map(Some).collect();
                for (axis, &n) in coordinates.iter().enumerate() {
                    let index =
                        if let Some((a, bit, state)) = unknown.filter(|(a, _, _)| *a == axis) {
                            let mut digits = vec!['0'; 64];
                            digits[63 - bit] = state;
                            decoded[a] = if config.use_4state {
                                None
                            } else {
                                Some(if state == 'x' {
                                    0
                                } else if bit == 63 {
                                    -(1i128 << 63)
                                } else {
                                    1i128 << bit
                                })
                            };
                            Value::from_str(&format!("64'b{}", digits.iter().collect::<String>()))
                                .unwrap()
                        } else {
                            Value::new(n as u64, 64, true)
                        };
                    sim.set(&format!("idx_{axis}"), index);
                }
                for depth in 1..=shape.len() {
                    let prefix: Option<Vec<_>> = decoded[..depth].iter().copied().collect();
                    let at: Vec<_> = (0..full)
                        .filter(|&bit| {
                            let Some(prefix) = &prefix else {
                                return false;
                            };
                            let mut coordinate = vec![0; shape.len()];
                            let mut left = bit;
                            for axis in (0..shape.len()).rev() {
                                coordinate[axis] = left % shape[axis];
                                left /= shape[axis];
                            }
                            coordinate.iter().zip(prefix).all(|(&a, &b)| a as i128 == b)
                        })
                        .collect();
                    let patch = &patches[depth - 1];
                    let rd: Vec<_> = if at.is_empty() {
                        vec![if config.use_4state { 'x' } else { '0' }; patch.len()]
                    } else {
                        at.iter().map(|&b| data[b]).collect()
                    };
                    let mut wr = data.clone();
                    for (&dst, &v) in at.iter().zip(patch) {
                        wr[dst] = v;
                    }
                    assert_eq!(
                        sim.get(&format!("rd_{depth}")),
                        Some(parse(&rd)),
                        "read shape={shape:?} depth={depth} coordinates={coordinates:?} unknown={unknown:?} config={config:?}"
                    );
                    assert_eq!(
                        sim.get(&format!("wr_{depth}")),
                        Some(parse(&wr)),
                        "write shape={shape:?} depth={depth} coordinates={coordinates:?} unknown={unknown:?} config={config:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn packed_multidimensional_ranges_stay_in_the_selected_row() {
    compare_packed_multidimensional_ranges(&[(&[3, 8], 1), (&[2, 3, 16], 2), (&[2, 3, 65], 2)]);
}

#[test]
fn packed_multidimensional_ranges_preserve_element_strides() {
    compare_packed_multidimensional_ranges(&[
        (&[3, 4, 3], 1),
        (&[2, 3, 4, 5], 2),
        (&[2, 3, 65], 1),
        (&[2, 3, 2, 17], 1),
    ]);
}

fn compare_packed_multidimensional_ranges(layouts: &[(&[usize], usize)]) {
    for &(shape, selected_axis) in layouts {
        let full: usize = shape.iter().product();
        let row = shape[selected_axis];
        let coordinate_shape = &shape[..=selected_axis];
        let stride: usize = shape[selected_axis + 1..].iter().product();
        let dims = shape
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let windows: &[usize] = if stride == 1 { &[3, 8, 65] } else { &[1, 2, 3] };
        for window in windows.iter().copied().filter(|&w| w <= row) {
            let patch_bits = window * stride;
            for form in ["plus", "minus", "step", "colon"] {
                let prefix = (0..selected_axis)
                    .map(|i| format!("[idx_{i}]"))
                    .collect::<String>();
                let col = format!("idx_{selected_axis}");
                let range = match form {
                    "plus" => format!("[{col}+:{window}]"),
                    "minus" => format!("[{col}-:{window}]"),
                    "step" => format!("[{col} step {window}]"),
                    _ => format!("[{}:0]", window - 1),
                };
                let ports = (0..=selected_axis)
                    .map(|i| format!("idx_{i}: input signed logic<64>, "))
                    .collect::<String>();
                let source = format!(
                    "module Top(data:input logic<{dims}>,patch:input logic<{patch_bits}>,alias_en:input logic,rd:output logic<{patch_bits}>,wr:output logic<{full}>,{ports}) {{var q:logic<{dims}>;assign rd = data{prefix}{range};always_comb {{q = data;q{prefix}{range} = patch;if alias_en {{q{prefix}[0+:{window}] = q;}}}}assign wr = q;}}"
                );
                for config in Config::all() {
                    let mut sim = Simulator::new(analyze(&source, &config), None);
                    let parse = |bits: &[char]| {
                        let digits: String = bits.iter().rev().collect();
                        Value::from_str(&format!("{}'b{digits}", bits.len())).unwrap()
                    };
                    let data: Vec<_> = (0..full)
                        .map(|b| {
                            if config.use_4state {
                                ['0', '1', 'x', 'z'][(b + b / 64) % 4]
                            } else {
                                ['0', '1'][(b + b / 64) % 2]
                            }
                        })
                        .collect();
                    let patch: Vec<_> = (0..patch_bits)
                        .map(|b| {
                            if config.use_4state {
                                ['1', 'z', '0', 'x'][b % 4]
                            } else {
                                ['1', '0'][b % 2]
                            }
                        })
                        .collect();
                    sim.set("data", parse(&data));
                    sim.set("patch", parse(&patch));
                    let mut cases = vec![
                        (vec![0i128; coordinate_shape.len()], None),
                        (
                            coordinate_shape.iter().map(|&n| n as i128 - 1).collect(),
                            None,
                        ),
                    ];
                    for axis in 0..coordinate_shape.len() {
                        for n in [
                            -1,
                            shape[axis] as i128,
                            shape[axis] as i128 + 1,
                            1i128 << 32,
                            (1i128 << 63) - 1,
                        ] {
                            let mut c = vec![0; coordinate_shape.len()];
                            c[axis] = n;
                            cases.push((c, None));
                        }
                        for bit in [0usize, 63] {
                            for state in ['x', 'z'] {
                                cases.push((
                                    vec![0; coordinate_shape.len()],
                                    Some((axis, bit, state)),
                                ));
                            }
                        }
                    }
                    for alias in [false, true] {
                        sim.set("alias_en", Value::new(u64::from(alias), 1, false));
                        for (c, unknown) in &cases {
                            let mut decoded: Vec<_> = c.iter().copied().map(Some).collect();
                            for (axis, &n) in c.iter().enumerate() {
                                let index = if let Some((a, bit, state)) =
                                    unknown.filter(|(a, _, _)| *a == axis)
                                {
                                    let mut bits = vec!['0'; 64];
                                    bits[63 - bit] = state;
                                    decoded[a] = if config.use_4state {
                                        None
                                    } else {
                                        Some(if state == 'x' {
                                            0
                                        } else if bit == 63 {
                                            -(1i128 << 63)
                                        } else {
                                            1i128 << bit
                                        })
                                    };
                                    Value::from_str(&format!(
                                        "64'b{}",
                                        bits.iter().collect::<String>()
                                    ))
                                    .unwrap()
                                } else {
                                    Value::new(n as u64, 64, true)
                                };
                                sim.set(&format!("idx_{axis}"), index);
                            }
                            let prefix: Option<Vec<_>> =
                                decoded[..decoded.len() - 1].iter().copied().collect();
                            let at: Vec<_> = (0..full)
                                .filter(|&bit| {
                                    let Some(prefix) = &prefix else {
                                        return false;
                                    };
                                    let mut coords = vec![0; shape.len()];
                                    let mut rest = bit;
                                    for axis in (0..shape.len()).rev() {
                                        coords[axis] = rest % shape[axis];
                                        rest /= shape[axis];
                                    }
                                    coords.iter().zip(prefix).all(|(&a, &b)| a as i128 == b)
                                })
                                .collect();
                            let start = if form == "colon" {
                                Some(0)
                            } else {
                                decoded.last().copied().flatten().map(|n| match form {
                                    "minus" => n - window as i128 + 1,
                                    "step" => n * window as i128,
                                    _ => n,
                                })
                            };
                            let start = start.map(|n| n * stride as i128);
                            let rd: Vec<_> = (0..patch_bits)
                                .map(|b| {
                                    start
                                        .and_then(|n| usize::try_from(n + b as i128).ok())
                                        .and_then(|b| at.get(b))
                                        .map(|&b| data[b])
                                        .unwrap_or(if config.use_4state { 'x' } else { '0' })
                                })
                                .collect();
                            let mut wr = data.clone();
                            if let Some(start) = start {
                                for (b, &v) in patch.iter().enumerate() {
                                    if let Ok(b) = usize::try_from(start + b as i128)
                                        && let Some(&dst) = at.get(b)
                                    {
                                        wr[dst] = v;
                                    }
                                }
                            }
                            if alias {
                                let copy = wr[..patch_bits].to_vec();
                                for (&dst, v) in at.iter().take(patch_bits).zip(copy) {
                                    wr[dst] = v;
                                }
                            }
                            assert_eq!(
                                sim.get("rd"),
                                Some(parse(&rd)),
                                "read shape={shape:?} form={form} window={window} coordinate={c:?} unknown={unknown:?} config={config:?}"
                            );
                            assert_eq!(
                                sim.get("wr"),
                                Some(parse(&wr)),
                                "write shape={shape:?} form={form} window={window} coordinate={c:?} unknown={unknown:?} alias={alias} config={config:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn packed_range_temporary_storage_does_not_scale_with_row_width() {
    for use_4state in [false, true] {
        let config = Config {
            use_4state,
            ..Config::default()
        };
        let storage = |row: usize| {
            let code = format!(
                "module Top(data: input logic<3, {row}>, row: input logic<2>, index: input signed logic<32>, result: output logic<8>) {{ assign result = data[row][index+:8]; }}"
            );
            analyze(&code, &config).comb_values.len()
        };
        let growth = storage(65536) - storage(4096);
        let declared = (65536 - 4096) * 3 / 8 * if use_4state { 2 } else { 1 };
        assert!(
            growth <= declared + 512,
            "row-sized temporary: growth={growth} declared={declared}"
        );
    }
}
