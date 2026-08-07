// Diagnostic ownership and provenance coverage for comb-loop migration.
use super::*;

#[test]
#[ignore = "comb-loop migration: diagnostic mismatch; diagnostic ownership and provenance"]
fn comb_loop_diagnostic_uses_short_assignment_cycle_witness() {
    // Why this case exists: all four variables belong to one maximal SCC, and
    // `a` has two reaching assignments in different branches. The actionable
    // witness is the short a/b cycle; reporting a=d, c=b, or d=c would expose
    // SCC membership rather than the actual cycle selected for this diagnostic.
    let code = r#"
    module Top (
        select: input  logic,
        o     : output logic,
    ) {
        var a: logic;
        var b: logic;
        var c: logic;
        var d: logic;
        always_comb {
            if select {
                a = b;
            } else {
                a = d;
            }
        }
        assign b = a;
        assign c = b;
        assign d = c;
        assign o = a;
    }
    "#;
    let errors = analyze(code);
    let loops = errors
        .iter()
        .filter(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
        .collect::<Vec<_>>();
    assert_eq!(loops.len(), 1, "{errors:#?}");
    let AnalyzerError::CombinationalLoop {
        identifier,
        input,
        error_location,
        loop_participants,
        ..
    } = loops[0]
    else {
        unreachable!()
    };
    assert_eq!(identifier, "a");
    assert_eq!(loop_participants.len(), 1, "{errors:#?}");

    let local_offset = |absolute: usize| {
        let mut base = 0usize;
        for source in &input.sources {
            if absolute < base + source.text.len() {
                return absolute - base;
            }
            base += source.text.len();
        }
        panic!("diagnostic offset {absolute} is outside its sources")
    };
    assert_eq!(
        local_offset(error_location.offset()),
        code.find("a = b;").expect("short-cycle assignment")
    );
    assert_eq!(
        local_offset(loop_participants[0].offset()),
        code.find("assign b = a;")
            .expect("short-cycle return assignment")
            + "assign ".len()
    );
}

#[test]
#[ignore = "comb-loop migration: diagnostic mismatch; diagnostic ownership and provenance"]
fn comb_loop_captured_coverage_sites_stay_region_local() {
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
    assert_eq!(coverage.len(), 1, "{errors:#?}");
    assert_eq!(coverage[0].len(), 1, "{errors:#?}");
    assert_eq!(
        coverage[0][0].offset(),
        code.find("value[0] = 1")
            .expect("retained capture assignment")
    );
}

#[test]
#[ignore = "comb-loop migration: diagnostic mismatch; diagnostic ownership and provenance"]
fn comb_loop_branch_weak_writes_share_one_coverage_diagnostic() {
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
    assert_eq!(coverage.len(), 1, "{errors:#?}");
    let AnalyzerError::UncoveredBranch {
        error_locations, ..
    } = coverage[0]
    else {
        unreachable!()
    };
    assert_eq!(error_locations.len(), 2, "{errors:#?}");
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "implicit preservation is not combinational feedback: {errors:#?}"
    );
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

#[test]
fn comb_loop_dynamic_loop_coverage_sites_stay_region_local() {
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
    assert_eq!(coverage.len(), 1, "{errors:#?}");
    assert_eq!(coverage[0].len(), 1, "{errors:#?}");
    assert_eq!(
        coverage[0][0].offset(),
        code.find("value[0] = 1").expect("retained bit assignment")
    );
}
