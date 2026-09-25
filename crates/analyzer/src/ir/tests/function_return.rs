use super::*;
use crate::ir::{Component, ControlFlow, VarPath};
use crate::value::Value;

/// Execute the generated body without FunctionCall's special handling of writes
/// to the return slot. Runtime IR consumers must get correct control flow too.
#[track_caller]
fn check_runtime_body(body: &str, cases: &[(u64, u64, u64)]) {
    let code = format!(
        r#"module Top (d: input logic<8>, q: output logic<8>, s: output logic<8>) {{
            function f(x: input logic<8>, seen: output logic<8>) -> logic<8> {{
                seen = 0;
                {body}
            }}
            always_comb {{ q = f(d, s); }}
        }}"#
    );
    let (mut context, body, ir) = runtime_function(&code);
    let x = body.arg_map[&VarPath::new(resource_table::insert_str("x"))];
    let seen = body.arg_map[&VarPath::new(resource_table::insert_str("seen"))];
    let ret = body.ret.unwrap();
    for &(input, expected, expected_seen) in cases {
        context
            .variables
            .get_mut(&x)
            .unwrap()
            .set_value(&[], Value::new(input, 8, false), None);
        for statement in &body.statements {
            assert!(statement.eval_value(&mut context) == ControlFlow::Continue);
        }
        assert_eq!(
            context.variables[&ret].get_value(&[]),
            Some(&Value::new(expected, 8, false)),
            "input={input}\n{ir}"
        );
        assert_eq!(
            context.variables[&seen].get_value(&[]),
            Some(&Value::new(expected_seen, 8, false)),
            "input={input}\n{ir}"
        );
        assert!(!context.function_returned);
    }
}

#[test]
fn conditional_return_stops_runtime_body() {
    check_runtime_body(
        "if x == 0 { return x + 8'd1; } seen = 9; return x + 8'd2;",
        &[(0, 1, 0), (5, 7, 9), (0, 1, 0)],
    );
}

#[test]
fn unconditional_return_discards_unreachable_runtime_body() {
    check_runtime_body("return x; seen = 9; return 8'd99;", &[(3, 3, 0)]);
}

#[test]
fn nested_branch_returns_stop_runtime_body() {
    check_runtime_body(
        r#"if x <: 3 {
            if x == 0 { return 8'd10; }
            else if x == 1 { return 8'd11; }
            seen = 2;
        } else { return 8'd13; }
        seen = seen + 8'd1;
        return 8'd12;"#,
        &[(0, 10, 0), (1, 11, 0), (2, 12, 3), (3, 13, 0)],
    );
}

#[test]
fn case_and_switch_returns_stop_runtime_body() {
    for branch in [
        "case x { 0: { return 8'd10; } 1: { seen = 1; } default: { return 8'd12; } }",
        "switch { x == 0: { return 8'd10; } x == 1: { seen = 1; } default: { return 8'd12; } }",
    ] {
        check_runtime_body(
            &format!("{branch} seen = seen + 8'd1; return 8'd11;"),
            &[(0, 10, 0), (1, 11, 2), (2, 12, 0)],
        );
    }
}

#[test]
fn loop_return_unwinds_all_runtime_loops() {
    check_runtime_body(
        r#"for i in 0..3 {
            for j in 0..3 {
                if x == i * 3 + j { return seen; }
                seen = seen + 8'd1;
            }
            seen = seen + 8'd10;
        }
        seen = seen + 8'd100;
        return seen;"#,
        &[(0, 0, 0), (1, 1, 1), (4, 14, 14), (9, 139, 139), (0, 0, 0)],
    );
}

#[test]
fn runtime_loop_break_still_only_exits_loop() {
    check_runtime_body(
        r#"for i in 0..3 {
            if x == 0 { break; }
            if x == i { return 8'd10; }
            seen = seen + 8'd1;
        }
        seen = seen + 8'd100;
        return seen;"#,
        &[(0, 100, 100), (1, 10, 1), (4, 103, 103)],
    );
}

#[test]
fn return_does_not_recheck_mutated_branch_condition() {
    check_runtime_body(
        r#"var test: logic<8>;
        test = x;
        if test == 0 { test = 1; return 8'd10; }
        seen = test;
        return 8'd20;"#,
        &[(0, 10, 0), (1, 20, 1)],
    );
}

#[test]
fn runtime_bound_loop_return_handles_empty_range() {
    check_runtime_body(
        r#"for i in 0..x {
            if i == 1 { return seen; }
            seen = seen + 8'd1;
        }
        seen = seen + 8'd100;
        return seen;"#,
        &[(0, 100, 100), (1, 101, 101), (2, 1, 1), (0, 100, 100)],
    );
}

#[test]
fn unconditional_return_in_runtime_loop() {
    check_runtime_body(
        r#"for i in 0..x {
            return i;
        }
        seen = 100;
        return seen;"#,
        &[(0, 100, 100), (1, 0, 0), (2, 0, 0)],
    );
}

fn runtime_function(code: &str) -> (Context, crate::ir::FunctionBody, Ir) {
    symbol_table::clear();
    attribute_table::clear();
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.extend(Analyzer::analyze_post_pass2(&ir));
    assert!(errors.is_empty(), "{errors:?}\n{code}");
    let Component::Module(module) = &ir.components[0] else {
        panic!()
    };
    let function = module
        .functions
        .values()
        .find(|f| f.name.to_string() == "f")
        .unwrap();
    let body = function.functions[0].clone();
    context.variables = module.variables.clone();
    context.functions = module.functions.clone();
    context.disalbe_const_opt = true;
    assert!(context.function_ret_var.is_none());
    (context, body, ir)
}

#[test]
fn runtime_return_preserves_module_interface_and_output_assignments() {
    let code = r#"
    interface Bus { var data: logic<8>; }
    module Top (d: input logic<8>, q: output logic<8>, s: output logic<8>,
                m: output logic<8>, b: output logic<8>) {
        inst bus: Bus;
        function f(x: input logic<8>, seen: output logic<8>) -> logic<8> {
            m = 10;
            bus.data = 20;
            seen = 30;
            if x == 0 { return 8'd1; }
            m = 99;
            bus.data = 99;
            seen = 99;
            return 8'd2;
        }
        always_comb { q = f(d, s); }
        assign b = bus.data;
    }
    "#;
    let (mut context, body, ir) = runtime_function(code);
    let x = body.arg_map[&VarPath::new(resource_table::insert_str("x"))];
    let seen = body.arg_map[&VarPath::new(resource_table::insert_str("seen"))];
    let find = |path: &str| {
        context
            .variables
            .values()
            .find(|v| v.path.to_string() == path)
            .unwrap()
            .id
    };
    let destinations = [body.ret.unwrap(), find("m"), find("bus.data"), seen];
    for (input, expected) in [
        (0, [1, 10, 20, 30]),
        (1, [2, 99, 99, 99]),
        (0, [1, 10, 20, 30]),
    ] {
        context
            .variables
            .get_mut(&x)
            .unwrap()
            .set_value(&[], Value::new(input, 8, false), None);
        for statement in &body.statements {
            assert!(statement.eval_value(&mut context) == ControlFlow::Continue);
        }
        for (id, value) in destinations.into_iter().zip(expected) {
            assert_eq!(
                context.variables[&id].get_value(&[]),
                Some(&Value::new(value, 8, false)),
                "input={input}, var={id}\n{ir}"
            );
        }
        assert!(!context.function_returned);
    }
}
