// Incomplete-effect boundary coverage for comb-loop analysis.
use super::*;

/// A gate made of `stages` sequential branches, either as a constant loop or
/// as the equivalent straight-line statements. A constant loop is evaluated
/// one iteration at a time, so both forms cost the same.
fn gate_stages(unrolled: bool, stages: usize, condition: impl Fn(&str) -> String) -> String {
    if unrolled {
        (0..stages)
            .map(|index| {
                format!(
                    "if {} {{ v = !v; }} else {{ v = 0; }}",
                    condition(&index.to_string())
                )
            })
            .collect()
    } else {
        let condition = condition("index");
        let iterator = if condition.contains("index") {
            "index"
        } else {
            "_index"
        };
        format!(
            "for {iterator} in 0..{stages} {{ if {condition} {{ v = !v; }} else {{ v = 0; }} }}"
        )
    }
}

/// One case of a workload: a label, its source, and whether it is a light
/// case, one that the analysis keeps small.
struct StepCase {
    label: String,
    code: String,
    light: bool,
}

impl StepCase {
    fn new(label: String, code: String, light: bool) -> Self {
        Self { label, code, light }
    }
}

fn reported_loops(errors: &[AnalyzerError]) -> Vec<&str> {
    errors
        .iter()
        .filter_map(|error| match error {
            AnalyzerError::CombinationalLoop { identifier, .. } => Some(identifier.as_str()),
            _ => None,
        })
        .collect()
}

/// Check a workload against the steps of its light cases. Given twice that
/// many steps per module, the most stages sharing them may waste, every
/// light case completes and finds the independent loop, and every heavy case
/// stops as incomplete without reporting a loop that does not exist.
/// `allowed` accepts the other diagnostics of a case.
fn check_against_light_steps(
    cases: &[StepCase],
    allowed: impl Fn(&StepCase, &AnalyzerError) -> bool,
) {
    let limit = cases
        .iter()
        .filter(|case| case.light)
        .map(|case| {
            crate::comb_loop_detect::reset_steps_taken();
            assert!(comb_loop_analysis_is_complete(&case.code), "{}", case.label);
            crate::comb_loop_detect::steps_taken()
        })
        .max()
        .expect("a workload has a light case")
        * 2;
    crate::comb_loop_detect::with_step_limit(limit, || {
        for case in cases {
            let label = &case.label;
            assert_eq!(
                comb_loop_analysis_is_complete(&case.code),
                case.light,
                "{label} with {limit} steps"
            );
            let errors = analyze(&case.code);
            assert!(
                errors.iter().all(|error| match error {
                    AnalyzerError::CombinationalLoop { .. } => true,
                    error => allowed(case, error),
                }),
                "{label}: {errors:?}"
            );
            let loops = reported_loops(&errors);
            if case.light {
                assert_eq!(loops, ["independent"], "{label}: {errors:?}");
            } else {
                assert!(
                    loops.iter().all(|identifier| *identifier == "independent"),
                    "{label}: {errors:?}"
                );
            }
        }
    });
}

fn unassigned_independent(error: &AnalyzerError) -> bool {
    matches!(error, AnalyzerError::UnassignVariable { identifier, .. } if identifier == "independent")
}

#[test]
fn instance_source_guard_limit_preserves_independent_cycles() {
    let mut cases = Vec::new();
    for (selector, unrolled) in [(false, false), (false, true), (true, false), (true, true)] {
        for stages in [4, 64] {
            let ports = if selector {
                "i: i, o: o[gate(flags, i)]"
            } else {
                "i: gate(flags, i), o: o"
            };
            let width = if selector { 2 } else { 1 };
            let gate = gate_stages(unrolled, stages, |index| format!("s[{index}]"));
            let code = format!(
                "module Child (i: input logic, o: output logic) {{ assign o = i; }}
                 module Top (flags: input logic<{stages}>, i: input logic,
                             o: output logic<{width}>, independent: output logic) {{
                    function gate (s: input logic<{stages}>, x: input logic) -> logic {{
                        var v: logic;
                        v = x;
                        {gate}
                        return v;
                    }}
                    inst child: Child ({ports});
                    assign independent = independent;
                 }}"
            );
            // Each assignment has only one guard. Walking back from the
            // result accumulates a growing prefix.
            cases.push(StepCase::new(
                format!("selector={selector}, unrolled={unrolled}, stages={stages}"),
                code,
                stages == 4,
            ));
        }
    }
    check_against_light_steps(&cases, |case, error| match error {
        // The runtime output select is part of the case, and is reported
        // separately.
        AnalyzerError::NonConstantOutputSelect { .. } => case.label.starts_with("selector=true"),
        error => unassigned_independent(error),
    });
}

#[test]
fn nested_runtime_loop_copy_limit_preserves_independent_cycles() {
    let mut cases = Vec::new();
    for imported in [false, true] {
        for kind in ["block", "overwritten", "function"] {
            for depth in [1, 16] {
                let destination = if kind == "function" {
                    "_discarded"
                } else {
                    "o"
                };
                let statements = if imported {
                    format!("{destination} = gate(1'b1, i);")
                } else {
                    format!(
                        "{destination} = i; {}",
                        format!("{destination} = !{destination};").repeat(32)
                    )
                };
                let loops = (0..depth).rev().fold(statements, |body, index| {
                    format!("for _iteration{index} in 0..n {{ {body} }}")
                });
                let body = if kind == "function" {
                    format!(
                        "function run () -> logic {{
                            var _discarded: logic;
                            _discarded = 0; {loops} return 0;
                         }}
                         assign o = run();"
                    )
                } else {
                    let overwrite = if kind == "overwritten" { "o = 0;" } else { "" };
                    format!("always_comb {{ o = 0; {loops} {overwrite} }}")
                };
                let function = if imported {
                    "function gate (s: input logic, x: input logic) -> logic {
                        var v: logic;
                        v = x;
                        for _index in 0..8 { if s { v = !v; } else { v = 0; } }
                        return v;
                     }"
                } else {
                    ""
                };
                let code = format!(
                    "module Top (i: input logic, n: input u32,
                                 o: output logic, independent: output logic) {{
                        {function} {body}
                        assign independent = independent;
                     }}"
                );
                // Enclosing loops also charge for copying the generated SSA of
                // an inner loop, even after imports are condensed or when the
                // final result is overwritten or discarded.
                cases.push(StepCase::new(
                    format!("imported={imported}, {kind}, depth={depth}"),
                    code,
                    depth == 1,
                ));
            }
        }
    }
    check_against_light_steps(&cases, |_, error| unassigned_independent(error));
}

#[test]
fn runtime_loop_import_limit_preserves_independent_cycles() {
    let mut cases = Vec::new();
    for kind in ["block", "overwritten", "separate", "function"] {
        for (calls, unrolled) in [(1, true), (16, true), (16, false)] {
            let gate = gate_stages(unrolled, 16, |_| "s".to_string());
            let destination = if kind == "function" {
                "_discarded"
            } else {
                "o"
            };
            let assignments = (0..calls)
                .map(|index| format!("{destination}[{index}] = gate(1'b1, i);"))
                .collect::<Vec<_>>();
            let loops = if kind == "separate" {
                assignments
                    .iter()
                    .map(|assign| format!("for _iteration in 0..n {{ {assign} }}"))
                    .collect::<String>()
            } else {
                format!("for _iteration in 0..n {{ {} }}", assignments.join("\n"))
            };
            let body = if kind == "function" {
                format!(
                    "function run () -> logic {{
                        var _discarded: logic<{calls}>;
                        _discarded = 0; {loops} return 0;
                     }}
                     assign o = run();"
                )
            } else {
                let overwrite = if kind == "overwritten" { "o = 0;" } else { "" };
                format!("always_comb {{ o = 0; {loops} {overwrite} }}")
            };
            let code = format!(
                r#"
                module Top (i: input logic, n: input u32,
                            o: output logic<{calls}>, independent: output logic) {{
                    function gate (s: input logic, x: input logic) -> logic {{
                        var v: logic;
                        v = x;
                        {gate}
                        return v;
                    }}
                    {body}
                    assign independent = independent;
                }}
                "#
            );
            // The stages are copied by every repeated transfer.
            cases.push(StepCase::new(
                format!("{kind}, calls={calls}, unrolled={unrolled}"),
                code,
                calls == 1,
            ));
        }
    }
    check_against_light_steps(&cases, |_, error| unassigned_independent(error));
}

#[test]
fn procedural_import_limit_preserves_independent_cycles() {
    for kind in ["procedure", "instance_side_effect"] {
        let mut cases = Vec::new();
        for (calls, unrolled) in [(1, true), (16, true), (16, false)] {
            let gate = gate_stages(unrolled, 8, |_| "s".to_string());
            let body = if kind == "procedure" {
                let assignments = (0..calls)
                    .map(|index| format!("o[{index}] = gate(1'b1, i);"))
                    .collect::<String>();
                format!("always_comb {{ {assignments} }}")
            } else {
                let functions = (0..calls)
                    .map(|index| format!(
                        "function sample{index} () -> logic {{ o[{index}] = gate(1'b1, i); return 0; }}"
                    ))
                    .collect::<String>();
                let actual = (0..calls)
                    .map(|index| format!("sample{index}()"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{functions} inst sink: Sink (i: {{{actual}}});")
            };
            let code = format!(
                r#"
                module Sink (i: input logic<{calls}>) {{}}
                module Top (i: input logic, o: output logic<{calls}>, independent: output logic) {{
                    function gate (s: input logic, x: input logic) -> logic {{
                        var v: logic;
                        v = x;
                        {gate}
                        return v;
                    }}
                    {body}
                    assign independent = independent;
                }}
                "#
            );
            cases.push(StepCase::new(
                format!("{kind}, calls={calls}, unrolled={unrolled}"),
                code,
                calls == 1,
            ));
        }
        check_against_light_steps(&cases, |case, error| match error {
            AnalyzerError::UnusedVariable { .. } => true,
            AnalyzerError::UnassignVariable { identifier, .. } => {
                identifier == "independent"
                    || (case.label.starts_with("instance_side_effect") && identifier == "o")
            }
            _ => false,
        });
    }
}

#[test]
fn instance_actual_expansion_limit_keeps_independent_cycles() {
    let cases = [4, 64]
        .into_iter()
        .map(|stages| {
            let stages_code = "y = !y;".repeat(stages);
            let code = format!(
                r#"
                module Child (i: input logic, o: output logic) {{ assign o = i; }}
                module Top (i: input logic, o: output logic, independent: output logic) {{
                    function chain (x: input logic) -> logic {{
                        var y: logic;
                        y = x;
                        {stages_code}
                        return y;
                    }}
                    inst child: Child (i: chain(i), o: o);
                    assign independent = independent;
                }}
            "#
            );
            StepCase::new(format!("stages={stages}"), code, stages == 4)
        })
        .collect::<Vec<_>>();
    check_against_light_steps(&cases, |_, error| unassigned_independent(error));
}

#[test]
fn procedural_guard_limit_counts_fragmented_case_ranges() {
    let mut cases = Vec::new();
    for fragmented in [false, true] {
        for (iterations, unrolled) in [(0, true), (64, true), (64, false)] {
            let gate = gate_stages(unrolled, iterations, |_| "s".to_string());
            let arms = (0..64)
                .map(|index| {
                    let returns = if fragmented {
                        index % 2 == 0
                    } else {
                        index < 32
                    };
                    let body = if returns { "return 0;" } else { "" };
                    format!("{index}: {{ {body} }}")
                })
                .collect::<String>();
            let code = format!(
                "module Top (sel: input u32, s: input logic, x: input logic,
                             o: output logic, independent: output logic) {{
                    function gate (sel: input u32, s: input logic, x: input logic) -> logic {{
                        case sel {{ {arms} default: {{}} }}
                        var v: logic;
                        v = x;
                        {gate}
                        return v;
                    }}
                    assign o = gate(sel, s, x);
                    assign independent = independent;
                 }}"
            );
            // Both continuations constrain only one case branch. Alternating
            // returns leave many disjoint ranges that each later if must copy;
            // contiguous returns leave just one range. The case join alone fits.
            cases.push(StepCase::new(
                format!("fragmented={fragmented}, iterations={iterations}, unrolled={unrolled}"),
                code,
                !fragmented || iterations == 0,
            ));
        }
    }
    check_against_light_steps(&cases, |_, error| unassigned_independent(error));
}

#[test]
fn procedural_guard_limit_bounds_early_exits_and_preserves_independent_cycles() {
    for kind in ["return", "break", "runtime_break"] {
        let cases = [2, 64, 256]
            .into_iter()
            .map(|size| {
                let exits = (0..size)
                    .map(|index| {
                        if kind == "return" {
                            format!("if flags[{index}] {{ return data; }}")
                        } else {
                            format!("if flags[{index}] {{ break; }}")
                        }
                    })
                    .collect::<String>();
                let body = if kind == "return" {
                    format!(
                        r#"
                        function first (flags: input logic<{size}>, data: input logic) -> logic {{
                            {exits}
                            return 0;
                        }}
                        assign o = first(flags, data);
                    "#
                    )
                } else {
                    let bound = if kind == "break" { "1" } else { "n" };
                    format!(
                        r#"
                        always_comb {{
                            o = o;
                            for _index in 0..{bound} {{ {exits} o = data; }}
                            o = 0;
                        }}
                    "#
                    )
                };
                let code = format!(
                    r#"
                    module Top (flags: input logic<{size}>, data: input logic,
                                n: input logic<32>, o: output logic, independent: output logic) {{
                        {body}
                        assign independent = independent;
                    }}
                "#
                );
                StepCase::new(format!("{kind}, {size}"), code, size == 2)
            })
            .collect::<Vec<_>>();
        check_against_light_steps(&cases, |_, error| {
            unassigned_independent(error) || matches!(error, AnalyzerError::UnassignVariable { .. })
        });
    }
}

#[test]
fn comb_loop_malformed_effect_is_a_causal_barrier() {
    // Why this case exists: a rejected statement may have unknown side
    // effects, so a cycle which crosses it is not proven. The malformed
    // procedure must not suppress a separate exact cycle in another procedure.
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            var c: logic;
            var d: logic;
            always_comb {
                a = b;
                missing_function();
                b = a;
            }
            always_comb {
                c = d;
                d = c;
                o = d;
            }
        }
        "#,
    );
    let loops = errors
        .iter()
        .filter_map(|error| match error {
            AnalyzerError::CombinationalLoop { identifier, .. } => Some(identifier.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        loops.len() == 1 && matches!(loops[0], "c" | "d"),
        "only the cycle independent of the malformed barrier is proven: {errors:#?}"
    );
}

#[test]
fn comb_loop_inout_boundary_does_not_prove_hard_feedback() {
    let errors = analyze(
        r#"
        module Top (
            io: inout  tri logic,
            o : output     logic,
        ) {
            assign io = o;
            assign o = io;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "an externally driven inout boundary cannot prove a hard loop: {errors:#?}"
    );
}

#[test]
fn comb_loop_dynamic_for_bound_over_known_regions_is_complete_and_loop_free() {
    let code = r#"
        module Top (
            n   : input  logic<32>,
            data: input  logic,
            o   : output logic,
        ) {
            var value: logic;
            always_comb {
                value = 0;
                for _index in 0..n {
                    value = data;
                }
                o = value;
            }
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_dynamic_for_bound_over_known_regions_detects_feedback() {
    // False-negative guard: the loop may execute zero times, but an
    // unconstrained runtime bound may also execute the feedback body. Treating
    // the existence of the zero-trip path as proof that the body is unreachable
    // would miss a realizable combinational loop.
    let code = r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _index in 0..n {
                    a = b;
                    b = a;
                }
                o = b;
            }
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_const_zero_for_bound_skips_unreachable_feedback() {
    // True negative: unlike the runtime-bound case above, this range is proven
    // empty, so the feedback-shaped body is unreachable.
    let code = r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _index in 0..0 {
                    a = b;
                    b = a;
                }
                o = b;
            }
        }
    "#;
    assert!(comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_dynamic_for_bound_with_unknown_effect_is_incomplete() {
    let code = r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _index in 0..n {
                    a = b;
                    missing_function();
                    b = a;
                }
                o = b;
            }
        }
    "#;
    assert!(!comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_oversized_constant_range_is_incomplete_without_false_feedback() {
    let evaluate_size_limit = Metadata::create_default("prj")
        .unwrap()
        .build
        .evaluate_size_limit;
    let code = format!(
        r#"
        module Top (
            o: output logic,
        ) {{
            var value   : logic [3];
            var feedback: logic;
            assign feedback = value[2];
            always_comb {{
                value = '{{default: 0}};
                value[1] = feedback;
                for index in 0..{} {{
                    value[index + 1] = value[index];
                }}
                o = feedback;
            }}
        }}
        "#,
        evaluate_size_limit + 1
    );

    assert!(!comb_loop_analysis_is_complete(&code));
    let errors = analyze(&code);
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "the expansion limit must not create a combinational-loop diagnostic: {errors:#?}"
    );
}

#[test]
fn comb_loop_dynamic_for_incomplete_effect_keeps_an_independent_cycle() {
    let code = r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            var c: logic;
            var d: logic;
            always_comb {
                for _index in 0..n {
                    a = b;
                    missing_function();
                    b = a;
                }
            }
            always_comb {
                c = d;
                d = c;
                o = d;
            }
        }
    "#;
    assert!(!comb_loop_analysis_is_complete(code));
    let loops = analyze(code)
        .into_iter()
        .filter(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
        .count();
    assert_eq!(loops, 1);
}

#[test]
fn comb_loop_modport_members_do_not_gain_cross_member_feedthrough() {
    let errors = analyze(
        r#"
        interface Bus {
            var request : logic;
            var response: logic;
            modport port {
                request : input,
                response: output,
            }
        }
        module Child (
            bus: modport Bus::port,
        ) {
            assign bus.response = 0;
        }
        module Top (
            o: output logic,
        ) {
            inst bus: Bus;
            inst child: Child (
                bus: bus,
            );
            assign bus.request = bus.response;
            assign o = bus.request;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "disjoint modport members must not acquire an invented return edge: {errors:#?}"
    );
}

#[test]
fn comb_loop_recursive_module_is_incomplete_not_hard_feedback() {
    let errors = analyze_with_large_stack(
        r#"
        module Recursive {
            inst next: Recursive;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "recursive hierarchy is incomplete, not proof of a hard loop: {errors:#?}"
    );
}

#[test]
fn comb_loop_unresolved_hierarchy_is_incomplete_not_hard_feedback() {
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            inst missing: Missing;
            assign o = 0;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "unresolved hierarchy is incomplete, not proof of a hard loop: {errors:#?}"
    );
}

#[test]
fn comb_loop_opaque_component_does_not_hide_independent_cycle() {
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var opaque_in : logic;
            var opaque_out: logic;
            var a: logic;
            var b: logic;
            inst ext: $sv::Ext (
                i_data: opaque_in,
                o_data: opaque_out,
            );
            assign opaque_in = opaque_out;
            assign a = b;
            assign b = a;
            assign o = a;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "an opaque component must not suppress a separate proven loop: {errors:#?}"
    );
}

#[test]
fn comb_loop_unresolved_hierarchy_does_not_hide_independent_cycle() {
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            inst missing: Missing;
            assign a = b;
            assign b = a;
            assign o = a;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "unresolved hierarchy must not suppress a separate proven loop: {errors:#?}"
    );
}

#[test]
fn partition_sweep_keeps_fragmented_modules_complete_and_parent_cycles() {
    for count in [8, 64] {
        let assignments = (0..count)
            .map(|index| {
                format!("assign o[{index}] = mem[{index}][{index}] ^ mem[index][{index}];")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let code = format!(
            "module Fragmented (index: input u32, mem: input logic<{count}>[{count}],
                                o: output logic<{count}>) {{
                {assignments}
             }}
             module Top (index: input u32, mem: input logic<{count}>[{count}],
                         o: output logic<{count}>, independent: output logic) {{
                inst child: Fragmented (index: index, mem: mem, o: o);
                assign independent = independent;
             }}"
        );
        assert!(comb_loop_analysis_is_complete(&code), "count={count}");
        let errors = analyze(&code);
        assert!(
            errors.iter().all(|error| match error {
                AnalyzerError::CombinationalLoop { identifier, .. }
                | AnalyzerError::UnassignVariable { identifier, .. } => identifier == "independent",
                _ => false,
            }),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| matches!(error,
                AnalyzerError::CombinationalLoop { identifier, .. } if identifier == "independent"
            )),
            "{errors:?}"
        );
    }
}

#[test]
fn regional_reads_take_module_steps() {
    // Every single-bit write leaves one more fragment in the write history,
    // and every later read of a different bit resolves through all of them.
    let code = |reads: usize| {
        let writes = (0..64)
            .map(|bit| format!("v[{bit}] = d[{bit}];"))
            .collect::<String>();
        let reads = (0..reads)
            .map(|bit| format!("o[{bit}] = v[{bit}];"))
            .collect::<String>();
        format!(
            "module Top (d: input logic<64>, o: output logic<64>) {{
                var v: logic<64>;
                always_comb {{
                    o = 0;
                    {writes}
                    {reads}
                }}
            }}"
        )
    };
    let steps = |reads| {
        crate::comb_loop_detect::reset_steps_taken();
        assert!(comb_loop_analysis_is_complete(&code(reads)));
        crate::comb_loop_detect::steps_taken()
    };
    // Each read passes at least half of the fragments.
    let (without, with) = (steps(0), steps(64));
    assert!(with - without >= 64 * 32, "{without} -> {with}");
}

#[test]
fn procedure_out_of_steps_does_not_starve_later_procedures() {
    // The heavy procedure copies sixteen unrolled gates through a runtime
    // loop; the light ones close a loop between themselves.
    let gate = gate_stages(true, 16, |_| "s".to_string());
    let heavy = format!(
        "always_comb {{ o = 0; for _iteration in 0..n {{ {} }} }}",
        (0..16)
            .map(|index| format!("o[{index}] = gate(1'b1, i);"))
            .collect::<String>()
    );
    let light = "always_comb { a = b; } always_comb { b = a; }";
    let code = |procedures: &str| {
        format!(
            r#"
            module Top (i: input logic, n: input u32, o: output logic<16>) {{
                var a: logic;
                var b: logic;
                function gate (s: input logic, x: input logic) -> logic {{
                    var v: logic;
                    v = x;
                    {gate}
                    return v;
                }}
                {procedures}
            }}
            "#
        )
    };
    // The light procedures complete on their first allowance, and the heavy
    // one needs more than every step.
    let limit = crate::comb_loop_detect::FIRST_ALLOWANCE * 8;
    crate::comb_loop_detect::reset_steps_taken();
    assert!(comb_loop_analysis_is_complete(&code(light)));
    assert!(crate::comb_loop_detect::steps_taken() < crate::comb_loop_detect::FIRST_ALLOWANCE);
    crate::comb_loop_detect::reset_steps_taken();
    assert!(comb_loop_analysis_is_complete(&code(&heavy)));
    assert!(crate::comb_loop_detect::steps_taken() > limit);
    crate::comb_loop_detect::with_step_limit(limit, || {
        for procedures in [format!("{heavy} {light}"), format!("{light} {heavy}")] {
            let code = code(&procedures);
            assert!(!comb_loop_analysis_is_complete(&code));
            let errors = analyze(&code);
            assert!(
                errors
                    .iter()
                    .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
                "{procedures}: {errors:?}"
            );
        }
    });
}
