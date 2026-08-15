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
