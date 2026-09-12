// Incomplete-effect boundary coverage for comb-loop analysis.
use super::*;

#[test]
fn contiguous_instance_array_input_fits_a_small_partition_budget() {
    for count in [16, 16_384] {
        let last = count - 1;
        let code = format!(
            "module Pass (i: input logic[{count}], o: output logic) {{ assign o = i[{last}]; }}
             module Top (i: input logic[{count}], o: output logic) {{
                inst child: Pass (i: i[0:{last}], o: o);
             }}"
        );
        crate::comb_loop_detect::with_partition_work_limit(512, || {
            assert!(comb_loop_analysis_is_complete(&code));
            assert!(analyze(&code).is_empty());
        });
    }
}

#[test]
fn partition_expansion_limit_propagates_and_keeps_parent_cycles() {
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
        crate::comb_loop_detect::with_partition_work_limit(4096, || {
            assert_eq!(comb_loop_analysis_is_complete(&code), count == 8);
            let errors = analyze(&code);
            assert!(
                errors.iter().all(|error| match error {
                    AnalyzerError::CombinationalLoop { identifier, .. }
                    | AnalyzerError::UnassignVariable { identifier, .. } =>
                        identifier == "independent",
                    _ => false,
                }),
                "{errors:?}"
            );
            assert!(errors.iter().any(|error| matches!(error,
                AnalyzerError::CombinationalLoop { identifier, .. } if identifier == "independent"
            )), "{errors:?}");
        });
    }
}

#[test]
fn instance_source_guard_limit_preserves_independent_cycles() {
    for selector in [false, true] {
        for stages in [4, 64] {
            let ports = if selector {
                "i: i, o: o[gate(flags, i)]"
            } else {
                "i: gate(flags, i), o: o"
            };
            let width = if selector { 2 } else { 1 };
            let code = format!(
                "module Child (i: input logic, o: output logic) {{ assign o = i; }}
                 module Top (flags: input logic<{stages}>, i: input logic,
                             o: output logic<{width}>, independent: output logic) {{
                    function gate (s: input logic<{stages}>, x: input logic) -> logic {{
                        var v: logic;
                        v = x;
                        for index in 0..{stages} {{
                            if s[index] {{ v = !v; }} else {{ v = 0; }}
                        }}
                        return v;
                    }}
                    inst child: Child ({ports});
                    assign independent = independent;
                 }}"
            );
            // Each assignment has only one guard, so construction fits in
            // either case. Walking back from the result accumulates a growing
            // prefix and must share the guard budget with its caller.
            crate::comb_loop_detect::with_procedure_guard_limit(1024, || {
                let case = format!("selector={selector}, stages={stages}");
                assert_eq!(comb_loop_analysis_is_complete(&code), stages == 4, "{case}");
                let errors = analyze(&code);
                assert!(
                    errors.iter().all(|error| match error {
                        AnalyzerError::CombinationalLoop { .. } => true,
                        AnalyzerError::UnassignVariable { identifier, .. } =>
                            identifier == "independent",
                        _ => false,
                    }),
                    "{case}: {errors:?}"
                );
                let loops = errors
                    .iter()
                    .filter_map(|error| match error {
                        AnalyzerError::CombinationalLoop { identifier, .. } => {
                            Some(identifier.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(loops, ["independent"], "{case}: {errors:?}");
            });
        }
    }
}

#[test]
fn nested_runtime_loop_copy_limit_preserves_independent_cycles() {
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
                // The inner loop fits. Enclosing loops must also charge for
                // copying its generated SSA, even after imports are condensed
                // or when the final result is overwritten or discarded.
                crate::comb_loop_detect::with_procedure_import_limit(1024, || {
                    let case = format!("imported={imported}, {kind}, depth={depth}");
                    assert_eq!(comb_loop_analysis_is_complete(&code), depth == 1, "{case}");
                    let errors = analyze(&code);
                    assert!(
                        errors.iter().all(|error| match error {
                            AnalyzerError::CombinationalLoop { .. } => true,
                            AnalyzerError::UnassignVariable { identifier, .. } =>
                                identifier == "independent",
                            _ => false,
                        }),
                        "{case}: {errors:?}"
                    );
                    let loops = errors
                        .iter()
                        .filter_map(|error| match error {
                            AnalyzerError::CombinationalLoop { identifier, .. } => {
                                Some(identifier.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(loops, ["independent"], "{case}: {errors:?}");
                });
            }
        }
    }
}

#[test]
fn runtime_loop_import_limit_preserves_independent_cycles() {
    for kind in ["block", "overwritten", "separate", "function"] {
        for calls in [1, 16] {
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
                        for _index in 0..8 {{ if s {{ v = !v; }} else {{ v = 0; }} }}
                        return v;
                    }}
                    {body}
                    assign independent = independent;
                }}
                "#
            );
            crate::comb_loop_detect::with_procedure_import_limit(1024, || {
                assert_eq!(
                    comb_loop_analysis_is_complete(&code),
                    calls == 1,
                    "{kind}, calls={calls}"
                );
                let errors = analyze(&code);
                let loops = errors
                    .iter()
                    .filter_map(|error| match error {
                        AnalyzerError::CombinationalLoop { identifier, .. } => {
                            Some(identifier.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(loops, ["independent"], "{kind}, calls={calls}: {errors:?}");
            });
        }
    }
}

#[test]
fn procedural_import_limit_preserves_independent_cycles() {
    for kind in ["procedure", "instance_side_effect"] {
        for calls in [1, 16] {
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
                        for _index in 0..8 {{ if s {{ v = !v; }} else {{ v = 0; }} }}
                        return v;
                    }}
                    {body}
                    assign independent = independent;
                }}
                "#
            );
            crate::comb_loop_detect::with_procedure_import_limit(1024, || {
                assert_eq!(
                    comb_loop_analysis_is_complete(&code),
                    calls == 1,
                    "{kind}, calls={calls}"
                );
                let errors = analyze(&code);
                assert!(
                    errors.iter().all(|error| match error {
                        AnalyzerError::CombinationalLoop { .. }
                        | AnalyzerError::UnusedVariable { .. } => true,
                        AnalyzerError::UnassignVariable { identifier, .. } =>
                            identifier == "independent"
                                || (kind == "instance_side_effect" && identifier == "o"),
                        _ => false,
                    }),
                    "{kind}, calls={calls}: {errors:?}"
                );
                let loops = errors
                    .iter()
                    .filter_map(|error| match error {
                        AnalyzerError::CombinationalLoop { identifier, .. } => {
                            Some(identifier.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(loops, ["independent"], "{kind}, calls={calls}: {errors:?}");
            });
        }
    }
}

#[test]
fn procedural_import_limit_does_not_limit_direct_assignments() {
    let assignments = (0..1024)
        .map(|index| format!("o[{index}] = i;"))
        .collect::<String>();
    for runtime_loop in [false, true] {
        let body = if runtime_loop {
            format!("o = 0; for _iteration in 0..n {{ {assignments} }}")
        } else {
            assignments.clone()
        };
        let code = format!(
            "module Top (i: input logic, n: input u32, o: output logic<1024>) {{ always_comb {{ {body} }} }}"
        );
        crate::comb_loop_detect::with_procedure_import_limit(0, || {
            assert!(comb_loop_analysis_is_complete(&code));
            assert!(analyze(&code).is_empty());
        });
    }
}

#[test]
fn instance_actual_expansion_limit_keeps_independent_cycles() {
    for stages in [4, 64] {
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
        crate::comb_loop_detect::with_module_summary_limit(128, || {
            assert_eq!(comb_loop_analysis_is_complete(&code), stages == 4);
            let errors = analyze(&code);
            let loops = errors
                .iter()
                .filter_map(|error| match error {
                    AnalyzerError::CombinationalLoop { identifier, .. } => {
                        Some(identifier.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(loops, ["independent"], "stages={stages}: {errors:?}");
        });
    }
}

#[test]
fn procedural_guard_limit_counts_fragmented_case_ranges() {
    for fragmented in [false, true] {
        for iterations in [0, 64] {
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
                        for _i in 0..{iterations} {{
                            if s {{ v = !v; }} else {{ v = 0; }}
                        }}
                        return v;
                    }}
                    assign o = gate(sel, s, x);
                    assign independent = independent;
                 }}"
            );
            // Both continuations constrain only one case branch. Alternating
            // returns leave many disjoint ranges that each later if must copy;
            // contiguous returns leave just one range. The case join alone fits.
            crate::comb_loop_detect::with_procedure_guard_limit(4096, || {
                let case = format!("fragmented={fragmented}, iterations={iterations}");
                assert_eq!(
                    comb_loop_analysis_is_complete(&code),
                    !fragmented || iterations == 0,
                    "{case}"
                );
                let errors = analyze(&code);
                assert!(
                    errors.iter().all(|error| match error {
                        AnalyzerError::CombinationalLoop { .. } => true,
                        AnalyzerError::UnassignVariable { identifier, .. } =>
                            identifier == "independent",
                        _ => false,
                    }),
                    "{case}: {errors:?}"
                );
                let loops = errors
                    .iter()
                    .filter_map(|error| match error {
                        AnalyzerError::CombinationalLoop { identifier, .. } => {
                            Some(identifier.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(loops, ["independent"], "{case}: {errors:?}");
            });
        }
    }
}

#[test]
fn procedural_guard_limit_bounds_early_exits_and_preserves_independent_cycles() {
    for kind in ["return", "break", "runtime_break"] {
        for size in [2, 64, 256] {
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
            crate::comb_loop_detect::with_procedure_guard_limit(128, || {
                assert_eq!(
                    comb_loop_analysis_is_complete(&code),
                    size == 2,
                    "{kind}, {size}"
                );
                let errors = analyze(&code);
                let loops = errors
                    .iter()
                    .filter_map(|error| match error {
                        AnalyzerError::CombinationalLoop { identifier, .. } => {
                            Some(identifier.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(loops, ["independent"], "{kind}, {size}: {errors:?}");
            });
        }
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
fn comb_loop_search_limit_propagates_incomplete_without_inventing_a_diagnostic() {
    let code = r#"
        module Child (o: output logic) { assign o = ~o; }
        module Top (o: output logic) { inst child: Child(o); }
    "#;
    crate::comb_loop_detect::with_cycle_search_limit(0, || {
        assert!(!comb_loop_analysis_is_complete(code));
        assert!(
            analyze(code)
                .iter()
                .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
            "an exhausted search has no proven cycle"
        );
        assert!(
            comb_loop_analysis_is_complete(
                r#"
            module Top(i: input logic, o: output logic) { assign o = i; }
        "#
            ),
            "acyclic graphs require no search"
        );
    });
    assert!(comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_malformed_boundary_preserves_a_later_exact_loop() {
    let code = r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            assign a = b;
            always_comb {
                missing_function();
                b = a;
                o = b;
            }
        }
    "#;
    assert!(!comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "the continuous-only source must survive the boundary so the later exact loop is visible"
    );
}

#[test]
fn comb_loop_boundary_does_not_kill_a_disjoint_bit_owned_by_another_process() {
    let code = r#"
        module Top (
            o: output logic,
        ) {
            var state   : logic<2>;
            var feedback: logic;
            assign feedback = state[1];
            always_comb {
                missing_function();
                state[0] = 0;
            }
            always_comb {
                state[1] = feedback;
                o = state[1];
            }
        }
    "#;
    assert!(!comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "a boundary in the bit-0 writer must not erase the independent bit-1 loop"
    );
}

#[test]
fn comb_loop_partial_function_summary_preserves_a_later_exact_return_path() {
    let code = r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            function read_a () -> logic {
                missing_function();
                return a;
            }
            assign a = b;
            always_comb {
                b = read_a();
                o = b;
            }
        }
    "#;
    assert!(!comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "a partial callee must not discard the exact return dependency rebuilt after its boundary"
    );
}

#[test]
fn comb_loop_opaque_branch_does_not_erase_an_exact_sibling_branch() {
    let code = r#"
        module Top (
            cond: input  logic,
            o   : output logic,
        ) {
            var a: logic;
            var b: logic;
            assign b = a;
            always_comb {
                if cond {
                    a = b;
                } else {
                    a = 0;
                    missing_function();
                }
                o = a;
            }
        }
    "#;
    assert!(!comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "an opaque sibling branch must not erase the exact feedback branch"
    );
}

#[test]
fn comb_loop_zero_trip_path_preserves_preloop_value_after_unknown_effect() {
    let code = r#"
        module Top (
            n: input  logic<32>,
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            var c: logic;
            assign a = c;
            always_comb {
                b = a;
                for _index in 0..n {
                    missing_function();
                }
                c = b;
                o = c;
            }
        }
    "#;
    assert!(!comb_loop_analysis_is_complete(code));
    assert!(
        analyze(code)
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "the zero-trip path must retain the exact pre-loop value consumed after the loop"
    );
}
