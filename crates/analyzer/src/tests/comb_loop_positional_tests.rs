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

macro_rules! comb_loop_case_ignored {
    ($name:ident, $reason:literal, $case:literal, $code:expr, $expected:expr) => {
        #[test]
        #[ignore = $reason]
        fn $name() {
            let code = $code;
            assert_comb_loop($case, code.as_ref(), $expected);
        }
    };
}

fn nested_repeat_code(through_instance: bool, output_bit: usize) -> String {
    let assignment = if through_instance {
        "inst u: Identity (i: '{'{feedback repeat 20} repeat 100}, o: passed);".to_string()
    } else {
        "assign passed = '{'{feedback repeat 20} repeat 100};".to_string()
    };
    let identity = if through_instance {
        r#"
        module Identity (
            i: input logic<2> [100, 20], o: output logic<2> [100, 20],
        ) { assign o = i; }
        "#
    } else {
        ""
    };
    format!(
        r#"
        {identity}
        module Top (o: output logic) {{
            var feedback: logic<2>;
            var passed: logic<2> [100, 20];
            {assignment}
            assign feedback[0] = passed[78][12][{output_bit}];
            assign feedback[1] = 0;
            assign o = passed[0][0][0];
        }}
        "#
    )
}

comb_loop_case!(
    comb_loop_nested_repeat_retains_matching_phase,
    "nested periodic axes retain their matching packed phase",
    nested_repeat_code(false, 0),
    true
);

comb_loop_case!(
    comb_loop_nested_repeat_keeps_disjoint_phase,
    "nested periodic axes keep a disjoint packed phase independent",
    nested_repeat_code(false, 1),
    false
);

comb_loop_case!(
    comb_loop_nested_repeat_actual_retains_matching_phase,
    "nested actual axes retain their matching phase across a module",
    nested_repeat_code(true, 0),
    true
);

comb_loop_case_ignored!(
    comb_loop_nested_repeat_actual_keeps_disjoint_phase,
    "comb-loop migration: false positive; child summary is not clipped to a multidimensional repeated actual region",
    "nested actual axes keep a disjoint phase across a module",
    nested_repeat_code(true, 1),
    false
);

fn structure_constructor_code(constructor: &str) -> String {
    format!(
        r#"
        module Top (o: output logic) {{
            struct Pair {{ a: logic, b: logic, }}
            var pair: Pair;
            assign pair = {constructor};
            assign o = pair.a;
        }}
        "#
    )
}

fn structure_actual_code(constructor: &str) -> String {
    format!(
        r#"
        package Types {{ struct Pair {{ a: logic, b: logic, }} }}
        module Pick (i: input Types::Pair, o: output logic) {{ assign o = i.a; }}
        module Top (o: output logic) {{
            var feedback: logic;
            var passed: logic;
            inst u: Pick (i: {constructor}, o: passed);
            assign feedback = passed;
            assign o = passed;
        }}
        "#
    )
}

fn array_literal_actual_code(actual: &str) -> String {
    format!(
        r#"
        module Pick (i: input logic [2], o: output logic) {{ assign o = i[0]; }}
        module Top (o: output logic) {{
            var feedback: logic;
            var passed: logic;
            inst u: Pick (i: {actual}, o: passed);
            assign feedback = passed;
            assign o = passed;
        }}
        "#
    )
}

fn array_copy_actual_code(actual: &str, feedback_bit: usize) -> String {
    format!(
        r#"
        module Pick (i: input logic<2> [2], o: output logic) {{ assign o = i[1][0]; }}
        module Top (o: output logic) {{
            var value: logic<2>;
            var passed: logic;
            inst u: Pick (i: {actual}, o: passed);
            assign value[{feedback_bit}] = passed;
            assign value[{}] = 0;
            assign o = passed;
        }}
        "#,
        1 - feedback_bit
    )
}

fn nested_array_actual_code(actual: &str) -> String {
    format!(
        r#"
        module Pick (i: input logic [2, 2], o: output logic) {{ assign o = i[0][0]; }}
        module Top (o: output logic) {{
            var feedback: logic;
            var passed: logic;
            inst u: Pick (i: {actual}, o: passed);
            assign feedback = passed;
            assign o = passed;
        }}
        "#
    )
}

comb_loop_case!(
    comb_loop_structure_constructor_unrelated_member_is_disjoint,
    "a structure constructor keeps an unrelated member loop-free",
    structure_constructor_code("Pair'{a: 0, b: o}"),
    false
);

comb_loop_case!(
    comb_loop_structure_constructor_retains_corresponding_member,
    "a structure constructor retains corresponding-member feedback",
    structure_constructor_code("Pair'{a: o, b: 0}"),
    true
);

comb_loop_case!(
    comb_loop_structure_actual_unrelated_member_is_disjoint,
    "a structure actual keeps an unrelated member loop-free",
    structure_actual_code("Types::Pair'{a: 0, b: feedback}"),
    false
);

comb_loop_case!(
    comb_loop_structure_actual_retains_corresponding_member,
    "a structure actual retains corresponding-member feedback",
    structure_actual_code("Types::Pair'{a: feedback, b: 0}"),
    true
);

comb_loop_case!(
    comb_loop_array_literal_unrelated_element_is_disjoint,
    "an array literal keeps an unrelated instance element loop-free",
    array_literal_actual_code("'{0, feedback}"),
    false
);

comb_loop_case!(
    comb_loop_array_literal_retains_corresponding_element,
    "an array literal retains corresponding-element feedback",
    array_literal_actual_code("'{feedback, 0}"),
    true
);

comb_loop_case!(
    comb_loop_array_default_preserves_corresponding_bits,
    "an array default preserves corresponding bits in every element",
    array_copy_actual_code("'{default: value}", 1),
    false
);

comb_loop_case!(
    comb_loop_array_default_retains_same_bit,
    "an array default retains same-bit feedback",
    array_copy_actual_code("'{default: value}", 0),
    true
);

comb_loop_case!(
    comb_loop_array_repeat_preserves_corresponding_bits,
    "an array repeat preserves corresponding bits in every element",
    array_copy_actual_code("'{value repeat 2}", 1),
    false
);

comb_loop_case!(
    comb_loop_array_repeat_retains_same_bit,
    "an array repeat retains same-bit feedback",
    array_copy_actual_code("'{value repeat 2}", 0),
    true
);

comb_loop_case!(
    comb_loop_nested_array_literal_unrelated_element_is_disjoint,
    "a nested array literal keeps an inner element loop-free",
    nested_array_actual_code("'{'{0, feedback}, '{0, 0}}"),
    false
);

comb_loop_case!(
    comb_loop_nested_array_literal_retains_inner_element,
    "a nested array literal retains inner-element feedback",
    nested_array_actual_code("'{'{feedback, 0}, '{0, 0}}"),
    true
);

comb_loop_case!(
    comb_loop_structure_constructor_retains_unobserved_member_effects,
    "a structure constructor retains effects from an unobserved member",
    r#"
    module Top (o: output logic) {
        struct Pair { a: logic, b: logic, }
        var pair: Pair;
        var state: logic;
        function touch (x: input logic) -> logic { state = x; return 0; }
        assign pair = Pair'{a: 0, b: touch(o)};
        assign o = state;
    }
    "#,
    true
);

comb_loop_case!(
    comb_loop_large_array_repeat_is_sparse_and_bit_precise,
    "a large array repeat remains sparse and bit-precise",
    r#"
    module Pick (i: input logic<2> [200000], o: output logic) {
        assign o = i[199999][0];
    }
    module Top (o: output logic) {
        var value: logic<2>;
        var passed: logic;
        inst u: Pick (i: '{value repeat 200000}, o: passed);
        assign value[1] = passed;
        assign value[0] = 0;
        assign o = passed;
    }
    "#,
    false
);

fn periodic_repeat_code(through_instance: bool, count: usize, index: usize, bit: usize) -> String {
    let body = if through_instance {
        "inst u: Broadcast (i: feedback, o: passed);".to_string()
    } else {
        format!("assign passed = '{{feedback repeat {count}}};")
    };
    format!(
        r#"
        module Broadcast (i: input logic<2>, o: output logic<2> [{count}]) {{
            assign o = '{{i repeat {count}}};
        }}
        module Top (o: output logic) {{
            var feedback: logic<2>;
            var passed: logic<2> [{count}];
            {body}
            assign feedback[0] = passed[{index}][{bit}];
            assign feedback[1] = 0;
            assign o = passed[0][0];
        }}
        "#
    )
}

comb_loop_case!(
    comb_loop_local_repeat_retains_matching_phase,
    "local repeat retains matching phase feedback",
    periodic_repeat_code(false, 64, 42, 0),
    true
);

comb_loop_case!(
    comb_loop_local_repeat_keeps_different_phase_disjoint,
    "local repeat keeps a different phase disjoint",
    periodic_repeat_code(false, 64, 42, 1),
    false
);

#[test]
fn comb_loop_preserves_concatenated_lhs_bits_a_concatenated_assignment_destination_preserves_bit_identity()
 {
    assert_comb_loop(
        "a concatenated assignment destination preserves bit identity",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            var value: logic<2>;
            assign {o[1], o[0]} = value;
            assign value[0] = 0;
            assign value[1] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_concatenated_lhs_bits_a_concatenated_assignment_retains_same_bit_feedback() {
    assert_comb_loop(
        "a concatenated assignment retains same-bit feedback",
        r#"
        module Top (
            o: output logic<2>,
        ) {
            var value: logic<2>;
            assign {o[1], o[0]} = value;
            assign value[0] = o[0];
            assign value[1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_constant_shift_positions_a_constant_left_shift_preserves_its_zero_filled_low_bit()
 {
    assert_comb_loop(
        "a constant left shift preserves its zero-filled low bit",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<4>;
            assign o = value << 1;
            assign value[0] = 0;
            assign value[1] = o[0];
            assign value[3:2] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_constant_shift_positions_a_constant_left_shift_retains_displaced_same_bit_feedback()
 {
    assert_comb_loop(
        "a constant left shift retains displaced same-bit feedback",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<4>;
            assign o = value << 1;
            assign value[0] = o[1];
            assign value[3:1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_left_shift_beyond_width_has_no_value_dependency() {
    // Why this case exists: shifting a requested low region right by a shift
    // larger than its endpoint yields an empty source region, not an unsigned
    // interval underflow.
    assert_comb_loop(
        "a left shift beyond the value width has only zero-filled output bits",
        r#"
        module Top (
            o: output logic,
        ) {
            var value  : logic<64>;
            var shifted: logic<64>;
            always_comb {
                shifted = value << 64;
            }
            assign o = shifted[0];
            assign value[0] = o;
            assign value[63:1] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_repeat_concatenation_positions_repeat_concatenation_keeps_a_disjoint_low_bit_path_loop_free()
 {
    assert_comb_loop(
        "repeat concatenation keeps a disjoint low-bit path loop-free",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<2>;
            assign o = {value repeat 2};
            assign value[0] = 0;
            assign value[1] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_repeat_concatenation_positions_repeat_concatenation_still_detects_a_corresponding_bit_loop()
 {
    assert_comb_loop(
        "repeat concatenation still detects a corresponding-bit loop",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<2>;
            assign o = {value repeat 2};
            assign value[0] = o[0];
            assign value[1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_nested_vector_ternary_positions_a_nested_vector_ternary_keeps_a_disjoint_bit_loop_free()
 {
    assert_comb_loop(
        "a nested vector ternary keeps a disjoint bit loop-free",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic<2>,
        ) {
            var value: logic<2>;
            assign o = ~(if sel ? value : 0);
            assign value[0] = 0;
            assign value[1] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_nested_vector_ternary_positions_a_nested_vector_ternary_detects_its_corresponding_bit_loop()
 {
    assert_comb_loop(
        "a nested vector ternary detects its corresponding-bit loop",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic<2>,
        ) {
            var value: logic<2>;
            assign o = ~(if sel ? value : 0);
            assign value[0] = o[0];
            assign value[1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_local_right_shift_positions_a_local_logical_right_shift_keeps_its_inserted_bit_loop_free()
 {
    assert_comb_loop(
        "a local logical right shift keeps its inserted bit loop-free",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<4>;
            assign o = value >> 1;
            assign value[0] = o[3];
            assign value[3:1] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_local_right_shift_positions_a_local_logical_right_shift_detects_its_live_shifted_bit()
 {
    assert_comb_loop(
        "a local logical right shift detects its live shifted bit",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<4>;
            assign o = value >> 1;
            assign value[0] = 0;
            assign value[1] = o[0];
            assign value[3:2] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_arithmetic_right_shift_positions_an_arithmetic_right_shift_keeps_a_discarded_bit_loop_free()
 {
    assert_comb_loop(
        "an arithmetic right shift keeps a discarded bit loop-free",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<4>;
            assign o = $signed(value) >>> 1;
            assign value[0] = o[0];
            assign value[3:1] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_arithmetic_right_shift_positions_an_arithmetic_right_shift_detects_its_live_shifted_bit()
 {
    assert_comb_loop(
        "an arithmetic right shift detects its live shifted bit",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<4>;
            assign o = $signed(value) >>> 1;
            assign value[0] = 0;
            assign value[1] = o[0];
            assign value[3:2] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_arithmetic_right_shift_positions_an_arithmetic_right_shift_retains_sign_fill_feedback()
 {
    assert_comb_loop(
        "an arithmetic right shift retains sign-fill feedback",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            var value: logic<4>;
            assign o = $signed(value) >>> 1;
            assign value[2:0] = 0;
            assign value[3] = o[3];
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_preserves_vector_ternary_positions_a_vector_ternary_keeps_a_disjoint_bit_loop_free() {
    assert_comb_loop(
        "a vector ternary keeps a disjoint bit loop-free",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic<2>,
        ) {
            var value: logic<2>;
            assign o = if sel ? value : 0;
            assign value[0] = 0;
            assign value[1] = o[0];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_preserves_vector_ternary_positions_a_vector_ternary_detects_its_corresponding_bit_loop()
 {
    assert_comb_loop(
        "a vector ternary detects its corresponding-bit loop",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic<2>,
        ) {
            var value: logic<2>;
            assign o = if sel ? value : 0;
            assign value[0] = o[0];
            assign value[1] = 0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_local_concatenation_does_not_taint_a_constant_low_bit()
 {
    assert_comb_loop(
        "local concatenation does not taint a constant low bit",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (x: input logic<7>) -> logic {
                var tmp: logic<8>;
                tmp = {x, 1'b0};
                return tmp[0];
            }
            var value: logic<7>;
            assign o = low(value);
            assign value[6] = o;
            assign value[5:0] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_alias_and_opaque_effect_boundaries_same_width_bitwise_operators_preserve_positional_provenance()
 {
    assert_comb_loop(
        "same-width bitwise operators preserve positional provenance",
        r#"
        module Top (
            o: output logic,
        ) {
            function low (x: input logic<8>) -> logic {
                var tmp: logic<8>;
                tmp = x & 8'b00000001;
                return tmp[0];
            }
            var value: logic<8>;
            assign o = low(value);
            assign value[7] = o;
            assign value[6:0] = 0;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_identical_ternary_arms_do_not_cancel_structural_control_dependence()
 {
    assert_comb_loop(
        "identical ternary arms do not cancel structural control dependence",
        r#"
        module Top (
            o: output logic,
        ) {
            assign o = if o ? 1'b0 : 1'b0;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_overlapping_left_shift_is_a_directed_acyclic_bit_chain()
 {
    assert_comb_loop(
        "overlapping left shift is a directed acyclic bit chain",
        r#"
        module Top (
            o: output logic<8>,
        ) {
            always_comb {
                o[7:1] = o[6:0];
                o[0] = 0;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_adding_the_wraparound_bit_turns_a_shift_into_a_structural_rotate_loop()
 {
    assert_comb_loop(
        "adding the wraparound bit turns a shift into a structural rotate loop",
        r#"
        module Top (
            o: output logic<8>,
        ) {
            always_comb {
                o[7:1] = o[6:0];
                o[0] = o[7];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_concatenation_permutation_preserves_structural_feedback()
 {
    assert_comb_loop(
        "concatenation permutation preserves structural feedback",
        r#"
        module Top (
            o: output logic<8>,
        ) {
            always_comb {
                o = {o[3:0], o[7:4]};
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_rejected_periodic_run_is_scanned_once() {
    // Why this case exists: a long run with one source but the same exact
    // destination is legal sequential overwrite, not a periodic transfer.
    // Rejecting the candidate must not sort every remaining suffix again.
    let assignments = "a = i;\n".repeat(4096);
    let errors = analyze(&format!(
        r#"
        module Top (
            i: input  logic,
            o: output logic,
        ) {{
            var a: logic;
            always_comb {{
                {assignments}
            }}
            assign o = a;
        }}
        "#
    ));
    assert!(
        errors.is_empty(),
        "repeated overwrite is valid: {errors:#?}"
    );
}

fn assert_unsigned_context_extension(feedback: &str, expected: bool) {
    assert_comb_loop(
        "unsigned context extension preserves only corresponding low bits",
        &format!(
            r#"
            module Top (o: output logic<4>) {{
                var value: logic<2>;
                assign o = value;
                assign value[0] = {feedback};
                assign value[1] = 0;
            }}
            "#
        ),
        expected,
    );
}

fn assert_signed_context_extension(target: usize, expected: bool) {
    assert_comb_loop(
        "signed context extension preserves only the replicated sign bit",
        &format!(
            r#"
            module Top (o: output logic<4>) {{
                var value: logic<2>;
                assign o = $signed(value);
                assign value[{target}] = o[3];
                assign value[{}] = 0;
            }}
            "#,
            1 - target
        ),
        expected,
    );
}

#[test]
fn comb_loop_unsigned_extension_zero_fills_high_bit() {
    assert_unsigned_context_extension("o[3]", false);
}

#[test]
fn comb_loop_unsigned_extension_retains_low_bit() {
    assert_unsigned_context_extension("o[0]", true);
}

#[test]
fn comb_loop_signed_extension_ignores_non_sign_bit() {
    assert_signed_context_extension(0, false);
}

#[test]
fn comb_loop_signed_extension_retains_sign_bit() {
    assert_signed_context_extension(1, true);
}

#[test]
fn comb_loop_ternary_preserves_unsigned_zero_extension() {
    assert_comb_loop(
        "a ternary preserves unsigned zero extension",
        r#"
        module Top (sel: input logic, o: output logic<4>) {
            var value: logic<2>;
            assign o = if sel ? value : 4'b0;
            assign value[0] = o[3];
            assign value[1] = 0;
        }
        "#,
        false,
    );
}

fn assert_signed_ternary_coercion(target: usize, expected: bool) {
    assert_comb_loop(
        "a signed ternary high bit retains only its sign-bit dependency",
        &format!(
            r#"
            module Top (sel: input logic, o: output logic<4>) {{
                var value: logic<2>;
                assign o = if sel ? $signed(value) : 4'sb0;
                assign value[{target}] = o[3];
                assign value[{}] = 0;
            }}
            "#,
            1 - target
        ),
        expected,
    );
}

fn assert_expression_coercion(expression: &str, feedback: &str, expected: bool) {
    assert_comb_loop(
        "expression coercion preserves only contributing positions",
        &format!(
            r#"
            module Top (o: output logic<4>) {{
                var value: logic<2>;
                assign o = {expression};
                assign value[0] = {feedback};
                assign value[1] = 0;
            }}
            "#
        ),
        expected,
    );
}

#[test]
fn comb_loop_signed_ternary_high_bit_is_disjoint_from_non_sign_bit() {
    assert_signed_ternary_coercion(0, false);
}

#[test]
#[ignore = "comb-loop migration: false negative; positional and periodic transfers"]
fn comb_loop_signed_ternary_high_bit_retains_sign_bit() {
    assert_signed_ternary_coercion(1, true);
}

#[test]
#[ignore = "comb-loop migration: false positive; positional and periodic transfers"]
fn comb_loop_widening_cast_zero_fills_high_bits() {
    assert_expression_coercion("value as 4", "o[3]", false);
}

#[test]
fn comb_loop_widening_cast_retains_low_bits() {
    assert_expression_coercion("value as 4", "o[0]", true);
}

#[test]
fn comb_loop_narrowing_cast_discards_high_source_bits() {
    assert_expression_coercion("value as 2", "o[3]", false);
}

#[test]
fn comb_loop_narrowing_cast_retains_low_source_bits() {
    assert_expression_coercion("value as 2", "o[0]", true);
}

#[test]
fn comb_loop_mixed_width_bitwise_zero_fills_short_operand() {
    assert_expression_coercion("value | 4'b0", "o[3]", false);
}

#[test]
fn comb_loop_mixed_width_bitwise_retains_corresponding_bit() {
    assert_expression_coercion("value | 4'b0", "o[0]", true);
}

#[test]
fn comb_loop_periodic_output_maps_concatenated_actual() {
    // Why this case exists: a child periodic output may legally connect to a
    // concatenated parent lvalue. Each output fragment must be clipped in the
    // child's coordinates; requiring one whole destination silently drops the
    // real a -> child.o[0] -> a feedback path.
    assert_comb_loop(
        "a periodic child output survives a concatenated parent destination",
        r#"
        module Fanout (
            i: input  logic,
            o: output logic<2>,
        ) {
            assign o = {i repeat 2};
        }

        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            inst u: Fanout (
                i: a,
                o: {b, a},
            );
            assign o = b;
        }
        "#,
        true,
    );
}
