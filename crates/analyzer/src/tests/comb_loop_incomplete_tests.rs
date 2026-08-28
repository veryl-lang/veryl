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
