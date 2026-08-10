// Diagnostic ownership and provenance coverage for comb-loop analysis.
use super::*;

fn captured_coverage_observation() -> (usize, Option<usize>, Option<usize>) {
    // Why this case exists: function summary coverage is mapped back into the
    // caller one captured region at a time. A caller default for value[1] must
    // prevent that bit's function assignment from appearing in value[0]'s
    // remaining coverage diagnostic.
    let code = r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var value: logic<2>;
            function write_bits (
                n: input logic<32>,
            ) {
                for _index in 0..n {
                    value[0] = 1;
                }
                for _index in 0..n {
                    value[1] = 1;
                }
            }
            always_comb {
                value[1] = 0;
                write_bits(n);
                o = value[0];
            }
        }
    "#;
    let errors = analyze(code);
    let coverage = errors
        .iter()
        .filter_map(|error| match error {
            AnalyzerError::UncoveredBranch {
                error_locations, ..
            } => Some(error_locations),
            _ => None,
        })
        .collect::<Vec<_>>();
    (
        coverage.len(),
        coverage.first().map(|sites| sites.len()),
        coverage
            .first()
            .and_then(|sites| sites.first())
            .map(|site| site.offset()),
    )
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: captured-region diagnostic count"]
fn comb_loop_captured_coverage_has_one_diagnostic() {
    assert_eq!(captured_coverage_observation().0, 1);
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: captured-region site count"]
fn comb_loop_captured_coverage_has_one_site() {
    assert_eq!(captured_coverage_observation().1, Some(1));
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: captured-region site provenance"]
fn comb_loop_captured_coverage_uses_retained_bit_site() {
    let code = r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var value: logic<2>;
            function write_bits (
                n: input logic<32>,
            ) {
                for _index in 0..n {
                    value[0] = 1;
                }
                for _index in 0..n {
                    value[1] = 1;
                }
            }
            always_comb {
                value[1] = 0;
                write_bits(n);
                o = value[0];
            }
        }
    "#;
    assert_eq!(captured_coverage_observation().2, code.find("value[0] = 1"));
}

fn branch_weak_write_observation() -> (usize, Option<usize>, bool) {
    // Why this case exists: distinct weak writes can reach the same retained
    // object through different branches. Coverage reporting should present one
    // coherent variable diagnostic containing both assignment sites.
    let errors = analyze(
        r#"
        module Top (
            condition: input  logic,
            left     : input  logic<2>,
            right    : input  logic<2>,
            o        : output logic,
        ) {
            var value: logic<4>;
            always_comb {
                if condition {
                    value[left] = 1;
                } else {
                    value[right] = 0;
                }
                o = value[0];
            }
        }
        "#,
    );
    let coverage = errors
        .iter()
        .filter(|error| matches!(error, AnalyzerError::UncoveredBranch { .. }))
        .collect::<Vec<_>>();
    let site_count = coverage.first().and_then(|error| match error {
        AnalyzerError::UncoveredBranch {
            error_locations, ..
        } => Some(error_locations.len()),
        _ => None,
    });
    let has_loop = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    (coverage.len(), site_count, has_loop)
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: merge weak-write diagnostic count"]
fn comb_loop_branch_weak_writes_share_one_coverage_diagnostic() {
    assert_eq!(branch_weak_write_observation().0, 1);
}

#[test]
#[ignore = "SSA latch coverage follow-up after comb-loop migration: merge weak-write sites"]
fn comb_loop_branch_weak_write_diagnostic_contains_both_sites() {
    assert_eq!(branch_weak_write_observation().1, Some(2));
}

#[test]
fn comb_loop_branch_weak_writes_do_not_create_feedback() {
    assert!(!branch_weak_write_observation().2);
}

#[test]
fn comb_loop_dynamic_loop_coverage_is_not_duplicated() {
    // Why this case exists: both legacy branch bookkeeping and MemorySSA can
    // observe the missing path inside a dynamic loop. Coverage ownership must
    // produce one warning rather than appending the same warning twice.
    let errors = analyze(
        r#"
        module Top (
            n        : input  logic<32>,
            condition: input  logic,
            o        : output logic,
        ) {
            var value: logic;
            always_comb {
                for _index in 0..n {
                    if condition {
                        value = 1;
                    }
                }
                o = value;
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
        "dynamic-loop coverage must be reported once: {errors:#?}"
    );
}

fn dynamic_loop_coverage_observation() -> (usize, Option<usize>, Option<usize>) {
    // Why this case exists: value[1] is fully defined even though it is also
    // written in a runtime loop. Its loop assignment must not be reported as a
    // covered site for the independently retained value[0].
    let code = r#"
        module Top (
            n        : input  logic<32>,
            condition: input  logic,
            o        : output logic<2>,
        ) {
            var value: logic<2>;
            always_comb {
                value[1] = 0;
                for _index in 0..n {
                    value[1] = 1;
                }
                if condition {
                    value[0] = 1;
                }
                o = value;
            }
        }
    "#;
    let errors = analyze(code);
    let coverage = errors
        .iter()
        .filter_map(|error| match error {
            AnalyzerError::UncoveredBranch {
                error_locations, ..
            } => Some(error_locations),
            _ => None,
        })
        .collect::<Vec<_>>();
    (
        coverage.len(),
        coverage.first().map(|sites| sites.len()),
        coverage
            .first()
            .and_then(|sites| sites.first())
            .map(|site| site.offset()),
    )
}

#[test]
fn comb_loop_dynamic_loop_coverage_has_one_diagnostic() {
    assert_eq!(dynamic_loop_coverage_observation().0, 1);
}

#[test]
fn comb_loop_dynamic_loop_coverage_has_one_site() {
    assert_eq!(dynamic_loop_coverage_observation().1, Some(1));
}

#[test]
fn comb_loop_dynamic_loop_coverage_site_stays_region_local() {
    let code = r#"
        module Top (
            n        : input  logic<32>,
            condition: input  logic,
            o        : output logic<2>,
        ) {
            var value: logic<2>;
            always_comb {
                value[1] = 0;
                for _index in 0..n {
                    value[1] = 1;
                }
                if condition {
                    value[0] = 1;
                }
                o = value;
            }
        }
    "#;
    assert_eq!(
        dynamic_loop_coverage_observation().2,
        code.find("value[0] = 1")
    );
}
