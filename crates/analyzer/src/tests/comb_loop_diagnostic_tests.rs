// Diagnostic ownership and provenance coverage for comb-loop analysis.
use super::*;

fn diagnostic_span_text<'a>(
    input: &'a crate::multi_sources::MultiSources,
    span: &miette::SourceSpan,
) -> Option<(&'a str, &'a str)> {
    let start = span.offset();
    let span_end = start.checked_add(span.len())?;
    let mut base: usize = 0;
    for source in &input.sources {
        let source_end = base.checked_add(source.text.len())?;
        if base <= start && span_end <= source_end {
            return Some((
                source.path.as_str(),
                source.text.get(start - base..span_end - base)?,
            ));
        }
        base = source_end;
    }
    None
}

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

#[test]
fn comb_loop_diagnostic_reports_every_data_carrying_statement_on_a_summarized_path() {
    let child = r#"
        module Child (
            accept: input logic,
            valid : input logic,
            passed: output logic,
        ) {
            var data_done: logic;
            var mcmd_active: logic;
            always_comb {
                data_done = accept && valid;
                mcmd_active =
                    valid || data_done;
                passed = 0;
                if mcmd_active {
                    passed = valid;
                }
            }
        }
    "#;
    let top = r#"
        module Top (
            valid : input logic,
            passed: output logic,
        ) {
            var accept: logic;
            inst child: Child (
                accept: accept,
                valid : valid,
                passed: passed,
            );
            assign accept = passed;
        }
    "#;
    let errors = analyze_multiple_inputs(&[child, top]);
    let (input, participants, path) = errors
        .iter()
        .find_map(|error| match error {
            AnalyzerError::CombinationalLoop {
                input,
                loop_participants,
                dependency_sites,
                ..
            } => Some((input, loop_participants, dependency_sites)),
            _ => None,
        })
        .expect("the parent connection closes the child feedthrough loop");
    assert_eq!(path.len(), 2);
    // The parent hop closing the cycle is reachable only as a participant.
    let participants = participants
        .iter()
        .map(|span| diagnostic_span_text(input, span).expect("participant is in source"))
        .collect::<Vec<_>>();
    assert_eq!(participants, vec![("test_1.veryl", "accept")]);
    let path = path
        .iter()
        .map(|step| diagnostic_span_text(input, step).expect("path step is in source"))
        .collect::<Vec<_>>();
    assert_eq!(
        path,
        vec![
            ("test_0.veryl", "data_done = accept && valid;"),
            (
                "test_0.veryl",
                "mcmd_active =\n                    valid || data_done;",
            ),
        ]
    );
}

#[test]
fn comb_loop_diagnostic_reports_every_data_carrying_branch() {
    let child = r#"
        module Child (
            select: input  logic,
            i     : input  logic,
            o     : output logic,
        ) {
            always_comb {
                if select {
                    o = i;
                } else {
                    o = ~i;
                }
            }
        }
    "#;
    let top = r#"
        module Top (
            select: input logic,
        ) {
            var feedback: logic;
            inst child: Child (
                select: select,
                i     : feedback,
                o     : feedback,
            );
        }
    "#;
    let errors = analyze_multiple_inputs(&[child, top]);
    let (input, path) = errors
        .iter()
        .find_map(|error| match error {
            AnalyzerError::CombinationalLoop {
                input,
                dependency_sites,
                ..
            } => Some((input, dependency_sites)),
            _ => None,
        })
        .expect("both child branches carry the feedback dependency");
    let path = path
        .iter()
        .map(|step| diagnostic_span_text(input, step).expect("path step is in source"))
        .collect::<Vec<_>>();

    assert_eq!(
        path,
        vec![("test_0.veryl", "o = i;"), ("test_0.veryl", "o = ~i;")]
    );
}

#[test]
fn comb_loop_diagnostic_reports_controlled_assignments_for_a_control_dependency() {
    let child = r#"
        module Child (
            i: input  logic,
            o: output logic,
        ) {
            always_comb {
                if i {
                    o = 1;
                } else {
                    o = 0;
                }
            }
        }
    "#;
    let top = r#"
        module Top {
            var feedback: logic;
            inst child: Child (
                i: feedback,
                o: feedback,
            );
        }
    "#;
    let errors = analyze_multiple_inputs(&[child, top]);
    let (input, sites) = errors
        .iter()
        .find_map(|error| match error {
            AnalyzerError::CombinationalLoop {
                input,
                dependency_sites,
                ..
            } => Some((input, dependency_sites)),
            _ => None,
        })
        .expect("the branch condition carries the feedback dependency");
    let sites = sites
        .iter()
        .map(|site| diagnostic_span_text(input, site).expect("dependency site is in source"))
        .collect::<Vec<_>>();

    assert_eq!(
        sites,
        vec![("test_0.veryl", "o = 1;"), ("test_0.veryl", "o = 0;")]
    );
}

#[test]
fn comb_loop_diagnostic_does_not_replay_provenance_without_a_loop() {
    crate::comb_loop_detect::reset_diagnostic_replay_count();
    let errors = analyze_multiple_inputs(&[
        r#"
        module Child (
            accept: input logic,
            valid : input logic,
            passed: output logic,
        ) {
            var data_done: logic;
            var mcmd_active: logic;
            always_comb {
                data_done = accept && valid;
                mcmd_active =
                    valid || data_done;
                passed = 0;
                if mcmd_active {
                    passed = valid;
                }
            }
        }
        "#,
        r#"
        module Top (
            valid : input logic,
            passed: output logic,
        ) {
            var accept: logic;
            inst child: Child (
                accept: accept,
                valid : valid,
                passed: passed,
            );
            assign accept = 0;
        }
        "#,
    ]);

    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. }))
    );
    assert_eq!(crate::comb_loop_detect::diagnostic_replay_count(), 0);
}

#[test]
fn comb_loop_diagnostic_traces_through_nested_module_summaries() {
    let leaf = r#"
        module Leaf (
            accept: input logic,
            valid : input logic,
            passed: output logic,
        ) {
            var data_done: logic;
            var mcmd_active: logic;
            always_comb {
                data_done = accept && valid;
                mcmd_active =
                    valid || data_done;
                passed = 0;
                if mcmd_active {
                    passed = valid;
                }
            }
        }
    "#;
    let wrapper = r#"
        module Wrapper (
            accept: input logic,
            valid : input logic,
            passed: output logic,
        ) {
            var leaf_accept: logic;
            var leaf_passed: logic;
            assign leaf_accept = accept;
            inst leaf: Leaf (
                accept: leaf_accept,
                valid : valid,
                passed: leaf_passed,
            );
            assign passed = leaf_passed;
        }
    "#;
    let top = r#"
        module Top (
            valid : input logic,
            passed: output logic,
        ) {
            var accept: logic;
            inst wrapper: Wrapper (
                accept: accept,
                valid : valid,
                passed: passed,
            );
            assign accept = passed;
        }
    "#;
    let errors = analyze_multiple_inputs(&[leaf, wrapper, top]);
    let (input, path) = errors
        .iter()
        .find_map(|error| match error {
            AnalyzerError::CombinationalLoop {
                input,
                dependency_sites,
                ..
            } => Some((input, dependency_sites)),
            _ => None,
        })
        .expect("the top connection closes the nested feedthrough loop");

    let path = path
        .iter()
        .map(|step| diagnostic_span_text(input, step).expect("path step is in source"))
        .collect::<Vec<_>>();
    assert_eq!(
        path,
        vec![
            ("test_1.veryl", "assign leaf_accept = accept;"),
            ("test_0.veryl", "data_done = accept && valid;"),
            (
                "test_0.veryl",
                "mcmd_active =\n                    valid || data_done;",
            ),
            ("test_1.veryl", "assign passed = leaf_passed;"),
        ]
    );
}

#[test]
fn comb_loop_diagnostic_uses_the_closing_parallel_summary_edge() {
    let child = r#"
        module Child (
            sel    : input  logic,
            shift_i: input  logic<4>,
            id_i   : input  logic<4>,
            o      : output logic<4>,
        ) {
            always_comb {
                o = 0;
                if sel {
                    o = shift_i >> 1;
                } else {
                    o[2:1] = id_i[2:1];
                }
            }
        }
    "#;
    let top = r#"
        module Top (
            sel: input  logic,
            out: output logic<4>,
        ) {
            var feedback: logic<4>;
            inst child: Child (
                sel    : sel,
                shift_i: feedback,
                id_i   : feedback,
                o      : feedback,
            );
            assign out = feedback;
        }
    "#;
    let errors = analyze_multiple_inputs(&[child, top]);
    let (cycle, input, path) = errors
        .iter()
        .find_map(|error| match error {
            AnalyzerError::CombinationalLoop {
                cycle,
                input,
                dependency_sites,
                ..
            } => Some((cycle, input, dependency_sites)),
            _ => None,
        })
        .expect("the direct feedthrough closes the self-loop");

    // The structural graph keeps the observed two-bit identity region intact.
    assert_eq!(cycle, "feedback[2:1] -> feedback[2:1]");
    assert_eq!(path.len(), 1);
    let (source, text) = diagnostic_span_text(input, &path[0]).expect("path step is in source");
    assert_eq!(source, "test_0.veryl");
    assert_eq!(text, "o[2:1] = id_i[2:1];");
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

fn reverse_procedural_chain(count: usize, one_line: bool) -> (String, String) {
    let variables = (0..count)
        .map(|index| format!("#[allow(unassign_variable)]\nvar x{index}: logic;"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut assignments = vec![format!("o = x{};", count - 1)];
    assignments.extend(
        (1..count)
            .rev()
            .map(|index| format!("x{index} = x{};", index - 1)),
    );
    assignments.push("x0 = i;".into());
    let assignments = assignments.join(if one_line { " " } else { "\n" });
    let child = format!(
        r#"
        module Child (
            i: input  logic,
            o: output logic,
        ) {{
            {variables}
            always_comb {{
                {assignments}
            }}
        }}
        "#,
    );
    let top = r#"
        module Top {
            var feedback: logic;
            inst child: Child (
                i: feedback,
                o: feedback,
            );
        }
    "#
    .to_string();
    (child, top)
}

#[test]
fn comb_loop_diagnostic_traces_each_procedure_once() {
    const COUNT: usize = 32;
    let (child, top) = reverse_procedural_chain(COUNT, false);
    crate::comb_loop_detect::reset_traced_procedure_evaluation_count();

    let errors = analyze_multiple_inputs(&[&child, &top]);
    let sites = errors.iter().find_map(|error| match error {
        AnalyzerError::CombinationalLoop {
            dependency_sites, ..
        } => Some(dependency_sites.len()),
        _ => None,
    });

    assert_eq!(sites, Some(COUNT + 1), "{errors:#?}");
    assert_eq!(
        crate::comb_loop_detect::traced_procedure_evaluation_count(),
        1,
        "one selected path must not re-evaluate its procedure for every edge",
    );
}

#[test]
fn comb_loop_diagnostic_shares_excerpts_on_one_physical_line() {
    const COUNT: usize = 32;
    let (child, top) = reverse_procedural_chain(COUNT, true);
    let errors = analyze_multiple_inputs(&[&child, &top]);
    let (input, sites) = errors
        .iter()
        .find_map(|error| match error {
            AnalyzerError::CombinationalLoop {
                input,
                dependency_sites,
                ..
            } => Some((input, dependency_sites)),
            _ => None,
        })
        .expect("the closed child feedthrough is a loop");

    assert_eq!(sites.len(), COUNT + 1, "{errors:#?}");
    for site in sites {
        diagnostic_span_text(input, site).expect("every dependency site remains addressable");
    }
    let retained = input
        .sources
        .iter()
        .map(|source| source.text.len())
        .sum::<usize>();
    assert!(
        retained <= (child.len() + top.len()) * 3,
        "same-line sites retained {retained} bytes for {} bytes of input",
        child.len() + top.len(),
    );
}

fn serial_instance_chain(count: usize) -> (String, String, String) {
    let leaf = r#"
        module Pass (
            i: input  logic,
            o: output logic,
        ) {
            assign o = i;
        }
    "#
    .to_string();
    let links = (0..count - 1)
        .map(|index| format!("var link_{index}: logic;"))
        .collect::<Vec<_>>()
        .join("\n");
    let instances = (0..count)
        .map(|index| {
            let input = if index == 0 {
                "i".to_string()
            } else {
                format!("link_{}", index - 1)
            };
            let output = if index + 1 == count {
                "o".to_string()
            } else {
                format!("link_{index}")
            };
            format!("inst stage_{index}: Pass (i: {input}, o: {output});")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let chain = format!(
        r#"
        module Chain (
            i: input  logic,
            o: output logic,
        ) {{
            {links}
            {instances}
        }}
        "#,
    );
    let top = r#"
        module Top {
            var feedback: logic;
            inst chain: Chain (
                i: feedback,
                o: feedback,
            );
        }
    "#
    .to_string();
    (leaf, chain, top)
}

#[test]
fn comb_loop_diagnostic_resolves_serial_instances_by_declaration_index() {
    const COUNT: usize = 32;
    let (leaf, chain, top) = serial_instance_chain(COUNT);
    crate::comb_loop_detect::reset_diagnostic_instance_probe_count();

    let errors = analyze_multiple_inputs(&[&leaf, &chain, &top]);
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
            .count(),
        1,
        "{errors:#?}",
    );
    assert_eq!(
        crate::comb_loop_detect::diagnostic_instance_probe_count(),
        COUNT + 1,
        "each summarized edge should require exactly one direct instance lookup",
    );
}

#[test]
fn comb_loop_diagnostic_skips_duplicate_region_provenance() {
    const COUNT: usize = 32;
    let assignments = (0..COUNT)
        .map(|index| format!("assign o[{index}] = i[{index}];"))
        .collect::<Vec<_>>()
        .join("\n");
    let child = format!(
        r#"
        module Child (
            i: input  logic [{COUNT}],
            o: output logic [{COUNT}],
        ) {{
            {assignments}
        }}
        "#,
    );
    let top = format!(
        r#"
        module Top {{
            var feedback: logic [{COUNT}];
            inst child: Child (
                i: feedback,
                o: feedback,
            );
        }}
        "#,
    );
    crate::comb_loop_detect::reset_diagnostic_provenance_build_count();

    let errors = analyze_multiple_inputs(&[&child, &top]);
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
            .count(),
        1,
        "{errors:#?}",
    );
    assert_eq!(
        crate::comb_loop_detect::diagnostic_provenance_build_count(),
        1,
        "duplicate region loops must be rejected before provenance is built",
    );
}
