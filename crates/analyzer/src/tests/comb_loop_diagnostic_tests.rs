// Diagnostic ownership and provenance coverage for comb-loop analysis.
use super::*;

#[test]
fn comb_loop_diagnostic_reports_an_ordered_dependency_cycle() {
    let errors = analyze(
        r#"
        module Top (
            a: output logic,
            b: output logic,
            c: output logic,
        ) {
            assign a = b;
            assign b = c;
            assign c = a;
        }
        "#,
    );
    let cycle = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle.as_str()),
        _ => None,
    });

    // Sorted SCC members would be a, b, c. The diagnostic follows the
    // dependency edges instead: a affects c, c affects b, and b affects a.
    assert_eq!(cycle, Some("a -> c -> b -> a"));
}

#[test]
fn comb_loop_diagnostic_labels_only_the_reported_cycle() {
    let errors = analyze(
        r#"
        module Top (
            a: output logic,
            b: output logic,
            c: output logic,
        ) {
            assign a = b | c;
            assign b = a;
            assign c = a;
        }
        "#,
    );
    let diagnostic = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop {
            cycle,
            loop_participants,
            ..
        } => Some((cycle.as_str(), loop_participants.len())),
        _ => None,
    });

    // The SCC also contains c, but the selected witness is the shorter a/b
    // cycle, so the unrelated c assignment should not be labeled.
    assert_eq!(diagnostic, Some(("a -> b -> a", 1)));
}

#[test]
fn comb_loop_diagnostic_preserves_array_elements_in_the_cycle() {
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var mem: logic [2];
            assign mem[0] = mem[1];
            assign mem[1] = mem[0];
            assign o = mem[0];
        }
        "#,
    );
    let cycle = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle.as_str()),
        _ => None,
    });

    assert_eq!(cycle, Some("mem[0] -> mem[1] -> mem[0]"));
}

#[test]
fn comb_loop_diagnostic_preserves_bit_regions_in_the_cycle() {
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var data: logic<2>;
            assign data[0] = data[1];
            assign data[1] = data[0];
            assign o = data[0];
        }
        "#,
    );
    let cycle = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle.as_str()),
        _ => None,
    });

    assert_eq!(cycle, Some("data[0] -> data[1] -> data[0]"));
}

#[test]
fn comb_loop_diagnostic_preserves_generate_hierarchy_in_the_cycle() {
    let errors = analyze(
        r#"
        module Top {
            if 1 :g_outer {
                for i in 0..2 :g_inner {
                    var a: logic;
                    var b: logic;
                    assign a = b;
                    assign b = a;
                }
            }
        }
        "#,
    );
    let mut cycles = errors
        .iter()
        .filter_map(|error| match error {
            AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    cycles.sort_unstable();

    assert_eq!(
        cycles,
        [
            "g_outer.g_inner[0].a -> g_outer.g_inner[0].b -> g_outer.g_inner[0].a",
            "g_outer.g_inner[1].a -> g_outer.g_inner[1].b -> g_outer.g_inner[1].a",
        ]
    );
}

#[test]
fn comb_loop_diagnostic_places_array_indices_inside_interface_member_paths() {
    let errors = analyze(
        r#"
        interface Bus {
            var value: logic [2];
        }
        module Top {
            if 1 :g {
                inst bus: Bus[2];
                assign bus[1].value[0] = bus[1].value[0];
            }
        }
        "#,
    );
    let cycle = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle.as_str()),
        _ => None,
    });

    assert_eq!(cycle, Some("g.bus[1].value[0] -> g.bus[1].value[0]"));
}

#[test]
fn comb_loop_diagnostic_preserves_selected_interface_array_prefixes() {
    let errors = analyze(
        r#"
        interface Bus {
            var value: logic [2];
        }
        module Top {
            if 1 :g {
                inst bus: Bus[2];
                assign bus[1].value = bus[1].value;
            }
        }
        "#,
    );
    let cycle = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle.as_str()),
        _ => None,
    });

    assert_eq!(cycle, Some("g.bus[1].value -> g.bus[1].value"));
}

#[test]
fn comb_loop_diagnostic_places_array_indices_on_imported_interface_members() {
    let errors = analyze(
        r#"
        interface Bus {
            var a: logic;
            var b: logic;
            function write_a () {
                a = b;
            }
            function write_b () {
                b = a;
            }
            modport monitor {
                write_a: import,
                write_b: import,
            }
        }
        module Receiver (
            bus: modport Bus::monitor[2],
        ) {
            always_comb {
                bus[1].write_a();
            }
            always_comb {
                bus[1].write_b();
            }
        }
        module Top {
            inst bus: Bus[2];
            inst receiver: Receiver (
                bus: bus,
            );
        }
        "#,
    );
    let cycle = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle.as_str()),
        _ => None,
    });

    assert_eq!(cycle, Some("bus[1].a -> bus[1].b -> bus[1].a"));
}

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
