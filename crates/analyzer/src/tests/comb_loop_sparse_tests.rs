use super::*;

fn assert_comb_loop(case: &str, code: &str, expected: bool) {
    let errors = analyze(code);
    let actual = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert_eq!(actual, expected, "{case}: {errors:?}");
}

macro_rules! comb_loop_case {
    ($name:ident, $case:literal, $code:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let code = $code;
            assert_comb_loop($case, code.as_ref(), $expected);
        }
    };
}

fn structural_selector_overlap_code(write_index: &str) -> String {
    format!(
        r#"
        module Top (idx: input bit, o: output logic) {{
            var value: logic<2>;
            assign o = value[idx];
            assign value[{write_index}] = o;
        }}
        "#
    )
}

fn assert_multiplied_dynamic_index_overlap(target: usize) {
    let clears = (0..3)
        .filter(|index| *index != target)
        .map(|index| format!("assign mem[{index}] = 0;"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_comb_loop(
        "a multiplied dynamic index structurally overlaps its target",
        &format!(
            r#"
            module Top (idx: input bit, o: output logic) {{
                var mem: logic [3];
                assign o = mem[idx * 2];
                assign mem[{target}] = o;
                {clears}
            }}
            "#
        ),
        true,
    );
}

comb_loop_case!(
    comb_loop_complementary_selectors_retain_structural_overlap,
    "complementary selectors retain structural overlap",
    structural_selector_overlap_code("~idx"),
    true
);

comb_loop_case!(
    comb_loop_identical_selectors_retain_structural_overlap,
    "identical selectors retain structural overlap",
    structural_selector_overlap_code("idx"),
    true
);

#[test]
fn comb_loop_oversized_array_is_sparse_and_complete_why_this_case_exists_a_sequential_memory_with_a_combinational_read_must()
 {
    // Why this case exists: a sequential memory with a combinational read must
    // remain loop-free even when its declared index space is too large for the
    // analyzer's ordinary elaboration limit.
    let code = r#"
    module ModuleA (
        clk:  input  clock,
        rst:  input  reset,
        idx:  input  logic<23>,
        wd:   input  logic<32>,
        rd:   output logic<32>,
    ) {
        var mem: logic<32> [8388608];
        always_ff {
            if_reset {
                mem[idx][7:0] = 0;
            } else {
                mem[idx][7:0]   = wd[7:0];
                mem[idx][15:8]  = wd[15:8];
                mem[idx][23:16] = wd[23:16];
                mem[idx][31:24] = wd[31:24];
            }
        }
        assign rd = mem[idx];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "oversized dynamic-index array must not be flagged as a comb loop",
    );
}

#[test]
fn comb_loop_oversized_array_is_sparse_and_complete_why_this_case_exists_the_former_size_guard_silently_discarded_every()
 {
    // Why this case exists: the former size guard silently discarded every
    // edge for arrays above 64K elements. A dynamic write can alias the exact
    // element it reads, so the same sparse object-level analysis must still
    // prove this real feedback path without enumerating 128K elements.
    let code = r#"
    module ModuleA (
        idx: input  logic<17>,
        rd:  output logic<32>,
    ) {
        var mem: logic<32> [131072];
        always_comb {
            mem[idx] = mem[0];
            rd = mem[idx];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "oversized arrays must retain proven dynamic/exact alias loops: {errors:#?}",
    );
}

#[test]
fn comb_loop_oversized_array_is_sparse_and_complete_why_this_case_exists_object_level_aliasing_must_not_invent_a_cycle_when()
 {
    // Why this case exists: object-level aliasing must not invent a cycle when
    // the dynamic store is fed only by an external input.
    let code = r#"
    module ModuleA (
        idx: input  logic<17>,
        wd:  input  logic<32>,
        rd:  output logic<32>,
    ) {
        var mem: logic<32> [131072];
        always_comb {
            mem[idx] = wd;
            rd = mem[idx];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "oversized feed-forward dynamic stores must remain loop-free: {errors:#?}",
    );
}

#[test]
fn comb_loop_oversized_array_is_sparse_and_complete_why_this_case_exists_a_dynamic_read_makes_an_object_uncertain_but_does()
 {
    // Why this case exists: a dynamic read makes an object uncertain but does
    // not turn a separate exact write into a dynamic write. Conflating the two
    // would alias mem[1] back onto mem[0] and invent a self-loop.
    let code = r#"
    module ModuleA (
        idx: input  logic<17>,
        rd:  output logic<32>,
    ) {
        var mem: logic<32> [131072];
        always_comb {
            rd = mem[idx];
            mem[1] = mem[0];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "a dynamic read must not broaden an exact write: {errors:#?}",
    );
}

#[test]
fn comb_loop_aliases_overlapping_unknown_regions_overlapping_dynamic_prefixes_retain_realizable_feedback()
 {
    assert_comb_loop(
        "overlapping dynamic prefixes retain realizable feedback",
        r#"
        module Top (
            i: input  logic,
            j: input  logic,
            k: input  logic,
            o: output logic,
        ) {
            var mem: logic [2, 2];
            var feedback: logic;
            always_comb {
                mem[i][j] = feedback;
            }
            always_comb {
                feedback = mem[0][k];
            }
            assign o = feedback;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_aliases_overlapping_unknown_regions_disjoint_dynamic_prefixes_remain_independent() {
    assert_comb_loop(
        "disjoint dynamic prefixes remain independent",
        r#"
        module Top (
            j: input  logic,
            k: input  logic,
            o: output logic,
        ) {
            var mem: logic [2, 2];
            var feedback: logic;
            always_comb {
                mem[1][j] = feedback;
            }
            always_comb {
                feedback = mem[0][k];
            }
            assign o = feedback;
        }
        "#,
        false,
    );
}

#[test]
#[ignore = "comb-loop migration: false positive; sparse and dynamic regions"]
fn comb_loop_alias_and_opaque_effect_boundaries_local_copy_chains_propagate_bit_identity_through_every_hop()
 {
    assert_comb_loop(
        "local copy chains propagate bit identity through every hop",
        r#"
        module Top (
            o: output logic,
        ) {
            function bit_two (x: input logic<8>) -> logic {
                var first : logic<8>;
                var second: logic<8>;
                first = x;
                second = first;
                return second[2];
            }
            var value: logic<8>;
            assign o = bit_two(value);
            assign value[7] = o;
            assign value[6:0] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_local_concatenation_retains_a_signal_low_bit() {
    assert_comb_loop(
        "local concatenation retains a signal low bit",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (x: input logic) -> logic {
                var tmp: logic<8>;
                tmp = {7'b0, x};
                return tmp[0];
            }
            assign o = low(o);
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_same_width_bitwise_operators_retain_same_bit_feedback()
 {
    assert_comb_loop(
        "same-width bitwise operators retain same-bit feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (x: input logic<8>) -> logic {
                var tmp: logic<8>;
                tmp = x | 8'b00000000;
                return tmp[0];
            }
            var value: logic<8>;
            assign o = low(value);
            assign value[0] = o;
            assign value[7:1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_structural_dependence_is_not_removed_by_boolean_cancellation()
 {
    assert_comb_loop(
        "structural dependence is not removed by Boolean cancellation",
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
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_reduction_operators_remain_dependent_on_every_operand_bit()
 {
    assert_comb_loop(
        "reduction operators remain dependent on every operand bit",
        r#"
        module Top (
            o: output logic,
        ) {
            function parity (x: input logic<8>) -> logic {
                return ^x;
            }
            var value: logic<8>;
            assign o = parity(value);
            assign value[7] = o;
            assign value[6:0] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_four_state_arithmetic_depends_on_every_operand_bit()
{
    assert_comb_loop(
        "four-state arithmetic depends on every operand bit",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (x: input logic<8>) -> logic {
                var tmp: logic<8>;
                tmp = x + 8'd1;
                return tmp[0];
            }
            var value: logic<8>;
            assign o = low(value);
            assign value[7] = o;
            assign value[6:0] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_dynamic_observer_must_not_hide_an_unrelated_proven_loop()
 {
    assert_comb_loop(
        "dynamic observer must not hide an unrelated proven loop",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic,
        ) {
            var observed: logic<4>;
            always_comb {
                $display("observed=%d", observed[index]);
                o = o;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_dynamic_write_to_one_object_must_not_hide_another_object_s_loop()
 {
    assert_comb_loop(
        "dynamic write to one object must not hide another object's loop",
        r#"
        module Top (
            index: input  logic<2>,
            data : input  logic,
            o    : output logic,
        ) {
            var memory: logic [4];
            always_comb {
                memory[index] = data;
                o = o;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_dominating_full_write_kills_partial_branch_feedback()
 {
    assert_comb_loop(
        "dominating full write kills partial branch feedback",
        r#"
        module Top (
            sel: input  logic,
            a  : input  logic,
            o  : output logic<2>,
        ) {
            always_comb {
                o = 0;
                if sel {
                    o[0] = a;
                }
                o[1] = o[0];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_phi_on_one_bit_cannot_manufacture_a_cross_bit_cycle()
 {
    assert_comb_loop(
        "phi on one bit cannot manufacture a cross-bit cycle",
        r#"
        module Top (
            sel: input  logic,
            a  : input  logic,
            b  : input  logic,
            o  : output logic<2>,
        ) {
            always_comb {
                o = 0;
                if sel {
                    o[0] = a;
                } else {
                    o[1] = b;
                }
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_dynamic_writes_to_separate_objects_do_not_alias() {
    assert_comb_loop(
        "dynamic writes to separate objects do not alias",
        r#"
        module Top (
            left_index : input  logic<2>,
            right_index: input  logic<2>,
            data       : input  logic,
            o          : output logic,
        ) {
            var left : logic<4>;
            var right: logic<4>;
            always_comb {
                left[left_index] = data;
                right[right_index] = left[left_index];
                o = right[0];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_two_object_local_dynamic_aliases_can_close_a_cross_object_cycle()
 {
    assert_comb_loop(
        "two object-local dynamic aliases can close a cross-object cycle",
        r#"
        module Top (
            left_index : input  logic<2>,
            right_index: input  logic<2>,
            o          : output logic,
        ) {
            var left : logic<4>;
            var right: logic<4>;
            always_comb {
                left[left_index] = right[right_index];
                right[right_index] = left[left_index];
                o = right[0];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_dynamic_select_alias_domain_a_dynamic_select_aliases_its_whole_longest_static_prefix()
{
    assert_comb_loop(
        "a dynamic select aliases its whole longest static prefix",
        r#"
        module Top (
            index: input  logic,
            o    : output logic<4>,
        ) {
            always_comb {
                o[1:0] = 0;
                o[index] = o[3];
                o[3] = o[2];
                o[2] = 0;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_dynamic_select_alias_domain_a_one_bit_dynamic_domain_still_aliases_both_representable_bits()
 {
    assert_comb_loop(
        "a one-bit dynamic domain still aliases both representable bits",
        r#"
        module Top (
            index: input  logic,
            o    : output logic<4>,
        ) {
            always_comb {
                o[index] = o[1];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_dynamic_select_alias_domain_a_representable_dynamic_target_can_close_a_cycle_through_a_high_bit()
 {
    assert_comb_loop(
        "a representable dynamic target can close a cycle through a high bit",
        r#"
        module Top (
            index: input  logic,
            o    : output logic<4>,
        ) {
            always_comb {
                o[index] = o[3];
                o[3] = o[0];
            }
        }
        "#,
        true,
    );
}

fn assert_structural_dynamic_selector_element(target: usize) {
    let clears = match target {
        0 => "assign value[1] = 0; assign value[2] = 0; assign value[3] = 0;",
        2 => "assign value[0] = 0; assign value[1] = 0; assign value[3] = 0;",
        _ => unreachable!(),
    };
    assert_comb_loop(
        "a shifted dynamic index structurally overlaps its target",
        &format!(
            r#"
            module Top (idx: input bit, o: output logic) {{
                var value: logic [4];
                assign o = value[idx + 2];
                assign value[{target}] = o;
                {clears}
            }}
            "#
        ),
        true,
    );
}

fn assert_structural_dynamic_part_select_bit(target: usize) {
    let clears = match target {
        0 => "assign value[1] = 0; assign value[2] = 0; assign value[3] = 0;",
        3 => "assign value[0] = 0; assign value[1] = 0; assign value[2] = 0;",
        _ => unreachable!(),
    };
    assert_comb_loop(
        "a dynamic indexed part-select structurally overlaps its target",
        &format!(
            r#"
            module Top (idx: input bit, o: output logic<2>) {{
                var value: logic<4>;
                assign o = value[idx+:2];
                assign value[{target}] = o[0];
                {clears}
            }}
            "#
        ),
        true,
    );
}

#[test]
fn comb_loop_shifted_dynamic_index_overlaps_element_zero() {
    assert_structural_dynamic_selector_element(0);
}

#[test]
fn comb_loop_shifted_dynamic_index_overlaps_element_two() {
    assert_structural_dynamic_selector_element(2);
}

#[test]
fn comb_loop_dynamic_part_select_overlaps_bit_three() {
    assert_structural_dynamic_part_select_bit(3);
}

#[test]
fn comb_loop_dynamic_part_select_overlaps_bit_zero() {
    assert_structural_dynamic_part_select_bit(0);
}

#[test]
fn comb_loop_multiplied_dynamic_index_overlaps_element_one() {
    assert_multiplied_dynamic_index_overlap(1);
}

#[test]
fn comb_loop_multiplied_dynamic_index_overlaps_element_two() {
    assert_multiplied_dynamic_index_overlap(2);
}
