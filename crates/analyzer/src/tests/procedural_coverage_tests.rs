use super::*;

#[test]
fn caller_defaults_cover_captured_storage() {
    for body in ["value = 0; update(en);", "update(en); value = 0;"] {
        let code = format!(
            r#"
            module Top(en: input logic, o: output logic) {{
                var value: logic;
                function update(en: input logic) {{
                    if en {{ value = 1; }}
                }}
                always_comb {{
                    {body}
                    o = value;
                }}
            }}
        "#
        );
        let errors = analyze(&code);
        assert!(errors.is_empty(), "{body}: {errors:#?}");
    }
}

#[test]
fn caller_default_does_not_initialize_an_output_formal() {
    let code = r#"
        module Top(en: input logic, o: output logic) {
            function update(en: input logic, result: output logic) {
                if en { result = 1; }
            }
            always_comb {
                o = 0;
                update(en, o);
            }
        }
    "#;
    let errors = analyze(code);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let AnalyzerError::UncoveredBranch {
        error_locations, ..
    } = &errors[0]
    else {
        panic!("expected the callee's unassigned output path: {errors:#?}");
    };
    assert_eq!(error_locations.len(), 1);
    assert_eq!(
        error_locations[0].offset(),
        code.find("result = 1").unwrap()
    );
}

#[test]
fn coverage_is_checked_at_the_process_exit() {
    let errors = analyze(
        r#"
        module Top(en: input logic, o: output logic) {
            always_comb {
                if en { o = 1; }
                o = 0;
            }
        }
    "#,
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn explicit_self_read_is_assigned_but_cyclic() {
    let errors = analyze(
        r#"
        module Top(o: output logic) {
            always_comb { o = !o; }
        }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{errors:#?}"
    );
}

#[test]
fn constant_argument_selects_a_captured_write() {
    let errors = analyze(
        r#"
        module Top(o: output logic) {
            var value: logic;
            function update(en: input logic) {
                if en { value = 1; }
            }
            always_comb {
                update(1'b1);
                o = value;
            }
        }
    "#,
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn constant_loop_inside_a_function_covers_its_output() {
    let errors = analyze(
        r#"
        module Top(stop: input logic, o: output logic) {
            function update(stop: input logic, result: output logic) {
                for i in 0..2 {
                    if i == 1 { result = 1; }
                    if i == 1 && stop { break; }
                }
            }
            always_comb { update(stop, o); }
        }
    "#,
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn dynamic_copy_out_reports_the_call_site() {
    let code = r#"
        module Top(index: input logic<2>, o: output logic<4>) {
            function set_one(result: output logic) { result = 1; }
            always_comb { set_one(o[index]); }
        }
    "#;
    let errors = analyze(code);
    assert!(
        errors.iter().all(|error| matches!(
            error,
            AnalyzerError::UnassignVariable { .. } | AnalyzerError::UncoveredBranch { .. }
        )),
        "{errors:#?}"
    );
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
    let error_locations = coverage[0];
    assert_eq!(error_locations.len(), 1);
    assert_eq!(
        error_locations[0].offset(),
        code.find("set_one(o[index])").unwrap()
    );
}

#[test]
fn a_skipped_captured_write_preserves_the_callers_feedback() {
    let errors = analyze(
        r#"
        module Top(n: input u32, o: output logic) {
            var value: logic;
            function update(n: input u32) {
                for _i in 0..n { value = 0; }
            }
            always_comb {
                value = o;
                update(n);
                o = value;
            }
        }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{errors:#?}"
    );
}

#[test]
fn a_zero_trip_call_keeps_inverting_feedback_from_the_caller() {
    let code = r#"
        module Top(n: input u32, o: output logic) {
            var value: logic;
            function update(n: input u32) {
                for _i in 0..n { value = 0; }
            }
            always_comb {
                value = !o;
                update(n);
                o = value;
            }
        }
    "#;
    // At n=0 the call changes nothing: o = !o. Assignment coverage is
    // complete, but that does not make the remaining feedback valid.
    assert!(comb_loop_analysis_is_complete(code));
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{errors:#?}"
    );
}

#[test]
fn uncovered_assignment_does_not_replace_the_cycle_diagnostic() {
    let code = r#"
        module Top(n: input u32, o: output logic) {
            var value: logic;
            function update(n: input u32) {
                for _i in 0..n { value = !value; }
            }
            always_comb {
                update(n);
                o = value;
            }
        }
    "#;
    // n=0 leaves value unassigned; n=1 explicitly reads and inverts it.
    // Neither finding is a reason to suppress the other.
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{errors:#?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
}

#[test]
fn incomplete_coverage_query_does_not_suppress_a_proven_cycle() {
    crate::comb_loop_detect::with_procedure_guard_limit(1, || {
        let code = r#"
            module Top(o: output logic) {
                always_comb { o = !o; }
            }
        "#;
        // This small query allowance cannot walk even the assignment effect,
        // but straight-line lowering and the value dependency are complete.
        assert!(comb_loop_analysis_is_complete(code));
        let errors = analyze(code);
        assert!(
            errors
                .iter()
                .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
            "{errors:#?}"
        );
    });
}

#[test]
fn output_formals_do_not_retain_a_previous_calls_value() {
    let errors = analyze(
        r#"
        module Top(o: output logic) {
            var discarded: logic;
            function update(en: input logic, data: input logic, result: output logic) {
                if en { result = data; }
            }
            always_comb {
                update(1'b1, o, discarded);
                update(1'b0, 1'b0, o);
            }
        }
    "#,
    );
    // The second automatic output is uninitialized, not the value of o that
    // the first call copied out. The incomplete function is still diagnosed.
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
}

#[test]
fn runtime_loop_retention_survives_branches_and_function_summaries() {
    for body in [
        "held = data;",
        "if en { held = data; }",
        "if en { break; } held = data;",
        "for _j in 0..m { held = data; }",
    ] {
        for in_function in [false, true] {
            let statements = format!("for _i in 0..n {{ {body} }}");
            let (function, invocation) = if in_function {
                (
                    format!("function update() {{ {statements} }}"),
                    "update();".into(),
                )
            } else {
                (String::new(), statements)
            };
            let code = format!(
                r#"
                module Top(n: input u32, m: input u32, en: input logic,
                           data: input logic, o: output logic) {{
                    var held: logic;
                    {function}
                    always_comb {{ {invocation} o = held; }}
                }}
            "#
            );
            assert!(comb_loop_analysis_is_complete(&code), "{code}");
            let errors = analyze(&code);
            assert_eq!(errors.len(), 1, "{code}: {errors:#?}");
            assert!(
                matches!(&errors[0], AnalyzerError::UncoveredBranch { identifier, .. }
                if identifier == "held"),
                "{code}: {errors:#?}"
            );
        }
    }
}

#[test]
fn a_conditional_captured_write_keeps_the_callers_feedback() {
    let code = r#"
        module Top(en: input logic, o: output logic) {
            var value: logic;
            function update() { if en { value = 0; } }
            always_comb {
                value = !o;
                update();
                o = value;
            }
        }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !matches!(error, AnalyzerError::UncoveredBranch { .. })),
        "{errors:#?}"
    );
}

#[test]
fn runtime_loop_bound_feedback_crosses_a_module_boundary() {
    let code = r#"
        module Hold(n: input logic<2>, o: output logic) {
            var held: logic;
            always_comb {
                for i in 0..n { held = i[0]; }
                o = held;
            }
        }
        module Top(o: output logic) {
            var n: logic<2>;
            assign n = {1'b1, o};
            inst u: Hold(n: n, o: o);
        }
    "#;
    // n is 2 or 3. The final held bit is the inverse of o, so this is
    // real feedback through the loop bound, not the n=0 retention path.
    assert!(comb_loop_analysis_is_complete(code));
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "{errors:#?}"
    );
}
