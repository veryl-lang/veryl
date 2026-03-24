use super::*;

#[test]
fn top_module_not_found() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = a + b;
    }
    "#;

    let result = analyze_top(code, &Config::default(), "NonExistent");
    assert!(matches!(
        result,
        Err(SimulatorError::TopModuleNotFound { .. })
    ));
}

#[test]
fn combinational_loop() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: output logic<32>,
    ) {
        var x: logic<32>;
        assign x = b + a;
        assign b = x;
    }
    "#;

    let result = analyze_top(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::CombinationalLoop { .. })
    ));
}

#[test]
fn no_initial_block() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = a + b;
    }
    "#;

    let ir = analyze(code, &Config::default());
    let module_name = ir.name.to_string();
    let result = run_native_testbench(ir, None, module_name);
    assert!(matches!(result, Err(SimulatorError::NoInitialBlock { .. })));
}

#[test]
fn recursive_function_unresolved() {
    // Direct recursion: analyzer converts the recursive call to Factor::Unknown
    // because the function body is not yet registered in context.functions
    // when processing its own body. The simulator detects this as
    // UnsupportedDescription during IR conversion.
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        function recurse(x: input logic<32>) -> logic<32> {
            return recurse(x);
        }

        always_comb {
            c = recurse(a);
        }
    }
    "#;

    let result = analyze_top(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::UnsupportedDescription { .. })
    ));
}

#[test]
fn unsupported_statement() {
    // SystemVerilog function call produces Statement::Unsupported
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        always_comb {
            c = a;
            $sv::sv_func();
        }
    }
    "#;

    let result = analyze_top(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::UnsupportedDescription { .. })
    ));
}
