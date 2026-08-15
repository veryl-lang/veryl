//! Intentional false positives of combinational-loop analysis.
//!
//! Keep every accepted false-positive class explicit here so that a reported
//! loop is not mistaken for an accidental regression. The accepted classes
//! are:
//! - algebraic identities and cancellation;
//! - equivalent results behind data-dependent control flow;
//! - correlations between separately written or instantiated predicates;
//! - runtime indices below one longest static prefix, including affine offsets,
//!   range-disjoint indices, and static suffix dimensions below a dynamic one;
//! - runtime ranges whose emptiness or exact iteration count requires bound
//!   correlation, inequality reasoning, or the bound type's finite width;
//! - forward/reverse iterator order, overwrite order, `break`, and iterator
//!   predicates excluded by runtime bounds or stride.
//!
//! Each class has a loop-free program below which is intentionally diagnosed.

use super::*;

fn assert_intentional_false_positive(case: &str, code: &str) {
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{case}: this loop-free case is intentionally diagnosed: {errors:?}"
    );
}

// Value and control expressions remain structural dependencies.

#[test]
fn duplicate_xor_operands_remain_structural_inputs() {
    assert_intentional_false_positive(
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
    assert_intentional_false_positive(
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
    assert_intentional_false_positive(
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
    assert_intentional_false_positive(
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

// Separately written predicates are not correlated.

#[test]
fn separate_complementary_conditionals_are_not_correlated() {
    assert_intentional_false_positive(
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
    assert_intentional_false_positive(
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

#[test]
fn shared_selector_across_instances_is_not_correlated() {
    assert_intentional_false_positive(
        "separate child instances do not share predicate identity through a common actual",
        r#"
        module SelectRoute (
            sel    : input  logic,
            i      : input  logic,
            o_true : output logic,
            o_false: output logic,
        ) {
            always_comb {
                if sel {
                    o_true = i;
                    o_false = 0;
                } else {
                    o_true = 0;
                    o_false = i;
                }
            }
        }
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var x: logic;
            var y: logic;
            inst forward: SelectRoute (
                sel,
                i      : x,
                o_true : y,
                o_false: _,
            );
            inst reverse: SelectRoute (
                sel,
                i      : y,
                o_true : _,
                o_false: x,
            );
            assign o = x | y;
        }
        "#,
    );
}

// Runtime loops use LSP aliasing and a conservative repeated transfer.

#[test]
fn correlated_dynamic_bounds_create_an_intentional_false_positive() {
    // The actual range n..n+1 contains exactly one iteration. Treating its two
    // bounds conservatively admits a two-iteration path and therefore rejects
    // the potential feedback from value[0] to value[2].
    let code = r#"
        module Top (
            n: input  logic,
            o: output logic,
        ) {
            var value   : logic [3];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {
                value = '{default: 0};
                value[0] = feedback;
                for index in n..(n + 1) {
                    value[index + 1] = value[index];
                }
                o = feedback;
            }
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_intentional_false_positive(
        "correlated runtime bounds invent a multi-iteration path",
        code,
    );
}

#[test]
fn runtime_loop_prefixes_are_not_ordered_by_iterator_value() {
    // Every concrete execution overwrites value[1] before it can reach
    // value[2]. Collapsing all iterator values into one LSP loses that order.
    assert_intentional_false_positive(
        "runtime accesses through index and index plus one share one LSP alias domain",
        r#"
        module Top (
            n: input  logic<3>,
            o: output logic,
        ) {
            var value   : logic [3];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {
                value = '{default: 0};
                value[1] = feedback;
                for index in 0..n {
                    value[index + 1] = value[index];
                }
                o = feedback;
            }
        }
        "#,
    );
}

#[test]
fn reverse_runtime_loop_prefixes_are_not_ordered_by_iterator_value() {
    // Every non-empty concrete range starts at index 2, which kills the seed
    // before any lower element is written.
    assert_intentional_false_positive(
        "a reverse iteration that kills the seed before propagation is not ordered",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [4];
            var feedback: logic;
            assign feedback = value[0];
            always_comb {
                value = '{default: 0};
                value[2] = feedback;
                for index in rev n..3 {
                    value[index] = value[index + 1];
                }
                o = feedback;
            }
        }
        "#,
    );
}

#[test]
fn runtime_index_offset_is_not_proven_disjoint_from_a_static_element() {
    // For every reachable iterator, index + 1 is in 1..=3 and can never write
    // the feedback source at element zero.
    assert_intentional_false_positive(
        "index plus one shares the array LSP with element zero",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [4];
            var feedback: logic;
            assign feedback = value[0];
            always_comb {
                value = '{default: 0};
                for index in 0..n {
                    value[index + 1] = feedback;
                }
                o = feedback;
            }
        }
        "#,
    );
}

#[test]
fn static_suffix_below_a_dynamic_dimension_is_not_kept_disjoint() {
    // The body writes only inner element zero while feedback reads inner
    // element one. The dynamic outer index shortens their shared LSP.
    assert_intentional_false_positive(
        "different inner dimensions below a dynamic outer index share one LSP",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [4, 2];
            var feedback: logic;
            assign feedback = value[0][1];
            always_comb {
                value = '{default: 0};
                for index in 0..n {
                    value[index][0] = feedback;
                }
                o = feedback;
            }
        }
        "#,
    );
}

#[test]
fn runtime_range_emptiness_is_not_proven_from_unsigned_bound_order() {
    // An unsigned value cannot be below zero, so n..0 has no iterations.
    assert_intentional_false_positive(
        "an unsigned forward range ending at zero is always empty",
        r#"
        module Top (
            n: input  logic<3>,
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            assign a = b;
            always_comb {
                b = 0;
                for _index in n..0 {
                    b = a;
                }
                o = b;
            }
        }
        "#,
    );
}

#[test]
fn runtime_bound_width_is_not_used_as_an_iteration_limit() {
    // A one-bit n is zero or one. The only possible iteration writes value[1]
    // and cannot propagate the seed as far as value[2].
    assert_intentional_false_positive(
        "a one-bit bound permits at most one iteration but closure is unbounded",
        r#"
        module Top (
            n: input  logic,
            o: output logic,
        ) {
            var value   : logic [3];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {
                value = '{default: 0};
                value[0] = feedback;
                for index in 0..n {
                    value[index + 1] = value[index];
                }
                o = feedback;
            }
        }
        "#,
    );
}

#[test]
fn runtime_break_is_not_used_as_an_iteration_limit() {
    // The unconditional break prevents the second iteration required to reach
    // value[2].
    assert_intentional_false_positive(
        "an unconditional break permits one iteration but closure is unbounded",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [3];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {
                value = '{default: 0};
                value[0] = feedback;
                for index in 0..n {
                    value[index + 1] = value[index];
                    break;
                }
                o = feedback;
            }
        }
        "#,
    );
}

#[test]
fn runtime_stride_reachability_is_not_used_to_prune_iterator_branches() {
    // Starting from zero with step += 2 never executes the index == 1 arm.
    assert_intentional_false_positive(
        "a step of two never reaches the iterator-one feedback arm",
        r#"
        module Top (
            n: input  logic<3>,
            o: output logic,
        ) {
            var value   : logic [3];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {
                value = '{default: 0};
                value[0] = feedback;
                for index in 0..n step += 2 {
                    if index == 0 {
                        value[1] = value[0];
                    }
                    if index == 1 {
                        value[2] = value[1];
                    }
                }
                o = feedback;
            }
        }
        "#,
    );
}

#[test]
fn runtime_bound_relation_is_not_used_to_prune_iterator_branches() {
    // Every iterator produced by n..4 is greater than or equal to n, so the
    // index < n arm is unreachable.
    assert_intentional_false_positive(
        "the iterator is not correlated with the bound that produced it",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [3];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {
                value = '{default: 0};
                value[1] = feedback;
                for index in n..4 {
                    if index <: n {
                        value[2] = value[1];
                    }
                }
                o = feedback;
            }
        }
        "#,
    );
}
