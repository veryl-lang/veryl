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
            ("SUM", 4),
            ("LESS_THAN_NEGATIVE", 0),
            ("SHIFT", 2),
            ("WIDTH", 32),
            ("UNSIGNED_RESULT", 5),
            ("SIGNED_RESULT", 5),
            ("CHOOSE_FALSE", 0xffff_ffff),
            ("CHOOSE_TRUE", 5),
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
