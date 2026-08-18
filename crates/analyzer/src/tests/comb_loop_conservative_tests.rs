//! Intentional false positives of combinational-loop analysis.
//!
//! Keep every accepted false-positive class explicit here so that a reported
//! loop is not mistaken for an accidental regression. The accepted classes
//! are:
//! - algebraic identities and cancellation;
//! - two-state operator semantics whose bit influence is narrower than an
//!   identity or whole-value dependency;
//! - constant arithmetic, masks, and predicates that make result bits
//!   independent of structurally present operands;
//! - equivalent results behind data-dependent control flow;
//! - runtime shifts and selectors whose reachable source sets are not one
//!   fixed positional offset;
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
    let complete = comb_loop_analysis_is_complete(code);
    let errors = analyze(code);
    assert!(
        complete
            && !errors.is_empty()
            && errors
                .iter()
                .all(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{case}: expected complete analysis and only combinational-loop diagnostics: {errors:?}"
    );
}

// Value and control expressions remain structural dependencies. The XOR case
// below is a definite false positive for two-state `bit`. The corresponding
// `logic` case is deliberately left unclassified: its answer depends on
// whether loop detection follows four-state SystemVerilog evaluation or the
// synthesized two-state circuit.

#[test]
fn duplicate_xor_operands_remain_structural_inputs() {
    assert_intentional_false_positive(
        "the detector does not prove x ^ x = 0",
        r#"
        module Top (
            o: output bit,
        ) {
            var value: bit;
            assign o = value ^ value;
            assign value = o;
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
            o: output bit,
        ) {
            var condition: bit;
            assign o = if condition ? 1'b0 : 1'b0;
            assign condition = o;
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
            o: output bit,
        ) {
            function choose (condition: input bit) -> bit {
                if condition {
                    return 0;
                } else {
                    return 0;
                }
            }
            var condition: bit;
            assign o = choose(condition);
            assign condition = o;
        }
        "#,
    );
}

#[test]
fn a_constant_and_mask_inside_a_function_does_not_remove_the_masked_input_bit() {
    assert_intentional_false_positive(
        "the detector does not remove an input bit annihilated by an AND mask",
        r#"
        module Top (
            o: output bit,
        ) {
            function low (x: input bit<8>) -> bit {
                var tmp: bit<8>;
                tmp = x & 8'b11111110;
                return tmp[0];
            }
            var value: bit<8>;
            assign o = low(value);
            assign value[0] = o;
            assign value[7:1] = 0;
        }
        "#,
    );
}

// These arithmetic cases are definite false positives for two-state `bit`
// operands. The corresponding `logic` cases are left unclassified: that
// depends on whether the detector follows SystemVerilog four-state expression
// semantics or the synthesized two-state circuit.

#[test]
fn addition_does_not_propagate_a_high_input_bit_to_the_low_result_bit() {
    assert_intentional_false_positive(
        "the low sum bit depends only on the low bits of the operands",
        r#"
        module Top (
            rhs: input  bit<8>,
            o  : output bit,
        ) {
            var value: bit<8>;
            var sum  : bit<8>;
            assign sum = value + rhs;
            assign value[7] = sum[0];
            assign value[6:0] = 0;
            assign o = sum[0];
        }
        "#,
    );
}

#[test]
fn subtraction_does_not_propagate_a_high_input_bit_to_the_low_result_bit() {
    assert_intentional_false_positive(
        "the low difference bit depends only on the low bits of the operands",
        r#"
        module Top (
            rhs: input  bit<8>,
            o  : output bit,
        ) {
            var value     : bit<8>;
            var difference: bit<8>;
            assign difference = value - rhs;
            assign value[7] = difference[0];
            assign value[6:0] = 0;
            assign o = difference[0];
        }
        "#,
    );
}

#[test]
fn negation_does_not_propagate_a_high_input_bit_to_the_low_result_bit() {
    assert_intentional_false_positive(
        "the low two's-complement bit depends only on the low input bit",
        r#"
        module Top (
            o: output bit,
        ) {
            var value  : bit<8>;
            var negated: bit<8>;
            assign negated = -value;
            assign value[7] = negated[0];
            assign value[6:0] = 0;
            assign o = negated[0];
        }
        "#,
    );
}

#[test]
fn multiplication_does_not_propagate_a_high_input_bit_to_the_low_result_bit() {
    assert_intentional_false_positive(
        "the low product bit depends only on the low bits of the operands",
        r#"
        module Top (
            rhs: input  bit<8>,
            o  : output bit,
        ) {
            var value  : bit<8>;
            var product: bit<8>;
            assign product = value * rhs;
            assign value[7] = product[0];
            assign value[6:0] = 0;
            assign o = product[0];
        }
        "#,
    );
}

#[test]
fn an_even_constant_multiplier_has_fixed_zero_trailing_bits() {
    assert_intentional_false_positive(
        "multiplication by twelve makes the low two result bits constant zero",
        r#"
        module Top (
            o: output bit,
        ) {
            var value  : bit<8>;
            var product: bit<8>;
            assign product = value * 8'd12;
            assign value[0] = product[1];
            assign value[7:1] = 0;
            assign o = product[1];
        }
        "#,
    );
}

#[test]
fn the_second_bit_of_a_two_state_square_is_always_zero() {
    assert_intentional_false_positive(
        "a square is zero or one modulo four and therefore never sets bit one",
        r#"
        module Top (
            o: output bit,
        ) {
            var value : bit<8>;
            var square: bit<8>;
            assign square = value * value;
            assign value[0] = square[1];
            assign value[7:1] = 0;
            assign o = square[1];
        }
        "#,
    );
}

#[test]
fn unsigned_division_by_a_power_of_two_discards_low_dividend_bits() {
    assert_intentional_false_positive(
        "unsigned division by eight is a right shift and discards the low three bits",
        r#"
        module Top (
            o: output bit,
        ) {
            var value   : bit<8>;
            var quotient: bit<8>;
            assign quotient = value / 8'd8;
            assign value[0] = quotient[0];
            assign value[7:1] = 0;
            assign o = quotient[0];
        }
        "#,
    );
}

#[test]
fn unsigned_remainder_by_a_power_of_two_ignores_high_dividend_bits() {
    assert_intentional_false_positive(
        "remainder by eight retains only the low three dividend bits",
        r#"
        module Top (
            o: output bit,
        ) {
            var value    : bit<8>;
            var remainder: bit<8>;
            assign remainder = value % 8'd8;
            assign value[7] = remainder[0];
            assign value[6:0] = 0;
            assign o = remainder[0];
        }
        "#,
    );
}

// Constants can eliminate individual data or predicate dependencies even when
// an operand remains structurally present in the expression.

#[test]
fn a_constant_or_mask_does_not_retain_the_forced_input_bit() {
    assert_intentional_false_positive(
        "an OR-mask one makes the corresponding result bit independent of the input",
        r#"
        module Top (
            o: output bit,
        ) {
            var value : bit<8>;
            var masked: bit<8>;
            assign masked = value | 8'b00000001;
            assign value[0] = masked[0];
            assign value[7:1] = 0;
            assign o = masked[0];
        }
        "#,
    );
}

#[test]
fn a_false_right_operand_annihilates_a_logical_and_result() {
    assert_intentional_false_positive(
        "a side-effect-free value AND false is independent of the value",
        r#"
        module Top (
            o: output bit,
        ) {
            var value: bit;
            assign o = value && 1'b0;
            assign value = o;
        }
        "#,
    );
}

#[test]
fn a_zero_bit_annihilates_an_and_reduction() {
    assert_intentional_false_positive(
        "an AND reduction containing zero is independent of its other operand bits",
        r#"
        module Top (
            o: output bit,
        ) {
            var value: bit;
            assign o = &{value, 1'b0};
            assign value = o;
        }
        "#,
    );
}

#[test]
fn an_aligned_unsigned_threshold_comparison_ignores_low_operand_bits() {
    assert_intentional_false_positive(
        "an eight-bit value is below 128 exactly when its high bit is zero",
        r#"
        module Top (
            o: output bit,
        ) {
            var value: bit<8>;
            var less : bit;
            assign less = value <: 8'd128;
            assign value[0] = less;
            assign value[7:1] = 0;
            assign o = less;
        }
        "#,
    );
}

#[test]
fn a_signed_comparison_with_zero_ignores_non_sign_bits() {
    assert_intentional_false_positive(
        "a signed value is below zero exactly when its sign bit is one",
        r#"
        module Top (
            o: output bit,
        ) {
            var value   : bit<8>;
            var negative: bit;
            assign negative = $signed(value) <: 0;
            assign value[0] = negative;
            assign value[7:1] = 0;
            assign o = negative;
        }
        "#,
    );
}

#[test]
fn wildcard_comparison_ignores_signal_bits_masked_by_the_pattern() {
    assert_intentional_false_positive(
        "X bits in a constant wildcard pattern do not inspect the corresponding signal bits",
        r#"
        module Top (
            o: output logic,
        ) {
            var value: logic<8>;
            var equal: logic;
            assign equal = value ==? 8'bxxxx0000;
            assign value[7] = equal;
            assign value[6:0] = 0;
            assign o = equal;
        }
        "#,
    );
}

// Runtime mappings are conservatively widened when their reachable bit range
// cannot be represented by one fixed positional offset.

#[test]
fn a_dynamic_left_shift_cannot_move_a_high_input_bit_to_the_low_result_bit() {
    assert_intentional_false_positive(
        "a left shift selects a lower source bit or zero for the low result bit",
        r#"
        module Top (
            amount: input  bit<3>,
            o     : output bit,
        ) {
            var value  : bit<8>;
            var shifted: bit<8>;
            assign shifted = value << amount;
            assign value[7] = shifted[0];
            assign value[6:0] = 0;
            assign o = shifted[0];
        }
        "#,
    );
}

#[test]
fn a_dynamic_right_shift_cannot_move_a_low_input_bit_to_the_high_result_bit() {
    assert_intentional_false_positive(
        "a right shift selects a higher source bit or zero for the high result bit",
        r#"
        module Top (
            amount: input  bit<3>,
            o     : output bit,
        ) {
            var value  : bit<8>;
            var shifted: bit<8>;
            assign shifted = value >> amount;
            assign value[0] = shifted[7];
            assign value[7:1] = 0;
            assign o = shifted[7];
        }
        "#,
    );
}

#[test]
fn a_ternary_condition_does_not_affect_a_result_bit_shared_by_both_arms() {
    assert_intentional_false_positive(
        "the low result bit is zero in both arms even though the complete arm values differ",
        r#"
        module Top (
            o: output bit,
        ) {
            var select: bit;
            var value : bit<2>;
            assign value = if select ? 2'b10 : 2'b00;
            assign select = value[0];
            assign o = value[0];
        }
        "#,
    );
}

#[test]
fn a_one_bit_dynamic_selector_cannot_reach_the_high_vector_bits() {
    assert_intentional_false_positive(
        "a one-bit index reaches only vector bits zero and one",
        r#"
        module Top (
            index: input  bit,
            o    : output bit,
        ) {
            var value: bit<4>;
            assign o = value[index];
            assign value[3] = o;
            assign value[2:0] = 0;
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
            sel: input  bit,
            o  : output bit,
        ) {
            var a: bit;
            var b: bit;
            assign a = if sel ? b : 0;
            assign b = if !sel ? a : 0;
            assign o = a | b;
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
