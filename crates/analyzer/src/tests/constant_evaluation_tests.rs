use super::*;
use crate::ir::{Component, Declaration, Statement};
use crate::value::Value;

#[test]
fn constant_function_memoization_matches_uncached_evaluation() {
    type EvaluationCase = (&'static str, fn(u64) -> u64);
    let cases: [EvaluationCase; 4] = [
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
