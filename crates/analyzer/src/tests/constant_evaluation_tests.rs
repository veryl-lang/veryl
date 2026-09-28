use super::*;
use crate::ir::{Component, Declaration, Statement};
use crate::value::Value;

#[test]
fn integer_system_functions_return_signed_integers() {
    for query in [
        "$bits(logic<5>)",
        "$size(logic<5>)",
        "$size(logic<5>, 1)",
        "$size(logic<3, 5>, 2)",
        "$clog2(17)",
    ] {
        let code = format!(
            r#"
module Top {{
    const SIGNED_VALUE: signed logic<8> = 8'hff;
    const UNSIGNED_VALUE: logic<8> = 8'hff;
    const SIGNED_FALSE: logic<32> = if 1'b0 ? {query} : SIGNED_VALUE;
    const SIGNED_TRUE: logic<32> = if 1'b1 ? SIGNED_VALUE : {query};
    const UNSIGNED_FALSE: logic<32> = if 1'b0 ? {query} : UNSIGNED_VALUE;
    const UNSIGNED_TRUE: logic<32> = if 1'b1 ? UNSIGNED_VALUE : {query};
    const UNSIGNED_OUTER_FALSE: logic<32> = (if 1'b0 ? {query} : SIGNED_VALUE) + 32'h0;
    const UNSIGNED_OUTER_TRUE: logic<32> = (if 1'b1 ? SIGNED_VALUE : {query}) + 32'h0;
    const UNSIGNED_SUB_CAST: logic<64> = ({query} - 32'd6) as 32;
    const REVERSE_SUB_CAST: logic<64> = (32'd4 - {query}) as 32;
    const SIGNED_SUB_CAST: logic<64> = ({query} - 6) as 32;
    const COMPARE_CAST: logic<32> = ({query} <: 6) as 1;
    const BITNOT_SUM_CAST: logic<64> = ({query} + ~32'sd5) as 32;
    const TERNARY_CAST: logic<64> = (if 1'b0 ? {query} : SIGNED_VALUE) as 32;
    const TERNARY_SHIFT: logic<32> = (if 1'b0 ? {query} : SIGNED_VALUE) >>> 1;
    const SUM: logic<32> = {query} + SIGNED_VALUE;
    const LESS_THAN_NEGATIVE: logic = {query} <: -1;
    const SHIFT: logic<32> = {query} >>> 1;
    const WIDTH: u32 = $bits({query});
    const UNSIGNED_RESULT: u32 = {query};
    const SIGNED_RESULT: i32 = {query};
    function choose(c: input logic) -> logic<32> {{
        return if c ? {query} : SIGNED_VALUE;
    }}
    const CHOOSE_FALSE: logic<32> = choose(1'b0);
    const CHOOSE_TRUE: logic<32> = choose(1'b1);
    function choose_unsigned(c: input logic) -> logic<32> {{
        return (if c ? {query} : SIGNED_VALUE) + 32'h0;
    }}
    const CHOOSE_UNSIGNED_FALSE: logic<32> = choose_unsigned(1'b0);
    const CHOOSE_UNSIGNED_TRUE: logic<32> = choose_unsigned(1'b1);
    function choose_comparison(c: input logic) -> logic<32> {{
        return if c ? ({query} <: 6) : SIGNED_VALUE;
    }}
    const CHOOSE_COMPARE_FALSE: logic<32> = choose_comparison(1'b0);
    const CHOOSE_COMPARE_TRUE: logic<32> = choose_comparison(1'b1);
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
        assert!(errors.is_empty(), "{query}: {errors:#?}");

        let Component::Module(module) = &ir.components[0] else {
            panic!("expected module");
        };
        // IEEE 1800-2023 20.6.2/20.7/20.8.1 return integer. Both signed arms must
        // sign-extend even when the query itself is in the unselected arm;
        // an unsigned arm must still zero-extend.
        for (name, expected) in [
            ("SIGNED_FALSE", 0xffff_ffff),
            ("SIGNED_TRUE", 0xffff_ffff),
            ("UNSIGNED_FALSE", 0xff),
            ("UNSIGNED_TRUE", 0xff),
            ("UNSIGNED_OUTER_FALSE", 0xff),
            ("UNSIGNED_OUTER_TRUE", 0xff),
            ("UNSIGNED_SUB_CAST", 0xffff_ffff),
            ("REVERSE_SUB_CAST", 0xffff_ffff),
            ("SIGNED_SUB_CAST", u64::MAX),
            ("COMPARE_CAST", 1),
            ("BITNOT_SUM_CAST", u64::MAX),
            ("TERNARY_CAST", u64::MAX),
            ("TERNARY_SHIFT", 0xffff_ffff),
            ("SUM", 4),
            ("LESS_THAN_NEGATIVE", 0),
            ("SHIFT", 2),
            ("WIDTH", 32),
            ("UNSIGNED_RESULT", 5),
            ("SIGNED_RESULT", 5),
            ("CHOOSE_FALSE", 0xffff_ffff),
            ("CHOOSE_TRUE", 5),
            ("CHOOSE_UNSIGNED_FALSE", 0xff),
            ("CHOOSE_UNSIGNED_TRUE", 5),
            ("CHOOSE_COMPARE_FALSE", 0xff),
            ("CHOOSE_COMPARE_TRUE", 1),
        ] {
            let variable = module
                .variables
                .values()
                .find(|x| x.path.to_string() == name)
                .unwrap();
            assert_eq!(
                variable.get_value(&[]).unwrap().to_u64(),
                Some(expected),
                "{query}: {name}\n{ir}"
            );
        }
    }
}

#[test]
fn bit_and_sign_cast_system_function_return_types() {
    for (query, width, extended) in [
        ("$onehot(4'b0100)", 1, 0xff),
        ("$signed(16'h0005)", 16, 0xffff_ffff),
        ("$unsigned(16'sh0005)", 16, 0xff),
    ] {
        let code = format!(
            r#"
module Top {{
    const SIGNED_VALUE: signed logic<8> = 8'hff;
    const WIDTH: u32 = $bits({query});
    const FALSE_ARM: logic<32> = if 1'b0 ? {query} : SIGNED_VALUE;
    const TRUE_ARM: logic<32> = if 1'b1 ? SIGNED_VALUE : {query};
    function choose(c: input logic) -> logic<32> {{
        return if c ? {query} : SIGNED_VALUE;
    }}
    const CHOOSE_FALSE: logic<32> = choose(1'b0);
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
        assert!(errors.is_empty(), "{query}: {errors:#?}");

        let Component::Module(module) = &ir.components[0] else {
            panic!("expected module");
        };
        for (name, expected) in [
            ("WIDTH", width),
            ("FALSE_ARM", extended),
            ("TRUE_ARM", extended),
            ("CHOOSE_FALSE", extended),
        ] {
            let variable = module
                .variables
                .values()
                .find(|x| x.path.to_string() == name)
                .unwrap();
            assert_eq!(
                variable.get_value(&[]).unwrap().to_u64(),
                Some(expected),
                "{query}: {name}\n{ir}"
            );
        }
    }
}

#[test]
fn constant_function_memoization_matches_uncached_evaluation() {
    type EvaluationCase = (&'static str, fn(u64) -> u64);
    let cases: [EvaluationCase; 8] = [
        (
            r#"
            function leaf(x: input u32) -> u32 {
                if x == 0 { return 3; }
                return x * 7 + 2;
            }
            function run(x: input u32) -> u32 {
                return leaf(x) + leaf(x) + leaf(x + 1);
            }
        "#,
            |x| 2 * if x == 0 { 3 } else { x * 7 + 2 } + (x + 1) * 7 + 2,
        ),
        (
            r#"
            var counter: u32;
            function bump() -> u32 { counter += 1; return 1; }
            function leaf(x: input u32) -> u32 { return x * 7; }
            function run(x: input u32) -> u32 {
                counter = x;
                let result: u32 = leaf(bump()) + leaf(bump());
                return result + counter;
            }
        "#,
            |x| x + 16,
        ),
        (
            r#"
            var counter: u32;
            function peek() -> u32 { return counter; }
            function run(x: input u32) -> u32 {
                counter = x;
                let first: u32 = peek();
                counter += 1;
                return first + peek();
            }
        "#,
            |x| x * 2 + 1,
        ),
        (
            r#"
            function add::<N: u32>(x: input u32) -> u32 { return x + N; }
            function run(x: input u32) -> u32 {
                return add::<1>(x) + add::<2>(x) + add::<1>(x);
            }
        "#,
            |x| x * 3 + 4,
        ),
        (
            r#"
            function pack(x: input u32, y: input u32) -> u32 { return x * 10 + y; }
            function run(x: input u32) -> u32 {
                return pack(x, pack(x + 1, 3));
            }
        "#,
            |x| x * 20 + 13,
        ),
        (
            r#"
            function pack(x: input u32, y: input u32) -> u32 { return x * 10 + y; }
            function run(x: input u32) -> u32 {
                let ignored: u32 = pack(x, pack(x + 1, 3));
                return pack(x, (x + 1) * 10 + 3) + (ignored & 0);
            }
        "#,
            |x| x * 20 + 13,
        ),
        (
            r#"
            function pack(x: input u32, y: input u32) -> u32 { return x * 10 + y; }
            function run(x: input u32) -> u32 {
                return pack(pack(x, 1), pack(x, 2));
            }
        "#,
            |x| x * 110 + 12,
        ),
        (
            r#"
            var counter: u32;
            function bump() -> u32 { counter += 1; return counter; }
            function pack(x: input u32, y: input u32) -> u32 { return x * 10 + y; }
            function run(x: input u32) -> u32 {
                counter = x;
                return pack(bump(), pack(bump(), bump()));
            }
        "#,
            |x| x * 21 + 33,
        ),
    ];
    for (functions, expected) in cases {
        let code =
            format!("module Top(i: input u32, o: output u32) {{ {functions} assign o = run(i); }}");
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
        assert!(errors.is_empty(), "{code}\n{errors:#?}");
        let Component::Module(module) = &ir.components[0] else {
            panic!("expected module");
        };
        let input = module
            .ports
            .iter()
            .find(|(path, _)| path.to_string() == "i")
            .unwrap()
            .1;
        let expression = module
            .declarations
            .iter()
            .find_map(|declaration| {
                let Declaration::Comb(comb) = declaration else {
                    return None;
                };
                comb.statements.iter().find_map(|statement| {
                    let Statement::Assign(assign) = statement else {
                        return None;
                    };
                    Some(&assign.expr)
                })
            })
            .unwrap();
        for cached in [false, true] {
            let mut context = Context::default();
            context.variables = module.variables.clone();
            context.functions = module.functions.clone();
            if !cached {
                // is_const is the cache's eligibility check. Value evaluation
                // itself is otherwise identical for this independent control.
                for function in context.functions.values_mut() {
                    function.is_const = false;
                }
            }
            for value in [0, 1, 7, 99, 1, 0] {
                context.variables.get_mut(input).unwrap().set_value(
                    &[],
                    Value::new(value, 32, false),
                    None,
                );
                let actual = expression.eval_value(&mut context).unwrap().to_u64();
                assert_eq!(
                    actual,
                    Some(expected(value)),
                    "cached={cached}, input={value}\n{code}"
                );
            }
        }
    }
}

#[test]
fn nested_function_actuals_do_not_corrupt_constant_folding() {
    let code = r#"
        module Top(o: output u32) {
            function pack(x: input u32, y: input u32) -> u32 {
                return x * 10 + y;
            }
            function run(x: input u32) -> u32 {
                let ignored: u32 = pack(x, pack(x + 1, 3));
                return pack(x, (x + 1) * 10 + 3) + (ignored & 0);
            }
            assign o = run(1);
        }
    "#;
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    assert!(errors.is_empty(), "{errors:#?}");
    let Component::Module(module) = &ir.components[0] else {
        panic!("expected module");
    };
    let expression = module
        .declarations
        .iter()
        .find_map(|declaration| {
            let Declaration::Comb(comb) = declaration else {
                return None;
            };
            comb.statements.iter().find_map(|statement| {
                let Statement::Assign(assign) = statement else {
                    return None;
                };
                Some(&assign.expr)
            })
        })
        .unwrap();
    // No variable/function table: inspect the actual folded constant rather
    // than re-evaluating run with a fresh set of call frames.
    let result = expression
        .eval_value(&mut Context::default())
        .unwrap()
        .to_u64();
    assert_eq!(result, Some(33), "{ir}");
}

/// The folded constant of each `assign`, in source order.
fn folded_assigns(code: &str) -> Vec<Value> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    // A multi-bit condition is the point here; it only draws a warning.
    errors.retain(|e| !matches!(e, AnalyzerError::InvalidLogicalOperand { .. }));
    assert!(errors.is_empty(), "{errors:#?}");
    let Component::Module(module) = &ir.components[0] else {
        panic!("expected module");
    };
    let mut ret = vec![];
    for declaration in &module.declarations {
        let Declaration::Comb(comb) = declaration else {
            continue;
        };
        for statement in &comb.statements {
            if let Statement::Assign(assign) = statement {
                ret.push(assign.expr.eval_value(&mut Context::default()).unwrap());
            }
        }
    }
    ret
}

#[test]
fn wide_constant_as_truth_value_index_and_bound() {
    // HI is nonzero only above bit 63; ONE and TWO are small values in a wide
    // type, which `to_usize` cannot convert either.
    let code = r#"
    module Top (
        o0: output logic<32>, o1: output logic<32>, o2: output logic<32>,
        o3: output logic<32>, o4: output logic<32>, o5: output logic<32>,
        o6: output logic<32>, o7: output logic<32>,
    ) {
        const HI : bit<128> = 128'h1_0000_0000_0000_0000;
        const ONE: bit<128> = 128'd1;
        const TWO: bit<128> = 128'd2;
        const VEC: bit<8>   = 8'b0000_0100;
        function f_if (x: input bit<128>) -> bit<32> {
            if x { return 1; }
            return 0;
        }
        function f_case (x: input bit<128>) -> bit<32> {
            case x {
                128'h1_0000_0000_0000_0000: return 5;
                default                   : return 9;
            }
        }
        function f_loop (n: input bit<128>) -> bit<32> {
            var acc: bit<32>;
            acc = 0;
            for _i in 0..n { acc += 1; }
            return acc;
        }
        const K_WIDTH: u32 = if HI ? 8 : 4;
        assign o0 = if HI ? 7 : 3;
        assign o1 = if ONE ? 7 : 3;
        assign o2 = f_if(HI);
        assign o3 = f_case(HI);
        assign o4 = {31'b0, VEC[TWO]};
        assign o5 = f_loop(TWO);
        assign o6 = K_WIDTH;
        if HI :g {
            assign o7 = 1;
        } else {
            assign o7 = 0;
        }
    }
    "#;
    let values: Vec<_> = folded_assigns(code)
        .iter()
        .map(|v| v.to_u64().unwrap())
        .collect();
    assert_eq!(values, [7, 7, 1, 5, 1, 2, 8, 1]);
}

#[test]
fn logical_value_of_a_partly_unknown_condition() {
    // A known 1 decides the condition whatever the x/z bits; with none, an
    // `if` and a ternary both take the false side, as in the 2-state simulator.
    let code = r#"
    module Top (o0: output logic<4>, o1: output logic<4>, o2: output logic<4>) {
        function f (c: input logic<2>) -> logic<4> {
            if c { return 4'b1100; }
            return 4'b1010;
        }
        function g (c: input logic<2>) -> logic<4> {
            return if c ? 4'b1100 : 4'b1010;
        }
        assign o0 = f(2'b1x);
        assign o1 = f(2'b0x);
        assign o2 = g(2'b1x);
    }
    "#;
    let values: Vec<_> = folded_assigns(code)
        .iter()
        .map(|v| v.to_u64().unwrap())
        .collect();
    assert_eq!(values, [0b1100, 0b1010, 0b1100]);
}
