use super::*;
use crate::ir::Component;
use crate::value::Value;

fn evaluate_contract_constant(code: &str) -> Value {
    let metadata = Metadata::create_default("prj").unwrap();
    let analyzer = Analyzer::new(&metadata);
    analyzer.clear();
    let parser = Parser::parse(code, &"").unwrap();
    let mut context = Context::default();
    let mut ir = Ir::default();
    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.extend(Analyzer::analyze_post_pass2(&ir));
    assert!(errors.is_empty(), "{code}\n{errors:#?}");
    let Component::Module(module) = &ir.components[0] else {
        panic!("expected module");
    };
    module
        .variables
        .values()
        .find(|variable| variable.path.to_string() == "RESULT")
        .unwrap()
        .get_value(&[])
        .unwrap()
        .clone()
}

#[test]
fn nonlinear_contract_products_and_signed_division() {
    for width in [4, 63, 64, 65, 128] {
        let mask = u128::MAX >> (128 - width);
        let sign = 1u128 << (width - 1);
        for signed in [false, true] {
            let modifier = if signed { "signed " } else { "" };
            let literal = if signed { "s" } else { "" };
            for (a, b, op, expected) in [
                (mask, mask, "*", 1),
                (sign, mask, "/", if signed { sign } else { 0 }),
                (sign, mask, "%", if signed { 0 } else { sign }),
                (
                    mask - 6,
                    3,
                    "/",
                    if signed { mask - 1 } else { (mask - 6) / 3 },
                ),
                (mask - 6, 3, "%", if signed { mask } else { (mask - 6) % 3 }),
            ] {
                let code = format!(
                    r#"
module Top {{
    function probe(a: input {modifier}logic<{width}>, b: input {modifier}logic<{width}>) -> {modifier}logic<{width}> {{ return a {op} b; }}
    const RESULT: {modifier}logic<{width}> = probe({width}'{literal}h{a:x}, {width}'{literal}h{b:x});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (expected, 0),
                    "{code}"
                );
            }
        }
    }
}

#[test]
fn nonlinear_contract_zero_divisor_and_unknowns() {
    for width in [4, 64, 65, 128] {
        let mask = u128::MAX >> (128 - width);
        for op in ["*", "/", "%", "**"] {
            for unknown in ['x', 'z'] {
                let code = format!(
                    r#"
module Top {{
    function probe(a: input logic<{width}>, b: input signed logic<4>) -> logic<{width}> {{ return a {op} b; }}
    const RESULT: logic<{width}> = probe({width}'b1, 4'sb{unknown}001);
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (0, mask),
                    "{code}"
                );
            }
        }
        for op in ["/", "%"] {
            let code = format!(
                r#"
module Top {{
    function probe(a: input logic<{width}>, b: input logic<{width}>) -> logic<{width}> {{ return a {op} b; }}
    const RESULT: logic<{width}> = probe({width}'b1, {width}'b0);
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                (0, mask),
                "{code}"
            );
        }
    }
}

#[test]
fn nonlinear_contract_negative_powers() {
    for width in [4, 64, 65, 128] {
        let mask = u128::MAX >> (128 - width);
        for (base, exponent, expected, unknown) in [
            (0, 0, 1, false),
            (0, 15, 0, true),
            (1, 15, 1, false),
            (mask, 15, mask, false),
            (mask, 14, 1, false),
            (2, 15, 0, false),
        ] {
            let code = format!(
                r#"
module Top {{
    function probe(a: input signed logic<{width}>, b: input signed logic<4>) -> signed logic<{width}> {{ return a ** b; }}
    const RESULT: signed logic<{width}> = probe({width}'sh{base:x}, 4'sh{exponent:x});
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                (expected, if unknown { mask } else { 0 }),
                "{code}"
            );
        }
    }
    for (width, expected) in [(4, 15), (8, 0)] {
        let code = format!(
            r#"
module Top {{
    function probe(a: input signed logic<4>, b: input signed logic<4>) -> logic<{width}> {{ return (a ** b) + {width}'b0; }}
    const RESULT: logic<{width}> = probe(4'shf, 4'shf);
}}
"#
        );
        let result = evaluate_contract_constant(&code);
        assert_eq!(
            (result.payload_u128(), result.mask_xz_u128()),
            (expected, 0),
            "{code}"
        );
    }
}

#[test]
fn nonlinear_contract_wide_power_exponents() {
    for width in [4, 64, 65, 128] {
        let mask = u128::MAX >> (128 - width);
        let code = format!(
            r#"
module Top {{
    function probe(a: input logic<{width}>, b: input logic<128>) -> logic<{width}> {{ return a ** b; }}
    const RESULT: logic<{width}> = probe({width}'h{mask:x}, 128'h10000000000000000);
}}
"#
        );
        let result = evaluate_contract_constant(&code);
        assert_eq!(
            (result.payload_u128(), result.mask_xz_u128()),
            (1, 0),
            "{code}"
        );
    }
}

#[test]
fn context_contract_type_cast_signed_parent() {
    let code = r#"
module Top {
    type Signed = signed logic<4>;
    function probe(a: input logic<4>) -> logic<128> { return (a as Signed) + 128'sb0; }
    const RESULT: logic<128> = probe(4'hf);
}
"#;
    let result = evaluate_contract_constant(code);
    assert_eq!(
        (result.payload_u128(), result.mask_xz_u128()),
        (u128::MAX, 0)
    );
}

#[test]
fn context_contract_comparison_and_self_determined_operands() {
    for width in [4, 63, 64, 65, 127] {
        let mask = u128::MAX >> (128 - width);
        for (expression, expected) in [
            ("(a + b) <: b", 1),
            ("|(a + b)", 0),
            ("~|(a + b)", 1),
            ("b << (a + b)", 1),
            ("b >> (a + b)", 1),
        ] {
            let code = format!(
                r#"
module Top {{
    function probe(a: input logic<{width}>, b: input logic<{width}>) -> logic<128> {{ return ({expression}) + 128'b0; }}
    const RESULT: logic<128> = probe({width}'h{mask:x}, {width}'b1);
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                (expected, 0),
                "{code}"
            );
        }
        let code = format!(
            r#"
module Top {{
    function probe(a: input signed logic<{width}>, b: input signed logic<{width}>) -> logic<128> {{ return (a <: b) + 128'b0; }}
    const RESULT: logic<128> = probe({width}'sh{mask:x}, {width}'sh0);
}}
"#
        );
        let result = evaluate_contract_constant(&code);
        assert_eq!(
            (result.payload_u128(), result.mask_xz_u128()),
            (1, 0),
            "{code}"
        );
    }
}

#[test]
fn context_contract_numeric_cast_carry_boundary() {
    for width in [4, 63, 64, 65, 127] {
        let mask = u128::MAX >> (128 - width);
        for target in [width, width + 1] {
            let code = format!(
                r#"
module Top {{
    function probe(a: input logic<{width}>, b: input logic<{width}>) -> logic<128> {{ return (a + b) as {target}; }}
    const RESULT: logic<128> = probe({width}'h{mask:x}, {width}'b1);
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            let expected = if target == width { 0 } else { 1u128 << width };
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                (expected, 0),
                "{code}"
            );
        }
    }
}

#[test]
fn context_contract_numeric_cast_preserves_type_sign() {
    for width in [4, 63, 64, 65, 127] {
        let sign = 1u128 << (width - 1);
        let mask = u128::MAX >> (128 - width);
        for signed in [false, true] {
            let modifier = if signed { "signed " } else { "" };
            let code = format!(
                r#"
module Top {{
    function probe() -> logic<128> {{
        var a: {modifier}logic<{width}>;
        a = {width}'h{sign:x};
        return a as 128;
    }}
    const RESULT: logic<128> = probe();
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            let expected = if signed { sign | !mask } else { sign };
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                (expected, 0),
                "{code}"
            );
        }
    }
}

#[test]
fn arithmetic_contract_modular_boundaries() {
    for width in [1, 4, 63, 64, 65, 128] {
        let mask = u128::MAX >> (128 - width);
        let sign = 1u128 << (width - 1);
        for signed in [false, true] {
            let modifier = if signed { "signed " } else { "" };
            let literal = if signed { "s" } else { "" };
            for (a, b, add, sub) in [
                (mask, 1, 0, mask ^ 1),
                (0, 1, 1, mask),
                (sign, sign, 0, 0),
                (1, 1, 2 & mask, 0),
            ] {
                for (op, expected) in [("+", add), ("-", sub)] {
                    let code = format!(
                        r#"
module Top {{
    function probe(a: input {modifier}logic<{width}>, b: input {modifier}logic<{width}>) -> {modifier}logic<{width}> {{ return a {op} b; }}
    const RESULT: {modifier}logic<{width}> = probe({width}'{literal}h{a:x}, {width}'{literal}h{b:x});
}}
"#
                    );
                    let result = evaluate_contract_constant(&code);
                    assert_eq!(
                        (result.payload_u128(), result.mask_xz_u128()),
                        (expected, 0),
                        "{code}"
                    );
                    assert_eq!(result.width(), width);
                    assert_eq!(result.signed(), signed);
                }
            }
            for (value, expected) in [(0, 0), (1, mask), (sign, sign)] {
                let code = format!(
                    r#"
module Top {{
    function probe(a: input {modifier}logic<{width}>) -> {modifier}logic<{width}> {{ return -a; }}
    const RESULT: {modifier}logic<{width}> = probe({width}'{literal}h{value:x});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (expected, 0),
                    "{code}"
                );
                assert_eq!(result.width(), width);
                assert_eq!(result.signed(), signed);
            }
        }
    }
}

#[test]
fn arithmetic_contract_four_state_and_unary_plus() {
    for width in [1, 4, 64, 65, 128] {
        let mask = u128::MAX >> (128 - width);
        let sign = 1u128 << (width - 1);
        for signed in [false, true] {
            let modifier = if signed { "signed " } else { "" };
            let literal = if signed { "s" } else { "" };
            for unknown in ['x', 'z'] {
                let digits = format!("{unknown}{}", "1".repeat(width - 1));
                for (expression, identity) in [
                    (format!("a + {width}'{literal}b0"), false),
                    (format!("a - {width}'{literal}b0"), false),
                    ("-a".to_owned(), false),
                    ("+a".to_owned(), true),
                ] {
                    let code = format!(
                        r#"
module Top {{
    function probe(a: input {modifier}logic<{width}>) -> {modifier}logic<{width}> {{ return {expression}; }}
    const RESULT: {modifier}logic<{width}> = probe({width}'{literal}b{digits});
}}
"#
                    );
                    let result = evaluate_contract_constant(&code);
                    let expected = if identity {
                        ((sign - 1) | if unknown == 'z' { sign } else { 0 }, sign)
                    } else {
                        (0, mask)
                    };
                    assert_eq!(
                        (result.payload_u128(), result.mask_xz_u128()),
                        expected,
                        "{code}"
                    );
                    assert_eq!(result.width(), width);
                    assert_eq!(result.signed(), signed);
                }
            }
        }
    }
}

#[test]
fn arithmetic_contract_context_width_and_signedness() {
    for (a_width, b_width, width) in [(4, 8, 16), (63, 64, 65), (64, 65, 128)] {
        let mask = u128::MAX >> (128 - width);
        let a_mask = u128::MAX >> (128 - a_width);
        for a_signed in [false, true] {
            for b_signed in [false, true] {
                let am = if a_signed { "signed " } else { "" };
                let bm = if b_signed { "signed " } else { "" };
                let al = if a_signed { "s" } else { "" };
                let bl = if b_signed { "s" } else { "" };
                for (op, expected) in [
                    (
                        "+",
                        if a_signed && b_signed {
                            0
                        } else {
                            1u128 << a_width
                        },
                    ),
                    (
                        "-",
                        if a_signed && b_signed {
                            mask - 1
                        } else {
                            a_mask - 1
                        },
                    ),
                ] {
                    let code = format!(
                        r#"
module Top {{
    function probe(a: input {am}logic<{a_width}>, b: input {bm}logic<{b_width}>) -> logic<{width}> {{ return a {op} b; }}
    const RESULT: logic<{width}> = probe({a_width}'{al}h{a_mask:x}, {b_width}'{bl}b1);
}}
"#
                    );
                    let result = evaluate_contract_constant(&code);
                    assert_eq!(
                        (result.payload_u128(), result.mask_xz_u128()),
                        (expected, 0),
                        "{code}"
                    );
                    assert_eq!(result.width(), width);
                }
            }
        }
    }
}

#[test]
fn conditional_contract_four_state_boundaries() {
    for width in [1, 4, 64, 65, 128] {
        for condition in ['0', '1', 'x', 'z'] {
            for (a, b) in [
                ("1100", "1010"),
                ("z110", "z100"),
                ("z", "z"),
                ("x", "z"),
                ("0", "1"),
            ] {
                let a = a.repeat(width).chars().take(width).collect::<String>();
                let b = b.repeat(width).chars().take(width).collect::<String>();
                let code = format!(
                    r#"
module Top {{
    function probe(c: input logic) -> logic<{width}> {{
        return if c ? {width}'b{a} : {width}'b{b};
    }}
    const RESULT: logic<{width}> = probe(1'b{condition});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                let mut payload = 0u128;
                let mut mask = 0u128;
                for (position, (a, b)) in a.chars().rev().zip(b.chars().rev()).enumerate() {
                    let bit = match condition {
                        '0' => b,
                        '1' => a,
                        _ => {
                            if a == b {
                                a
                            } else {
                                'x'
                            }
                        }
                    };
                    if matches!(bit, '1' | 'z') {
                        payload |= 1u128 << position;
                    }
                    if matches!(bit, 'x' | 'z') {
                        mask |= 1u128 << position;
                    }
                }
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (payload, mask),
                    "{code}"
                );
                assert_eq!(result.width(), width);
                assert!(!result.signed());
            }
        }
    }
}

#[test]
fn conditional_contract_width_and_signedness() {
    for (a_width, b_width, width) in [(4, 8, 16), (63, 64, 65), (64, 65, 128), (65, 64, 128)] {
        for a_signed in [false, true] {
            for b_signed in [false, true] {
                for force_unsigned in [false, true] {
                    for condition in ['0', '1', 'x', 'z'] {
                        let a_modifier = if a_signed { "signed " } else { "" };
                        let b_modifier = if b_signed { "signed " } else { "" };
                        let a_literal = if a_signed { "s" } else { "" };
                        let b_literal = if b_signed { "s" } else { "" };
                        let suffix = if force_unsigned {
                            format!(" | {width}'b0")
                        } else {
                            String::new()
                        };
                        let code = format!(
                            r#"
module Top {{
    function probe(c: input logic, a: input {a_modifier}logic<{a_width}>, b: input {b_modifier}logic<{b_width}>) -> logic<{width}> {{
        return (if c ? a : b){suffix};
    }}
    const RESULT: logic<{width}> = probe(1'b{condition}, {a_width}'{a_literal}b{ones}, {b_width}'{b_literal}b0);
}}
"#,
                            ones = "1".repeat(a_width)
                        );
                        let result = evaluate_contract_constant(&code);
                        let extended_width = if a_signed && b_signed && !force_unsigned {
                            width
                        } else {
                            a_width
                        };
                        let a = u128::MAX >> (128 - extended_width);
                        let expected = match condition {
                            '0' => (0, 0),
                            '1' => (a, 0),
                            _ => (0, a),
                        };
                        assert_eq!(
                            (result.payload_u128(), result.mask_xz_u128()),
                            expected,
                            "{code}"
                        );
                        assert_eq!(result.width(), width);
                    }
                }
            }
        }
    }
}

#[test]
fn conditional_contract_statement_unknown_takes_false_side() {
    for condition in ['0', '1', 'x', 'z'] {
        let code = format!(
            r#"
module Top {{
    function probe(c: input logic) -> logic<4> {{
        if c {{ return 4'b1100; }}
        return 4'b1010;
    }}
    const RESULT: logic<4> = probe(1'b{condition});
}}
"#
        );
        let result = evaluate_contract_constant(&code);
        let expected = if condition == '1' { 0b1100 } else { 0b1010 };
        assert_eq!(
            (result.payload_u128(), result.mask_xz_u128()),
            (expected, 0),
            "{code}"
        );
    }
}

#[test]
fn conditional_contract_unknown_condition_merges_arms() {
    for condition in ['0', '1', 'x', 'z'] {
        for direct in [false, true] {
            let expression = if direct {
                format!("if 1'b{condition} ? 4'b1100 : 4'b1010")
            } else {
                format!("probe(1'b{condition})")
            };
            let code = format!(
                r#"
module Top {{
    function probe(c: input logic) -> logic<4> {{
        return if c ? 4'b1100 : 4'b1010;
    }}
    const RESULT: logic<4> = {expression};
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            let expected = match condition {
                '0' => (0b1010, 0),
                '1' => (0b1100, 0),
                _ => (0b1000, 0b0110),
            };
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                expected,
                "{code}"
            );
        }
    }
}

#[test]
fn gates_contract_bitwise_context_width_and_signedness() {
    for (x_width, y_width, width) in [(4, 8, 16), (63, 64, 65), (64, 65, 128)] {
        for x_signed in [false, true] {
            for y_signed in [false, true] {
                for op in ["&", "|", "^", "~^"] {
                    let x_modifier = if x_signed { "signed " } else { "" };
                    let y_modifier = if y_signed { "signed " } else { "" };
                    let x_literal = if x_signed { "s" } else { "" };
                    let y_literal = if y_signed { "s" } else { "" };
                    let code = format!(
                        r#"
module Top {{
    function probe(a: input {x_modifier}logic<{x_width}>, b: input {y_modifier}logic<{y_width}>) -> logic<{width}> {{
        return a {op} b;
    }}
    const RESULT: logic<{width}> = probe({x_width}'{x_literal}b{ones}, {y_width}'{y_literal}b1);
}}
"#,
                        ones = "1".repeat(x_width)
                    );
                    let result = evaluate_contract_constant(&code);
                    let mask = u128::MAX >> (128 - width);
                    let a = if x_signed && y_signed {
                        mask
                    } else {
                        u128::MAX >> (128 - x_width)
                    };
                    let expected = match op {
                        "&" => a & 1,
                        "|" => a | 1,
                        "^" => a ^ 1,
                        _ => (a ^ 1) ^ mask,
                    };
                    assert_eq!(
                        (result.payload_u128(), result.mask_xz_u128()),
                        (expected, 0),
                        "{code}"
                    );
                    assert_eq!(result.width(), width);
                }
            }
        }
    }
}

#[test]
fn gates_contract_bitwise_truth_tables() {
    let symbols = ['0', '1', 'x', 'z'];
    let tables = [
        (
            "&",
            [[0, 0, 0, 0], [0, 1, 2, 2], [0, 2, 2, 2], [0, 2, 2, 2]],
        ),
        (
            "|",
            [[0, 1, 2, 2], [1, 1, 1, 1], [2, 1, 2, 2], [2, 1, 2, 2]],
        ),
        (
            "^",
            [[0, 1, 2, 2], [1, 0, 2, 2], [2, 2, 2, 2], [2, 2, 2, 2]],
        ),
        (
            "~^",
            [[1, 0, 2, 2], [0, 1, 2, 2], [2, 2, 2, 2], [2, 2, 2, 2]],
        ),
    ];
    for width in [1, 4, 64, 65, 128] {
        let mask = u128::MAX >> (128 - width);
        for (op, table) in tables {
            for (i, a) in symbols.into_iter().enumerate() {
                for (j, b) in symbols.into_iter().enumerate() {
                    let code = format!(
                        r#"
module Top {{
    function probe(a: input logic<{width}>, b: input logic<{width}>) -> logic<{width}> {{
        return a {op} b;
    }}
    const RESULT: logic<{width}> = probe({width}'b{a}, {width}'b{b});
}}
"#,
                        a = a.to_string().repeat(width),
                        b = b.to_string().repeat(width)
                    );
                    let result = evaluate_contract_constant(&code);
                    let expected = match table[i][j] {
                        0 => (0, 0),
                        1 => (mask, 0),
                        _ => (0, mask),
                    };
                    assert_eq!(
                        (result.payload_u128(), result.mask_xz_u128()),
                        expected,
                        "{code}"
                    );
                    assert_eq!(result.width(), width);
                }
            }
        }
        for (a, output) in [('0', '1'), ('1', '0'), ('x', 'x'), ('z', 'x')] {
            let code = format!(
                r#"
module Top {{
    function probe(a: input logic<{width}>) -> logic<{width}> {{ return ~a; }}
    const RESULT: logic<{width}> = probe({width}'b{a});
}}
"#,
                a = a.to_string().repeat(width)
            );
            let result = evaluate_contract_constant(&code);
            let expected = match output {
                '0' => (0, 0),
                '1' => (mask, 0),
                _ => (0, mask),
            };
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                expected,
                "{code}"
            );
            assert_eq!(result.width(), width);
        }
    }
}

#[test]
fn gates_contract_logical_truth_tables() {
    let symbols = ['0', '1', 'x', 'z'];
    for (op, table) in [
        (
            "&&",
            [[0, 0, 0, 0], [0, 1, 2, 2], [0, 2, 2, 2], [0, 2, 2, 2]],
        ),
        (
            "||",
            [[0, 1, 2, 2], [1, 1, 1, 1], [2, 1, 2, 2], [2, 1, 2, 2]],
        ),
    ] {
        for (i, a) in symbols.into_iter().enumerate() {
            for (j, b) in symbols.into_iter().enumerate() {
                let code = format!(
                    r#"
module Top {{
    function probe(a: input logic, b: input logic) -> logic {{ return a {op} b; }}
    const RESULT: logic = probe(1'b{a}, 1'b{b});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                let expected = match table[i][j] {
                    0 => (0, 0),
                    1 => (1, 0),
                    _ => (0, 1),
                };
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    expected,
                    "{code}"
                );
                assert_eq!(result.width(), 1);
                assert!(!result.signed());
            }
        }
    }
    for (a, expected) in [('0', (1, 0)), ('1', (0, 0)), ('x', (0, 1)), ('z', (0, 1))] {
        let code = format!(
            r#"
module Top {{
    function probe(a: input logic) -> logic {{ return !a; }}
    const RESULT: logic = probe(1'b{a});
}}
"#
        );
        let result = evaluate_contract_constant(&code);
        assert_eq!(
            (result.payload_u128(), result.mask_xz_u128()),
            expected,
            "{code}"
        );
        assert_eq!(result.width(), 1);
        assert!(!result.signed());
    }
}

#[test]
fn gates_contract_reduction_dominance_and_parity() {
    for width in [1, 4, 64, 65, 128] {
        for (digits, and, or, xor) in [
            ("0".repeat(width), 0, 0, 0),
            ("1".repeat(width), 1, 1, i32::from(width % 2 == 1)),
            ("x".repeat(width), 2, 2, 2),
            ("z".repeat(width), 2, 2, 2),
            (
                format!("{}0", "x".repeat(width - 1)),
                0,
                if width == 1 { 0 } else { 2 },
                if width == 1 { 0 } else { 2 },
            ),
            (
                format!("{}1", "z".repeat(width - 1)),
                if width == 1 { 1 } else { 2 },
                1,
                if width == 1 { 1 } else { 2 },
            ),
        ] {
            for (op, base, inverted) in [
                ("&", and, false),
                ("~&", and, true),
                ("|", or, false),
                ("~|", or, true),
                ("^", xor, false),
                ("~^", xor, true),
            ] {
                let code = format!(
                    r#"
module Top {{
    function probe(a: input logic<{width}>) -> logic {{ return {op}a; }}
    const RESULT: logic = probe({width}'b{digits});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                let expected = if base == 2 {
                    (0, 1)
                } else {
                    (u128::from((base == 1) != inverted), 0)
                };
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    expected,
                    "{code}"
                );
                assert_eq!(result.width(), 1);
                assert!(!result.signed());
            }
        }
    }
}

#[test]
fn comparison_contract_relational_unknowns() {
    for width in [1, 4, 64, 65, 128] {
        for op in ["<:", "<=", ">:", ">="] {
            for (a, b) in [("1", "x"), ("z", "0"), ("1x", "00")] {
                if width == 1 && a == "1x" {
                    continue;
                }
                let code = format!(
                    r#"
module Top {{
    function probe(a: input logic<{width}>, b: input logic<{width}>) -> logic {{
        return a {op} b;
    }}
    const RESULT: logic = probe({width}'b{a}, {width}'b{b});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (0, 1),
                    "{code}"
                );
                assert_eq!(result.width(), 1, "{code}");
                assert!(!result.signed(), "{code}");
            }
        }
    }
}

#[test]
fn comparison_contract_ambiguous_equality_and_known_mismatches() {
    for width in [1, 4, 64, 65, 128] {
        for (op, wildcard, negated) in [
            ("==", false, false),
            ("!=", false, true),
            ("==?", true, false),
            ("!=?", true, true),
        ] {
            for (a, b, known_mismatch) in [
                ("1", "x", false),
                ("x", "1", false),
                ("1", "z", false),
                ("z", "1", false),
                ("x", "x", false),
                ("z", "z", false),
                ("1x", "0x", true),
            ] {
                if known_mismatch && width == 1 {
                    continue;
                }
                let code = format!(
                    r#"
module Top {{
    function probe(a: input logic<{width}>, b: input logic<{width}>) -> logic {{
        return a {op} b;
    }}
    const RESULT: logic = probe({width}'b{a}, {width}'b{b});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                let unknown = !known_mismatch && !(wildcard && matches!(b, "x" | "z"));
                let equal = !known_mismatch;
                let payload = u128::from(!unknown && (equal != negated));
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (payload, u128::from(unknown)),
                    "{code}"
                );
                assert_eq!(result.width(), 1, "{code}");
                assert!(!result.signed(), "{code}");
            }
        }
    }
}

#[test]
fn comparison_contract_mixed_width_and_signedness() {
    for (x_width, y_width) in [(4, 8), (63, 64), (64, 65), (65, 64), (65, 128)] {
        for x_signed in [false, true] {
            for y_signed in [false, true] {
                for (op, expected_signed, expected_unsigned) in [
                    ("==", false, false),
                    ("!=", true, true),
                    ("==?", false, false),
                    ("!=?", true, true),
                    ("<:", true, false),
                    ("<=", true, false),
                    (">:", false, true),
                    (">=", false, true),
                ] {
                    let x_modifier = if x_signed { "signed " } else { "" };
                    let y_modifier = if y_signed { "signed " } else { "" };
                    let x_literal = if x_signed { "s" } else { "" };
                    let y_literal = if y_signed { "s" } else { "" };
                    let code = format!(
                        r#"
module Top {{
    function probe(a: input {x_modifier}logic<{x_width}>, b: input {y_modifier}logic<{y_width}>) -> logic {{
        return a {op} b;
    }}
    const RESULT: logic = probe({x_width}'{x_literal}b{ones}, {y_width}'{y_literal}b0);
}}
"#,
                        ones = "1".repeat(x_width)
                    );
                    let result = evaluate_contract_constant(&code);
                    let expected = if x_signed && y_signed {
                        expected_signed
                    } else {
                        expected_unsigned
                    };
                    assert_eq!(
                        (result.payload_u128(), result.mask_xz_u128()),
                        (u128::from(expected), 0),
                        "{code}"
                    );
                    assert_eq!(result.width(), 1, "{code}");
                    assert!(!result.signed(), "{code}");
                }
            }
        }
    }
}

#[test]
fn shift_contract_four_state_boundaries() {
    for (op, left, arithmetic) in [
        ("<<", true, false),
        (">>", false, false),
        ("<<<", true, true),
        (">>>", false, true),
    ] {
        for width in [4, 64, 65, 128] {
            for signed in [false, true] {
                if arithmetic && !signed {
                    continue;
                }
                for sign in ['0', '1', 'x', 'z'] {
                    for amount in [0, 1, width - 1, width, width + 1] {
                        let modifier = if signed { "signed " } else { "" };
                        let literal_sign = if signed { "s" } else { "" };
                        let digits = format!("{sign}{}1", "0".repeat(width - 2));
                        let code = format!(
                            r#"
module Top {{
    function probe(x: input {modifier}logic<{width}>, n: input u32) -> logic<{width}> {{
        return x {op} n;
    }}
    const RESULT: logic<{width}> = probe({width}'{literal_sign}b{digits}, {amount});
}}
"#
                        );
                        let result = evaluate_contract_constant(&code);
                        let mut payload = 0u128;
                        let mut mask_xz = 0u128;
                        // Derive each bit from its source; missing signed bits repeat the sign.
                        for position in 0..width {
                            let source = if left {
                                position.checked_sub(amount)
                            } else {
                                Some(position + amount)
                            };
                            let bit = if source.is_none_or(|source| source >= width) {
                                if !left && arithmetic && signed {
                                    sign
                                } else {
                                    '0'
                                }
                            } else if source == Some(width - 1) {
                                sign
                            } else if source == Some(0) {
                                '1'
                            } else {
                                '0'
                            };
                            if matches!(bit, '1' | 'z') {
                                payload |= 1u128 << position;
                            }
                            if matches!(bit, 'x' | 'z') {
                                mask_xz |= 1u128 << position;
                            }
                        }
                        assert_eq!(
                            (result.payload_u128(), result.mask_xz_u128()),
                            (payload, mask_xz),
                            "{code}"
                        );
                        assert_eq!(result.width(), width, "{code}");
                    }
                }
            }
        }
    }
}

#[test]
fn shift_contract_unknown_and_wide_amounts() {
    for width in [4, 64, 65, 128] {
        for op in ["<<", ">>", "<<<", ">>>"] {
            for (digits, unknown, amount) in [
                ("1", false, 1usize),
                ("10000000000000000000000000", false, usize::MAX),
                ("x", true, 0),
                ("z", true, 0),
            ] {
                let code = format!(
                    r#"
module Top {{
    function probe(n: input logic<128>) -> logic<{width}> {{
        var x: signed logic<{width}>;
        x = {width}'sb1;
        return x {op} n;
    }}
    const RESULT: logic<{width}> = probe(128'h{digits});
}}
"#
                );
                let result = evaluate_contract_constant(&code);
                let mask = u128::MAX >> (128 - width);
                let payload = if unknown || amount >= width {
                    0
                } else if op == "<<" || op == "<<<" {
                    1u128 << amount
                } else {
                    0
                };
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (payload, if unknown { mask } else { 0 }),
                    "{code}"
                );
                assert_eq!(result.width(), width);
            }
        }
    }
}

#[test]
fn selection_contract_dynamic_read_marks_missing_bits_unknown() {
    for source_width in [2, 63, 64, 65, 128] {
        for output_width in [1, 4] {
            for index in [0, source_width - 1, source_width, source_width + 1] {
                let select = if output_width == 1 {
                    "i".to_string()
                } else {
                    format!("i+:{output_width}")
                };
                let code = format!(
                    r#"
module Top {{
    function probe(i: input u32) -> logic<{output_width}> {{
        var x: logic<{source_width}>;
        x = {source_width}'b{digits};
        return x[{select}];
    }}
    const RESULT: logic<{output_width}> = probe({index});
}}
"#,
                    digits = "1".repeat(source_width)
                );
                let metadata = Metadata::create_default("prj").unwrap();
                let analyzer = Analyzer::new(&metadata);
                analyzer.clear();
                let parser = Parser::parse(&code, &"").unwrap();
                let mut context = Context::default();
                let mut ir = Ir::default();
                let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
                errors.extend(Analyzer::analyze_post_pass1());
                errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
                errors.extend(Analyzer::analyze_post_pass2(&ir));
                assert!(errors.is_empty(), "{code}\n{errors:#?}");
                let Component::Module(module) = &ir.components[0] else {
                    panic!("expected module");
                };
                let result = module
                    .variables
                    .values()
                    .find(|variable| variable.path.to_string() == "RESULT")
                    .unwrap()
                    .get_value(&[])
                    .unwrap();
                let valid = source_width.saturating_sub(index).min(output_width);
                let payload = (1u128 << valid) - 1;
                let mask_xz = ((1u128 << output_width) - 1) ^ payload;
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (payload, mask_xz),
                    "{code}\n{ir}"
                );
                assert_eq!(result.width(), output_width, "{code}");
            }
        }
    }
}

#[test]
fn resize_contract_function_return_preserves_four_state_bits() {
    for (source_width, width) in [
        (4, 8),
        (4, 64),
        (4, 65),
        (64, 65),
        (65, 128),
        (65, 4),
        (128, 64),
    ] {
        for signed in [false, true] {
            for sign in ['0', '1', 'x', 'z'] {
                let modifier = if signed { "signed " } else { "" };
                let literal_sign = if signed { "s" } else { "" };
                let digits = format!("{sign}{}1", "0".repeat(source_width - 2));
                let code = format!(
                    r#"
module Top {{
    function resize(x: input {modifier}logic<{source_width}>) -> {modifier}logic<{width}> {{
        return x;
    }}
    const RESULT: {modifier}logic<{width}> = resize({source_width}'{literal_sign}b{digits});
}}
"#
                );
                let metadata = Metadata::create_default("prj").unwrap();
                let analyzer = Analyzer::new(&metadata);
                analyzer.clear();
                let parser = Parser::parse(&code, &"").unwrap();
                let mut context = Context::default();
                let mut ir = Ir::default();
                let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
                errors.extend(Analyzer::analyze_post_pass1());
                errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
                errors.extend(Analyzer::analyze_post_pass2(&ir));
                assert!(errors.is_empty(), "{code}\n{errors:#?}");
                let Component::Module(module) = &ir.components[0] else {
                    panic!("expected module");
                };
                let result = module
                    .variables
                    .values()
                    .find(|variable| variable.path.to_string() == "RESULT")
                    .unwrap()
                    .get_value(&[])
                    .unwrap();
                let mask = u128::MAX >> (128 - width);
                let top = 1u128 << (source_width - 1);
                let extension = if signed && width > source_width {
                    mask ^ (u128::MAX >> (128 - source_width))
                } else {
                    0
                };
                let sign_plane = (top | extension) & mask;
                let payload = 1 | if matches!(sign, '1' | 'z') {
                    sign_plane
                } else {
                    0
                };
                let mask_xz = if matches!(sign, 'x' | 'z') {
                    sign_plane
                } else {
                    0
                };
                assert_eq!(
                    (result.payload_u128(), result.mask_xz_u128()),
                    (payload, mask_xz),
                    "{code}\n{ir}"
                );
                assert_eq!(result.width(), width, "{code}");
            }
        }
    }
}

#[test]
fn assignment_contract_dynamic_out_of_bounds_write_is_ignored() {
    for width in [2, 63, 64, 65] {
        for index in [width - 1, width, width + 1] {
            let code = format!(
                r#"
module Top {{
    function probe(i: input u32) -> logic {{
        var x: logic<{width}>;
        x = 0;
        x[i] = 1'b1;
        return x == 0;
    }}
    const RESULT: logic = probe({index});
}}
"#
            );
            symbol_table::clear();
            attribute_table::clear();
            doc_comment_table::clear();
            let metadata = Metadata::create_default("prj").unwrap();
            let parser = Parser::parse(&code, &"").unwrap();
            let analyzer = Analyzer::new(&metadata);
            let mut context = Context::default();
            let mut ir = Ir::default();
            let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
            errors.extend(Analyzer::analyze_post_pass1());
            errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
            errors.extend(Analyzer::analyze_post_pass2(&ir));
            assert!(errors.is_empty(), "{code}\n{errors:#?}");
            let Component::Module(module) = &ir.components[0] else {
                panic!("expected module");
            };
            let result = module
                .variables
                .values()
                .find(|variable| variable.path.to_string() == "RESULT")
                .unwrap();
            assert_eq!(
                result.get_value(&[]).unwrap().to_u64(),
                Some(u64::from(index >= width)),
                "width={width}, index={index}\n{ir}"
            );
        }
    }
}
