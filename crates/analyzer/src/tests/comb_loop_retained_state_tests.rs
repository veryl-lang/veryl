use super::*;

fn assert_comb_loop(case: &str, code: &str, expected: bool) {
    let errors = analyze(code);
    let actual = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert_eq!(actual, expected, "{case}: {errors:?}");
}

fn assert_incomplete_assignment_without_comb_loop(case: &str, code: &str) {
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{case}: retained entry state must be diagnosed as an incomplete assignment: {errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{case}: state retention is not combinational feedback: {errors:#?}"
    );
}

fn assert_cond_type_coverage(statement: &str, cond_type: &str, expect_uncovered: bool) {
    let body = if statement == "if" {
        format!("#[cond_type({cond_type})] if sel {{ o = 1; }}")
    } else {
        format!("#[cond_type({cond_type})] case sel {{ 0: o = 1; }}")
    };
    let errors = analyze(&format!(
        r#"
        module Top (sel: input logic, o: output logic) {{
            always_comb {{ {body} }}
        }}
        "#
    ));
    let uncovered = errors
        .iter()
        .filter(|error| matches!(error, AnalyzerError::UncoveredBranch { .. }))
        .count();
    assert_eq!(
        uncovered,
        usize::from(expect_uncovered),
        "{statement} cond_type({cond_type}) produced unexpected diagnostics: {errors:#?}"
    );
}

macro_rules! cond_type_case {
    ($name:ident, $statement:literal, $cond_type:literal, $expected:expr) => {
        #[test]
        fn $name() {
            assert_cond_type_coverage($statement, $cond_type, $expected);
        }
    };
}

cond_type_case!(comb_coverage_dynamic_if_unique, "if", "unique", false);

cond_type_case!(comb_coverage_dynamic_if_unique0, "if", "unique0", false);

cond_type_case!(comb_coverage_dynamic_if_priority, "if", "priority", false);

cond_type_case!(comb_coverage_dynamic_if_none, "if", "none", true);

cond_type_case!(comb_coverage_dynamic_case_unique, "case", "unique", false);

cond_type_case!(comb_coverage_dynamic_case_unique0, "case", "unique0", false);

cond_type_case!(
    comb_coverage_dynamic_case_priority,
    "case",
    "priority",
    false
);

cond_type_case!(comb_coverage_dynamic_case_none, "case", "none", true);

#[test]
fn comb_loop_drops_unreachable_statements_after_break() {
    // Why this case exists: IEEE 1800-2023 12.8 makes break jump to the loop
    // exit. Statements after it are unreachable and cannot prove a hard SCC.
    assert_comb_loop(
        "unreachable statements after break do not form a loop",
        r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var x: logic;
            var y: logic;
            always_comb {
                for i in 0..n {
                    break;
                    x = y;
                    y = x;
                }
                o = 0;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_a_full_write_after_a_dynamic_self_store_kills_the_dead_feedback()
 {
    assert_comb_loop(
        "a full write after a dynamic self-store kills the dead feedback",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic<4>,
        ) {
            always_comb {
                o[index] = o[index];
                o = 0;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_a_dominating_full_write_kills_a_later_dynamic_read_of_the_old_value()
 {
    assert_comb_loop(
        "a dominating full write kills a later dynamic read of the old value",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic<4>,
        ) {
            always_comb {
                o = 0;
                o[index] = o[index];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_an_unrelated_exact_bit_write_cannot_kill_dynamic_feedback()
 {
    assert_comb_loop(
        "an unrelated exact bit write cannot kill dynamic feedback",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic<4>,
        ) {
            always_comb {
                o[index] = o[index];
                o[0] = 0;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_all_branch_exits_overwriting_the_object_kill_earlier_dynamic_feedback()
 {
    assert_comb_loop(
        "all branch exits overwriting the object kill earlier dynamic feedback",
        r#"
        module Top (
            index: input  logic<2>,
            sel  : input  logic,
            o    : output logic<4>,
        ) {
            always_comb {
                o[index] = o[index];
                if sel {
                    o = 0;
                } else {
                    o = 1;
                }
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_one_branch_preserving_a_dynamic_self_store_preserves_feedback()
 {
    assert_comb_loop(
        "one branch preserving a dynamic self-store preserves feedback",
        r#"
        module Top (
            index: input  logic<2>,
            sel  : input  logic,
            o    : output logic<4>,
        ) {
            always_comb {
                o[index] = o[index];
                if sel {
                    o = 0;
                }
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_a_dominating_value_write_also_kills_a_self_derived_dynamic_address()
 {
    assert_comb_loop(
        "a dominating value write also kills a self-derived dynamic address",
        r#"
        module Top (
            data: input  logic,
            o   : output logic<4>,
        ) {
            always_comb {
                o = 0;
                o[o[1:0]] = data;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_kill_semantics_an_undominated_value_driving_its_own_dynamic_address_is_feedback()
 {
    assert_comb_loop(
        "an undominated value driving its own dynamic address is feedback",
        r#"
        module Top (
            data: input  logic,
            o   : output logic<4>,
        ) {
            always_comb {
                o[o[1:0]] = data;
            }
        }
        "#,
        true,
    );
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: zero-trip loop"]
fn comb_loop_dynamic_loop_zero_trip_retention_is_not_feedback() {
    // Why this case exists: a runtime loop can execute zero times. The value
    // retained on that path infers state, but it is not a same-evaluation read
    // from `held` and therefore must not become a combinational self-loop.
    assert_incomplete_assignment_without_comb_loop(
        "a zero-trip runtime loop retains state rather than feeding back combinationally",
        r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var held: logic;
            always_comb {
                for index in 0..n {
                    held = index[0];
                }
                o = held;
            }
        }
        "#,
    );
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: unwritten dynamic elements"]
fn comb_loop_dynamic_element_retention_is_not_feedback() {
    // Why this case exists: one dynamic element write leaves every other
    // candidate element unchanged. May-write coverage must not masquerade as
    // must-write coverage, and the preserved elements are latch state rather
    // than a combinational self-read.
    assert_incomplete_assignment_without_comb_loop(
        "a dynamic element store leaves unselected elements unassigned",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic,
        ) {
            var held: logic [4];
            always_comb {
                held[index] = 1;
                o = held[0];
            }
        }
        "#,
    );
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: oversized dynamic store"]
fn comb_loop_oversized_dynamic_retention_is_sparse_and_not_feedback() {
    // Why this case exists: the legacy assignment table skips arrays above its
    // enumeration limit. Retention coverage must stay declaration-width
    // independent instead of either disappearing or expanding 131072 elements,
    // and it still must not enter the combinational SCC graph.
    assert_incomplete_assignment_without_comb_loop(
        "an oversized dynamic store is diagnosed without element enumeration",
        r#"
        module Top (
            index: input  logic<17>,
            o    : output logic,
        ) {
            var held: logic [131072];
            always_comb {
                held[index] = 1;
                o = held[0];
            }
        }
        "#,
    );
}

#[test]
fn comb_loop_missing_if_arm_retention_is_not_feedback() {
    // Why this case exists: the existing uncovered-branch checker already
    // identifies this latch. The causal graph must not add a second and
    // semantically incorrect combinational-loop diagnosis for the same
    // unassigned path.
    assert_incomplete_assignment_without_comb_loop(
        "an if without an else infers a latch, not a combinational loop",
        r#"
        module Top (
            enable: input  logic,
            o     : output logic,
        ) {
            var held: logic;
            always_comb {
                if enable {
                    held = 1;
                }
                o = held;
            }
        }
        "#,
    );
}

#[test]
fn comb_loop_missing_switch_default_retention_is_not_feedback() {
    // Why this case exists: n-way control flow reaches the same entry-state
    // phi as an uncovered if. Missing-default retention must receive the same
    // latch diagnosis without being reclassified as combinational feedback.
    assert_incomplete_assignment_without_comb_loop(
        "a switch without a default infers a latch, not a combinational loop",
        r#"
        module Top (
            select: input  logic,
            o     : output logic,
        ) {
            var held: logic;
            always_comb {
                switch {
                    select: held = 1;
                }
                o = held;
            }
        }
        "#,
    );
}

#[test]
fn comb_loop_retention_still_reports_incomplete_assignment() {
    // Why this case exists: retention itself is diagnosed as incomplete
    // assignment, but it must not erase the explicit held -> o -> enable
    // value/control dependencies. Those dependencies form proven structural
    // feedback independently of the retained entry-state path.
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var enable: logic;
            var held  : logic;
            always_comb {
                if enable {
                    held = 0;
                }
                o = held;
            }
            assign enable = o;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "the missing assignment path must remain diagnosed: {errors:#?}"
    );
}

#[test]
fn comb_loop_retention_does_not_hide_cross_variable_feedback() {
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var enable: logic;
            var held  : logic;
            always_comb {
                if enable {
                    held = 0;
                }
                o = held;
            }
            assign enable = o;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "retention must not hide independently proven structural feedback: {errors:#?}"
    );
}

#[test]
fn comb_loop_default_before_dynamic_loop_kills_retention() {
    // Why this case exists: a dominating full assignment removes the entry
    // state before a possibly-zero-trip loop. This is the positive control for
    // both coverage and loop analysis and must remain diagnostic-free.
    let errors = analyze(
        r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var value: logic;
            always_comb {
                value = 0;
                for index in 0..n {
                    value = index[0];
                }
                o = value;
            }
        }
        "#,
    );
    assert!(
        errors.is_empty(),
        "a dominating default is complete: {errors:#?}"
    );
}

#[test]
fn comb_loop_full_write_after_dynamic_loop_kills_retention() {
    // Why this case exists: incomplete-assignment coverage is a property of
    // the procedure exit, not of an intermediate statement. A later full
    // write kills the zero-trip entry value left by the runtime loop.
    let errors = analyze(
        r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var value: logic;
            always_comb {
                for index in 0..n {
                    value = index[0];
                }
                value = 0;
                o = value;
            }
        }
        "#,
    );
    assert!(
        errors.is_empty(),
        "a later full write kills runtime-loop retention: {errors:#?}"
    );
}

#[test]
fn comb_loop_nonempty_const_loop_has_no_zero_trip_path() {
    // Why this case exists: `break` keeps a constant loop in runtime-form IR,
    // but 0..1 still enters its body exactly once. Runtime representation does
    // not by itself imply that a zero-trip path is reachable.
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var value: logic;
            always_comb {
                for _index in 0..1 {
                    value = 1;
                    break;
                }
                o = value;
            }
        }
        "#,
    );
    assert!(
        errors.is_empty(),
        "a statically nonempty loop assigns before breaking: {errors:#?}"
    );
}

#[test]
fn comb_loop_empty_const_loop_has_no_body_path() {
    // Why this case exists: `break` keeps this const-evaluable empty range in
    // runtime-form IR, but its body is still unreachable. Treating "empty" as
    // merely "possibly empty" invents both a write and a hard cycle.
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _index in 1..1 {
                    a = b;
                    break;
                }
            }
            assign b = a;
            assign o = b;
        }
        "#,
    );
    assert!(
        errors.iter().all(|error| !matches!(
            error,
            AnalyzerError::CombinationalLoop { .. } | AnalyzerError::UncoveredBranch { .. }
        )),
        "an empty loop has no reachable body assignment: {errors:#?}"
    );
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: conditional break"]
fn comb_loop_break_before_write_retains_coverage() {
    // Why this case exists: a const singleton loop is guaranteed to enter its
    // body, but a conditional break can still bypass a later assignment. The
    // missed write is a coverage error, not a zero-trip path or comb feedback.
    assert_incomplete_assignment_without_comb_loop(
        "a break before the first write leaves the output unassigned",
        r#"
        module Top (
            stop: input  logic,
            o   : output logic,
        ) {
            var value: logic;
            always_comb {
                for _index in 0..1 {
                    if stop {
                        break;
                    }
                    value = 1;
                }
                o = value;
            }
        }
        "#,
    );
}

fn assert_singleton_const_range_retains_coverage(range: &str) {
    assert_incomplete_assignment_without_comb_loop(
        "a singleton runtime-form range can break before its write",
        &format!(
            r#"
            module Top (
                stop: input  logic,
                o   : output logic,
            ) {{
                var value: logic;
                always_comb {{
                    for _index in {range} {{
                        if stop {{
                            break;
                        }}
                        value = 1;
                    }}
                    o = value;
                }}
            }}
            "#
        ),
    );
}

fn assert_empty_const_range_has_no_body_path(range: &str) {
    let errors = analyze(&format!(
        r#"
        module Top (
            o: output logic,
        ) {{
            var a: logic;
            var b: logic;
            always_comb {{
                for _index in {range} {{
                    a = b;
                    break;
                }}
            }}
            assign b = a;
            assign o = b;
        }}
        "#
    ));
    assert!(
        errors.iter().all(|error| !matches!(
            error,
            AnalyzerError::CombinationalLoop { .. } | AnalyzerError::UncoveredBranch { .. }
        )),
        "an empty {range} loop has no body path: {errors:#?}"
    );
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: inclusive singleton range"]
fn comb_loop_const_range_inclusive_singleton_retains_coverage() {
    assert_singleton_const_range_retains_coverage("0..=0");
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: reverse singleton range"]
fn comb_loop_const_range_reverse_singleton_retains_coverage() {
    assert_singleton_const_range_retains_coverage("rev 0..1");
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: stepped singleton range"]
fn comb_loop_const_range_stepped_singleton_retains_coverage() {
    assert_singleton_const_range_retains_coverage("1..2 step *= 2");
}

#[test]
fn comb_loop_const_range_exclusive_empty_has_no_body_path() {
    assert_empty_const_range_has_no_body_path("1..1");
}

#[test]
fn comb_loop_const_range_reverse_empty_has_no_body_path() {
    assert_empty_const_range_has_no_body_path("rev 1..1");
}

#[test]
fn comb_loop_const_range_stepped_empty_has_no_body_path() {
    assert_empty_const_range_has_no_body_path("2..2 step *= 2");
}

#[test]
fn comb_loop_const_iterator_prunes_dead_if_edges() {
    // Why this case exists: break keeps the singleton loop in runtime-form IR,
    // but its sole iterator value is still the constant 1. The unreachable
    // i==0 assignment must not create a stronger-than-SV a -> b -> a loop.
    assert_comb_loop(
        "a runtime-form singleton retains its iterator constant",
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for i in 1..2 {
                    if i == 0 {
                        a = b;
                    } else {
                        a = 0;
                    }
                    break;
                }
            }
            assign b = a;
            assign o = b;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_const_iterator_keeps_taken_if_edges() {
    assert_comb_loop(
        "a reachable iterator-selected if arm retains its dependency",
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for i in 1..2 {
                    if i == 1 {
                        a = b;
                    } else {
                        a = 0;
                    }
                    break;
                }
            }
            assign b = a;
            assign o = b;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_const_iterator_prunes_dead_case_arms() {
    // Why this case exists: case selection uses the same per-iteration SV
    // value as if selection. A dead i==0 arm in a singleton i==1 loop cannot
    // contribute a hard dependency merely because break prevented frontend
    // unrolling.
    assert_comb_loop(
        "a runtime-form singleton prunes dead case arms",
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for i in 1..2 {
                    case i {
                        0      : a = b;
                        default: a = 0;
                    }
                    break;
                }
            }
            assign b = a;
            assign o = b;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_const_iterator_keeps_taken_case_arms() {
    assert_comb_loop(
        "a reachable iterator-selected case arm retains its dependency",
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for i in 1..2 {
                    case i {
                        1      : a = b;
                        default: a = 0;
                    }
                    break;
                }
            }
            assign b = a;
            assign o = b;
        }
        "#,
        true,
    );
}

const CONST_ITERATOR_MUST_WRITE: &str = r#"
    module Top (
        stop: input  logic,
        o   : output logic,
    ) {
        var value: logic;
        always_comb {
            for i in 0..2 {
                if i == 1 {
                    value = 1;
                }
                if i == 1 && stop {
                    break;
                }
            }
            o = value;
        }
    }
"#;

#[test]
fn comb_loop_const_iterator_preserves_must_write_paths() {
    assert_comb_loop(
        "const iterator paths do not invent retained-state feedback",
        CONST_ITERATOR_MUST_WRITE,
        false,
    );
}

#[test]
#[ignore = "legacy AssignTable evaluates a const for body once without binding the iterator or modeling break paths"]
fn latch_const_iterator_preserves_must_write_paths() {
    // Why this case exists: the two const iterations are i=0 and i=1, and the
    // only reachable break follows the i=1 assignment. Every exit is covered;
    // collapsing both iterations into one unknown body invents retention.
    let errors = analyze(CONST_ITERATOR_MUST_WRITE);
    assert!(
        errors.is_empty(),
        "all finite-loop exits assign value: {errors:#?}"
    );
}

#[test]
fn comb_loop_seeded_finite_recurrence_is_feed_forward() {
    // Why this case exists: a conditional break keeps a finite const loop in
    // runtime-form IR. Its loop-carried value is a bounded combinational chain
    // rooted at the dominating default, not an unbounded graph backedge.
    let errors = analyze(
        r#"
        module Top (
            stop: input  logic,
            o   : output logic,
        ) {
            var value: logic;
            always_comb {
                value = 0;
                for _index in 0..2 {
                    value = !value;
                    if stop {
                        break;
                    }
                }
                o = value;
            }
        }
        "#,
    );
    assert!(
        errors.is_empty(),
        "a bounded recurrence with a dominating seed is feed-forward: {errors:#?}"
    );
}

#[test]
fn comb_loop_unseeded_finite_recurrence_is_feedback() {
    // Why this case exists: bounded lowering must remove only loop-carried
    // pseudo-feedback. Without a dominating seed, the first iteration's
    // explicit value read still forms real combinational self-feedback.
    assert_comb_loop(
        "a finite runtime-form loop retains its first explicit self-read",
        r#"
        module Top (
            stop: input  logic,
            o   : output logic,
        ) {
            var value: logic;
            always_comb {
                for _index in 0..2 {
                    value = !value;
                    if stop {
                        break;
                    }
                }
                o = value;
            }
        }
        "#,
        true,
    );
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: function output weak write"]
fn comb_loop_function_output_weak_write_retains_coverage() {
    // Why this case exists: a function output argument is copied back to its
    // caller, but a dynamic write inside the function still leaves unselected
    // packed bits without a definition. Function summarization must preserve
    // that exit-coverage fact without inventing a comb dependency.
    assert_incomplete_assignment_without_comb_loop(
        "a function output weak write remains incomplete at the caller",
        r#"
        module Top (
            index: input  logic<2>,
            o    : output logic,
        ) {
            function write_selected (
                index: input  logic<2>,
                value: output logic<4>,
            ) {
                value[index] = 1;
            }
            var value: logic<4>;
            always_comb {
                write_selected(index, value);
                o = value[0];
            }
        }
        "#,
    );
}

#[test]
fn comb_coverage_cond_type_suppression_is_join_local() {
    // Why this case exists: dropping every write beneath cond_type from
    // coverage would also deprive the ordinary outer join of its diagnostic
    // source. Suppression belongs to the annotated CFG join, while retention
    // introduced by another join remains diagnostic.
    let errors = analyze(
        r#"
        module Top (
            outer: input  logic,
            inner: input  logic,
            o    : output logic,
        ) {
            always_comb {
                if outer {
                    #[cond_type(priority)]
                    if inner {
                        o = 1;
                    }
                }
            }
        }
        "#,
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error, AnalyzerError::UncoveredBranch { .. }))
            .count(),
        1,
        "the unannotated outer join must remain diagnostic: {errors:#?}"
    );
}

#[test]
fn comb_loop_cond_type_does_not_erase_feedback() {
    // Why this case exists: cond_type affects only incomplete-assignment
    // reporting. The retained value and explicit self-read remain in the
    // causal summary, so a proven structural loop must still be rejected.
    let errors = analyze(
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            always_comb {
                #[cond_type(priority)]
                if sel {
                    o = o;
                }
            }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "cond_type must not erase a proven feedback dependency: {errors:#?}"
    );
}

#[test]
fn comb_loop_cond_type_suppresses_coverage() {
    let errors = analyze(
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            always_comb {
                #[cond_type(priority)]
                if sel {
                    o = o;
                }
            }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "cond_type(priority) must still suppress its coverage diagnostic: {errors:#?}"
    );
}
