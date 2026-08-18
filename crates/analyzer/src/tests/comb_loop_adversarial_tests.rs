use super::*;

fn assert_comb_loop(case: &str, code: &str, expected: bool) {
    let errors = analyze(code);
    let actual = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert_eq!(actual, expected, "{case}: {errors:?}");
}

#[test]
fn comb_loop_false_negative_constant_dead_logical_rhs_overwrites_feedback() {
    assert_comb_loop(
        "a function in a constant-dead logical RHS cannot overwrite feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            var a    : logic;
            var b    : logic;
            var dummy: logic;
            function clear_a () -> logic {
                a = 1'b0;
                return 1'b0;
            }
            always_comb {
                a = b;
                dummy = 1'b0 && clear_a();
                b = a;
                o = a | b | dummy;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_false_positive_constant_dead_logical_rhs_creates_feedback() {
    assert_comb_loop(
        "a function in a constant-dead logical RHS cannot create feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            var a    : logic;
            var b    : logic;
            var dummy: logic;
            function write_a (value: input logic) -> logic {
                a = value;
                return 1'b0;
            }
            always_comb {
                a = 1'b0;
                dummy = 1'b0 && write_a(b);
                b = a;
                o = a | b | dummy;
            }
        }
        "#,
        false,
    );
}

#[test]
fn array_literal_default_does_not_contaminate_an_explicit_element() {
    assert_comb_loop(
        "an array literal default does not contaminate its explicit element",
        r#"
        module Top (
            o: output logic,
        ) {
            var source: logic [4];
            var built : logic [4];
            assign built = '{source[0], default: 1'b0};
            assign source[0] = built[3];
            assign source[1] = 0;
            assign source[2] = 0;
            assign source[3] = 0;
            assign o = built[0];
        }
        "#,
        false,
    );
}

#[test]
fn array_literal_explicit_element_retains_feedback() {
    assert_comb_loop(
        "an array literal retains its explicit element",
        r#"
        module Top (
            o: output logic,
        ) {
            var source: logic [4];
            var built : logic [4];
            assign built = '{source[0], default: 1'b0};
            assign source[0] = built[0];
            assign source[1] = 0;
            assign source[2] = 0;
            assign source[3] = 0;
            assign o = built[3];
        }
        "#,
        true,
    );
}

#[test]
fn multidimensional_array_literal_keeps_flattened_elements_disjoint() {
    assert_comb_loop(
        "a nested array literal keeps distinct flattened elements disjoint",
        r#"
        module Top (
            o: output logic,
        ) {
            var source: logic [2, 2];
            var built : logic [2, 2];
            assign built = '{'{source[0][0], 1'b0}, '{1'b0, 1'b0}};
            assign source[0][0] = built[1][1];
            assign source[0][1] = 0;
            assign source[1] = '{default: 0};
            assign o = built[0][0];
        }
        "#,
        false,
    );
}

#[test]
fn multidimensional_array_literal_retains_matching_element_feedback() {
    assert_comb_loop(
        "a nested array literal retains matching flattened element feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            var source: logic [2, 2];
            var built : logic [2, 2];
            assign built = '{'{source[0][0], 1'b0}, '{1'b0, 1'b0}};
            assign source[0][0] = built[0][0];
            assign source[0][1] = 0;
            assign source[1] = '{default: 0};
            assign o = built[1][1];
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_false_positive_function_array_return_widens_elements() {
    assert_comb_loop(
        "selecting one unpacked function result element does not read its siblings",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function identity (x: input Pair) -> Pair {
                return x;
            }
            var source  : logic [2];
            var returned: logic [2];
            assign returned = identity(source);
            assign source[0] = returned[1];
            assign source[1] = 0;
            assign o = returned[0];
        }
        "#,
        false,
    );
}

#[test]
fn function_array_return_retains_matching_element_feedback() {
    assert_comb_loop(
        "selecting an unpacked function result element retains its matching source",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function identity (x: input Pair) -> Pair {
                return x;
            }
            var source  : logic [2];
            var returned: logic [2];
            assign returned = identity(source);
            assign source[0] = returned[0];
            assign source[1] = 0;
            assign o = returned[1];
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_false_positive_function_array_output_widens_elements() {
    assert_comb_loop(
        "an unpacked function output preserves element identity",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function copy (
                x: input  Pair,
                y: output Pair,
            ) {
                y = x;
            }
            var source  : Pair;
            var returned: Pair;
            always_comb {
                copy(source, returned);
            }
            assign source[0] = returned[1];
            assign source[1] = 0;
            assign o = returned[0];
        }
        "#,
        false,
    );
}

#[test]
fn function_array_output_retains_matching_element_feedback() {
    assert_comb_loop(
        "an unpacked function output retains matching element feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function copy (
                x: input  Pair,
                y: output Pair,
            ) {
                y = x;
            }
            var source  : Pair;
            var returned: Pair;
            always_comb {
                copy(source, returned);
            }
            assign source[0] = returned[0];
            assign source[1] = 0;
            assign o = returned[1];
        }
        "#,
        true,
    );
}

#[test]
fn overlapping_union_members_retain_feedback() {
    assert_comb_loop(
        "overlapping packed union members denote the same storage",
        r#"
        module Top (
            o: output logic,
        ) {
            union Overlay {
                bits: logic<2>,
                low : logic,
            }
            var value: Overlay;
            assign value.bits[0] = value.low;
            assign value.bits[1] = 0;
            assign o = value.low;
        }
        "#,
        true,
    );
}

#[test]
fn seeded_compound_assignment_is_feed_forward() {
    assert_comb_loop(
        "a compound assignment reads the immediately preceding definition",
        r#"
        module Top (
            i: input  logic,
            o: output logic,
        ) {
            var value: logic;
            always_comb {
                value = 0;
                value |= i;
                o = value;
            }
        }
        "#,
        false,
    );
}

#[test]
fn unseeded_compound_assignment_retains_self_feedback() {
    assert_comb_loop(
        "a compound assignment explicitly reads its old destination",
        r#"
        module Top (
            i: input  logic,
            o: output logic,
        ) {
            always_comb {
                o |= i;
            }
        }
        "#,
        true,
    );
}

#[test]
fn switch_statement_arms_are_path_exclusive() {
    assert_comb_loop(
        "opposing dependencies in switch arms cannot execute together",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                switch {
                    sel    : a = b;
                    !sel   : b = a;
                    default: {
                        a = 0;
                        b = 0;
                    }
                }
                o = a | b;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_false_positive_nested_function_array_identity_widens_elements() {
    assert_comb_loop(
        "nested unpacked-array identity calls preserve element identity",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function identity (x: input Pair) -> Pair {
                return x;
            }
            function nested (x: input Pair) -> Pair {
                return identity(identity(x));
            }
            var source  : Pair;
            var returned: Pair;
            assign returned = nested(source);
            assign source[0] = returned[1];
            assign source[1] = 0;
            assign o = returned[0];
        }
        "#,
        false,
    );
}

#[test]
fn nested_function_array_identity_retains_matching_feedback() {
    assert_comb_loop(
        "nested unpacked-array identity calls retain matching-element feedback",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function identity (x: input Pair) -> Pair {
                return x;
            }
            function nested (x: input Pair) -> Pair {
                return identity(identity(x));
            }
            var source  : Pair;
            var returned: Pair;
            assign returned = nested(source);
            assign source[0] = returned[0];
            assign source[1] = 0;
            assign o = returned[1];
        }
        "#,
        true,
    );
}

#[test]
fn function_array_permutation_keeps_distinct_elements_disjoint() {
    assert_comb_loop(
        "an unpacked-array function permutation preserves the source element mapping",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function swap (x: input Pair) -> Pair {
                return '{x[1], x[0]};
            }
            var source  : Pair;
            var returned: Pair;
            assign returned = swap(source);
            assign source[0] = returned[0];
            assign source[1] = 0;
            assign o = returned[1];
        }
        "#,
        false,
    );
}

#[test]
fn function_array_permutation_retains_mapped_feedback() {
    assert_comb_loop(
        "an unpacked-array function permutation retains feedback through the mapped element",
        r#"
        module Top (
            o: output logic,
        ) {
            type Pair = logic [2];
            function swap (x: input Pair) -> Pair {
                return '{x[1], x[0]};
            }
            var source  : Pair;
            var returned: Pair;
            assign returned = swap(source);
            assign source[0] = returned[1];
            assign source[1] = 0;
            assign o = returned[0];
        }
        "#,
        true,
    );
}

#[test]
fn runtime_loop_forward_array_chain_is_not_feedback() {
    assert_comb_loop(
        "each runtime-loop iteration copies a lower element into a higher element",
        r#"
        module Top (
            n: input  logic<2>,
            i: input  logic,
            o: output logic,
        ) {
            var value: logic [4];
            always_comb {
                value = '{default: 0};
                value[0] = i;
                for index in 0..n {
                    value[index + 1] = value[index];
                }
                o = value[3];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_false_negative_runtime_loop_requires_multiple_iterations() {
    let code = r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [4];
            var feedback: logic;
            assign feedback = value[3];
            always_comb {
                value = '{default: 0};
                value[0] = feedback;
                for index in 0..n {
                    value[index + 1] = value[index];
                }
                o = feedback;
            }
        }
        "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_comb_loop(
        "runtime-loop iterations can carry a dependency from element zero to element three",
        code,
        true,
    );
}

#[test]
fn comb_loop_runtime_loop_propagates_through_an_unsplit_middle_partition() {
    assert_comb_loop(
        "a runtime loop propagates through every iteration inside a sparse middle partition",
        r#"
        module Top (
            n: input  logic<3>,
            o: output logic,
        ) {
            var value   : logic [5];
            var feedback: logic;
            assign feedback = value[4];
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
        true,
    );
}

#[test]
fn comb_loop_runtime_loop_propagates_between_unsplit_partitions() {
    assert_comb_loop(
        "runtime-loop iterations compose dependencies between sparse middle partitions",
        r#"
        module Top (
            n: input  logic<3>,
            o: output logic,
        ) {
            var a       : logic [5];
            var b       : logic [5];
            var feedback: logic;
            assign feedback = a[4];
            always_comb {
                a = '{default: 0};
                b = '{default: 0};
                a[0] = feedback;
                for index in 0..n {
                    a[index + 1] = b[index];
                    b[index + 1] = a[index];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_runtime_loop_respects_a_dynamic_start_lower_bound() {
    assert_comb_loop(
        "runtime-loop splitting cannot execute indices below a dynamic start expression",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [5];
            var feedback: logic;
            assign feedback = value[4];
            always_comb {
                value = '{default: 0};
                value[1] = feedback;
                for index in (n + 1)..4 {
                    value[index + 1] = value[index];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn runtime_loop_with_identical_exclusive_bounds_is_empty() {
    assert_comb_loop(
        "an exclusive runtime range with identical bounds has no body path",
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
                for index in n..n {
                    value[index + 1] = value[index];
                }
                o = feedback;
            }
        }
        "#,
        false,
    );
}

#[test]
fn runtime_loop_with_two_dynamic_bounds_retains_a_feasible_feedback_path() {
    assert_comb_loop(
        "dynamic start and end cuts retain a feasible ordered iteration sequence",
        r#"
        module Top (
            start: input  logic<3>,
            stop : input  logic<3>,
            o    : output logic,
        ) {
            var value   : logic [5];
            var feedback: logic;
            assign feedback = value[4];
            always_comb {
                value = '{default: 0};
                value[1] = feedback;
                for index in start..stop {
                    value[index + 1] = value[index];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_runtime_reverse_loop_requires_multiple_iterations() {
    assert_comb_loop(
        "a descending runtime loop carries a dependency from the high element to the low element",
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
                value[3] = feedback;
                for index in rev 0..n {
                    value[index] = value[index + 1];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_multidimensional_runtime_loop_uses_flattened_stride() {
    assert_comb_loop(
        "a runtime loop carries dependencies between rows of a multidimensional array",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [4, 2];
            var feedback: logic;
            assign feedback = value[3][0];
            always_comb {
                value = '{default: 0};
                value[0][0] = feedback;
                for index in 0..n {
                    value[index + 1][0] = value[index][0];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_runtime_loop_carries_dependencies_across_body_statements() {
    assert_comb_loop(
        "runtime-loop iterations compose dependencies across ordered body statements",
        r#"
        module Top (
            n: input  logic<2>,
            o: output logic,
        ) {
            var a       : logic [3];
            var b       : logic [3];
            var feedback: logic;
            assign feedback = a[2];
            always_comb {
                a = '{default: 0};
                b = '{default: 0};
                a[0] = feedback;
                for index in 0..n {
                    a[index + 1] = b[index];
                    b[index + 1] = a[index];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_runtime_loop_with_modulo_index_may_wrap_to_its_seed() {
    assert_comb_loop(
        "a runtime loop with a modulo index may carry feedback around the array",
        r#"
        module Top (
            n: input  logic<3>,
            o: output logic,
        ) {
            var value   : logic [4];
            var feedback: logic;
            assign feedback = value[0];
            always_comb {
                value = '{default: 0};
                value[1] = feedback;
                for index in 1..n {
                    value[(index + 1) % 4] = value[index % 4];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_runtime_loop_does_not_sample_modulo_access_at_affine_boundaries_only() {
    assert_comb_loop(
        "an unrelated affine access cannot hide intermediate modulo iterations",
        r#"
        module Top (
            n: input  logic<4>,
            o: output logic,
        ) {
            var value   : logic [4];
            var scratch : logic [8];
            var feedback: logic;
            assign feedback = value[0];
            always_comb {
                value = '{default: 0};
                scratch = '{default: 0};
                value[1] = feedback;
                for index in 1..n {
                    value[(index + 1) % 4] = value[index % 4];
                    scratch[index] = 1'b1;
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_runtime_iterator_and_other_dynamic_index_share_an_lsp() {
    assert_comb_loop(
        "the loop iterator and another dynamic index may address the same LSP region",
        r#"
        module Top (
            n: input  logic<2>,
            j: input  logic<2>,
            o: output logic,
        ) {
            var value   : logic [4];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {
                value = '{default: 0};
                value[0] = feedback;
                for i in 0..n {
                    value[j] = value[i];
                }
                o = feedback;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_dynamic_non_additive_step_retains_possible_feedback() {
    // This is not an intentional false positive: when (n | 1) is below ten,
    // the loop executes and assigns b from a, so the feedback is realizable.
    let code = r#"
        module Top (
            n: input  logic<4>,
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            assign a = b;
            always_comb {
                b = 0;
                for _index in (n | 1)..10 step *= 2 {
                    b = a;
                }
                o = b;
            }
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert_comb_loop(
        "a non-additive runtime step retains its realizable body feedback",
        code,
        true,
    );
}

#[test]
fn comb_loop_runtime_iterator_conditions_retain_reachable_iterations() {
    assert_comb_loop(
        "runtime iterator conditions retain a feedback path across iterations",
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
                for index in 0..n {
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
        true,
    );
}

#[test]
fn constant_dead_function_in_destination_index_does_not_create_feedback() {
    assert_comb_loop(
        "a function in a constant-dead destination index operand is not evaluated",
        r#"
        module Top (o: output logic) {
            var a: logic;
            var b: logic;
            var value: logic [2];
            function write_a (x: input logic) -> logic {
                a = x;
                return 0;
            }
            always_comb {
                a = 0;
                value[1'b0 && write_a(b)] = 0;
                b = a;
                o = b | value[0];
            }
        }
        "#,
        false,
    );
}

#[test]
fn constant_dead_function_in_source_index_does_not_kill_feedback() {
    assert_comb_loop(
        "a function in a constant-dead source index operand cannot overwrite feedback",
        r#"
        module Top (o: output logic) {
            var a: logic;
            var b: logic;
            var dummy: logic;
            var value: logic [2];
            function clear_a () -> logic {
                a = 0;
                return 0;
            }
            always_comb {
                a = b;
                value = '{default: 0};
                dummy = value[1'b0 && clear_a()];
                b = a;
                o = b | dummy;
            }
        }
        "#,
        true,
    );
}

#[test]
fn zero_offset_cycle_is_not_hidden_by_parallel_shifted_paths() {
    assert_comb_loop(
        "parallel shifted dependencies cannot hide the same-bit cycle",
        r#"
        module Top (o: output logic<3>) {
            var a: logic<3>;
            var b: logic<3>;
            var c: logic<3>;
            var d: logic<3>;
            assign a = b | (b << 1) | (b << 2);
            assign d = a | (a << 1) | (a << 2);
            assign c = d | (d << 1) | (d << 2);
            assign b = c | (c << 1) | (c << 2);
            assign o = a;
        }
        "#,
        true,
    );
}

#[test]
fn dynamic_packed_select_stays_within_its_struct_member() {
    assert_comb_loop(
        "a dynamic packed select cannot alias a disjoint struct member",
        r#"
        module Top (
            index: input  bit<2>,
            o    : output bit,
        ) {
            struct Pair {
                selected: bit<4>,
                result  : bit<4>,
            }
            var left : Pair;
            var right: Pair;
            assign left = Pair'{
                selected: 0,
                result  : right.result[0],
            };
            assign right = Pair'{
                selected: 0,
                result  : left.selected[index],
            };
            assign o = right.result[0];
        }
        "#,
        false,
    );
}

#[test]
fn dynamic_packed_select_stays_within_a_struct_member_nested_in_a_union() {
    assert_comb_loop(
        "a dynamic packed select cannot escape a nested member through a union overlay",
        r#"
        module Top (
            index: input  bit<2>,
            o    : output bit,
        ) {
            struct Pair {
                selected: bit<4>,
                result  : bit<4>,
            }
            union Overlay {
                pair: Pair,
                raw : bit<8>,
            }
            var left : Overlay;
            var right: Overlay;
            assign left.pair = Pair'{
                selected: 0,
                result  : right.pair.result[0],
            };
            assign right.pair = Pair'{
                selected: 0,
                result  : left.pair.selected[index],
            };
            assign o = right.pair.result[0];
        }
        "#,
        false,
    );
}
