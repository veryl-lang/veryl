// Incomplete-effect boundary coverage for comb-loop analysis.
use super::*;

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
