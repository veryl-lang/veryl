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

    // The analyzer rejects this too; this test is about the simulator's own
    // detector, which is the safety net for a loop the analyzer misses.
    let result = analyze_top_allowing_comb_loop(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::CombinationalLoop { .. })
    ));
}

#[test]
fn combinational_loop_through_one_struct_member() {
    // The counterpart of `struct_members_across_a_boundary_are_not_one_loop`:
    // there the ring is written at one member and read back at another, here
    // both ends are `busy`.  Keying by bits must not lose a loop that closes
    // inside one member.
    let code = r#"
    package pkg {
        struct cfg_t {
            incr: logic,
            strb: logic,
        }

        struct sts_t {
            busy    : logic,
            overflow: logic,
        }
    }

    #[allow(unassign_variable)]
    module Reg (
        cfg: input  pkg::cfg_t,
        sts: output pkg::sts_t,
        req: input  logic     ,
    ) {
        assign sts.busy = cfg.incr & req;
    }

    #[allow(unassign_variable)]
    module Top (
        a  : input  logic,
        req: input  logic,
        o  : output logic,
    ) {
        var cfg: pkg::cfg_t;
        var sts: pkg::sts_t;

        inst u: Reg (
            cfg: cfg,
            sts: sts,
            req     ,
        );

        assign cfg.incr = !(a & sts.busy);
        assign o        = sts.busy;
    }
    "#;

    let result = analyze_top_allowing_comb_loop(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::CombinationalLoop { .. })
    ));
}

#[test]
fn combinational_loop_through_a_dynamic_bit_write() {
    // `v[i]` names its destination bit at runtime, so the write reports no
    // range at all.  The bit-keyed check has to read that as "every bit" --
    // narrowing it to nothing would drop the edge and lose the loop.
    let code = r#"
    #[allow(unassign_variable)]
    module Top (
        a: input  logic   ,
        i: input  logic<3>,
        o: output logic<8>,
    ) {
        var v: logic<8>;
        var u: logic   ;

        assign v[i] = u & a;
        assign u    = v[3];
        assign o    = v;
    }
    "#;

    let result = analyze_top_allowing_comb_loop(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::CombinationalLoop { .. })
    ));
}

#[test]
fn combinational_loop_closed_by_a_later_writer_of_the_bits_read() {
    // `v` has two writers, and the one that produces the bits `z` reads comes
    // AFTER it.  Binding the read to its last prior writer, as a reassigned
    // variable is bound, finds a writer of the OTHER half and nothing that
    // closes the ring -- so the bits read have to reach back for their writer
    // even when it sits later.
    let code = r#"
    module Top (
        o: output logic<8>,
    ) {
        var v: logic<8>;
        var z: logic<4>;

        assign v[3:0] = z;
        assign z      = v[7:4];
        assign v[7:4] = v[3:0];
        assign o      = v;
    }
    "#;

    let result = analyze_top_allowing_comb_loop(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::CombinationalLoop { .. })
    ));
}

#[test]
fn combinational_loop_closed_inside_one_arm_of_a_case() {
    // The ring runs through `2'd0` alone -- `a` is `b` there and `b` is `a`
    // always -- and the other arms are only alternatives to it.  Keying the
    // check by branch splits the `case` into a piece per arm, so `a` reads as
    // several writers where it used to read as one: WHICH writer binds has to
    // stay the statement's answer, or the last prior piece binds, the arm
    // that closes the ring is never reached, and a real loop goes unreported.
    let code = r#"
    module Top (
        s: input  logic<2>,
        c: input  logic   ,
        o: output logic   ,
    ) {
        var a: logic;
        var b: logic;

        always_comb {
            case s {
                2'd0: {
                    a = b;
                }
                2'd1: {
                    a = c;
                }
                default: {
                    a = 1'b0;
                }
            }
        }

        assign b = a;
        assign o = b;
    }
    "#;

    let result = analyze_top_allowing_comb_loop(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::CombinationalLoop { .. })
    ));
}

#[test]
fn undetermined_width() {
    // An unevaluatable width used to panic during IR construction.
    let code = r#"
    module Top (
        a: input  logic,
        c: output logic,
    ) {
        var d: logic<$sv::some_pkg::WIDTH>;
        assign d = a;
        assign c = d[0];
    }
    "#;

    let result = analyze_top(code, &Config::default(), "Top");
    assert!(matches!(
        result,
        Err(SimulatorError::UndeterminedWidth { .. })
    ));

    // A package const gets a synthesized path that must not reach the message.
    let code = r#"
    package P {
        const X: logic<$sv::some_pkg::WIDTH> = 0;
    }
    module Top (
        c: output logic,
    ) {
        assign c = P::X[0];
    }
    "#;

    match analyze_top(code, &Config::default(), "Top") {
        Err(SimulatorError::UndeterminedWidth { subject, .. }) => {
            assert!(
                !subject.contains("__const_"),
                "internal name leaked: {subject}"
            );
        }
        Err(x) => panic!("unexpected error: {x:?}"),
        Ok(_) => panic!("expected UndeterminedWidth"),
    }
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
