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

#[test]
fn unsupported_sv_module_instance() {
    // Instantiating a SystemVerilog blackbox (`$sv::SvMod`) must surface as
    // UnsupportedDescription rather than panic during IR build.
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        inst u: $sv::SvMod (
            a,
            c,
        );
    }
    "#;

    let result = analyze_top(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::UnsupportedDescription { .. })
    ));
}

#[test]
fn dynamic_index_leaving_a_sub_array() {
    // These used to hit a `build_linear_index_expr` assertion instead of
    // reporting.
    let bodies = [
        "x = 8'd0; s[i] = t;",
        "x = 8'd0; s[i] = pk::TBL;",
        "x = 8'd0; s[i] = f();",
        "x = 8'd0; s[i] = u[j];",
        "{s[i], x} = 16'h1234;",
    ];

    for body in bodies {
        let code = format!(
            r#"
    package pk {{
        type sub = logic<8> [3];
        const TBL: sub = '{{8'd1, 8'd2, 8'd3}};
    }}
    module Top (
        i: input  logic<1>,
        j: input  logic<1>,
        x: output logic<8>,
        o: output logic<8>,
    ) {{
        function f () -> pk::sub {{
            return '{{8'd1, 8'd2, 8'd3}};
        }}
        var t: logic<8> [3];
        var u: logic<8> [2, 3];
        var s: logic<8> [2, 3];
        assign t = '{{8'd1, 8'd2, 8'd3}};
        assign u = '{{'{{8'd1, 8'd2, 8'd3}}, '{{8'd4, 8'd5, 8'd6}}}};
        always_comb {{
            s = '{{'{{8'd0, 8'd0, 8'd0}}, '{{8'd0, 8'd0, 8'd0}}}};
            {body}
        }}
        assign o = s[1][2];
    }}
    "#
        );

        let result = analyze_top(&code, &Config::default(), "Top");
        assert!(
            matches!(result, Err(SimulatorError::UnsupportedDescription { .. })),
            "{body}"
        );
    }
}
