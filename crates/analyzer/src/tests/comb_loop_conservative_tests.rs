//! Intentional limits of combinational-loop analysis.
//!
//! The detector preserves structural dependencies instead of proving value
//! identities or correlating predicates written in separate expressions.
//! These cases are expected to report a loop and are not implementation
//! defects tracked by ignored tests.

use super::*;

fn assert_intentionally_conservative(case: &str, code: &str) {
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{case}: structural dependence must be preserved: {errors:?}"
    );
}

#[test]
fn duplicate_xor_operands_remain_structural_inputs() {
    assert_intentionally_conservative(
        "the detector does not prove x ^ x = 0",
        r#"
        module Top (
            o: output logic,
        ) {
            assign o = o ^ o;
        }
        "#,
    );
}

#[test]
fn identical_ternary_arms_retain_structural_control_dependence() {
    assert_intentionally_conservative(
        "the detector does not eliminate an identical-arm conditional",
        r#"
        module Top (
            o: output logic,
        ) {
            assign o = if o ? 1'b0 : 1'b0;
        }
        "#,
    );
}

#[test]
fn identical_function_branches_retain_structural_control_dependence() {
    assert_intentionally_conservative(
        "the detector does not prove a function result independent of its condition",
        r#"
        module Top (
            o: output logic,
        ) {
            function choose (condition: input logic) -> logic {
                if condition {
                    return 0;
                } else {
                    return 0;
                }
            }
            assign o = choose(o);
        }
        "#,
    );
}

#[test]
fn boolean_cancellation_inside_a_function_does_not_remove_its_input_dependency() {
    assert_intentionally_conservative(
        "the detector does not prove x & 0 = 0",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (x: input logic<8>) -> logic {
                var tmp: logic<8>;
                tmp = x & 8'b00000000;
                return tmp[0];
            }
            var value: logic<8>;
            assign o = low(value);
            assign value[0] = o;
            assign value[7:1] = 0;
        }
        "#,
    );
}

#[test]
fn separate_complementary_conditionals_are_not_correlated() {
    assert_intentionally_conservative(
        "separately written predicates are not proven mutually exclusive",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                if sel {
                    a = b;
                }
                if !sel {
                    b = a;
                }
                o = a | b;
            }
        }
        "#,
    );
}

#[test]
fn complementary_short_circuit_predicates_are_not_correlated() {
    assert_intentionally_conservative(
        "predicates in separate short-circuit operands are not proven mutually exclusive",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var a    : logic;
            var b    : logic;
            var dummy: logic;
            function write_a (value: input logic) -> logic {
                a = value;
                return 1'b0;
            }
            function write_b (value: input logic) -> logic {
                b = value;
                return 1'b0;
            }
            always_comb {
                dummy = (sel && write_a(b)) | (!sel && write_b(a));
                o = a | b | dummy;
            }
        }
        "#,
    );
}
