use super::*;
use miette::{Diagnostic, Severity};
use veryl_metadata::ResetType;

fn analyze_with_reset_type(code: &str, reset_type: ResetType) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.build.reset_type = reset_type;
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();

    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.extend(Analyzer::analyze_post_pass2(&ir));
    errors
}

#[test]
fn statement_after_if_reset_reset_types() {
    for (reset_type, warned) in [
        ("reset", true),
        ("reset_async_low", true),
        ("reset_async_high", true),
        ("reset_sync_low", false),
        ("reset_sync_high", false),
    ] {
        // Generic reset must stay portable even when this project uses a
        // synchronous reset. Explicit types must override the build setting.
        for configured_type in [
            ResetType::AsyncLow,
            ResetType::AsyncHigh,
            ResetType::SyncLow,
            ResetType::SyncHigh,
        ] {
            for event_list in ["(clk, rst)", "(clk)", ""] {
                let code = format!(
                    r#"
                    module ModuleA (
                        clk: input clock,
                        rst: input {reset_type},
                        a: input logic,
                        b: input logic,
                        x: output logic,
                        y: output logic,
                    ) {{
                        always_ff {event_list} {{
                            if_reset {{
                                x = 0;
                            }} else {{
                                x = a;
                            }}
                            if b {{
                                y = 1;
                            }}
                        }}
                    }}
                    "#
                );
                let errors = analyze_with_reset_type(&code, configured_type);
                assert_eq!(
                    errors.len(),
                    usize::from(warned),
                    "{reset_type}, {configured_type:?}, {event_list}: {errors:?}"
                );
                if warned {
                    assert!(matches!(
                        errors[0],
                        AnalyzerError::StatementAfterIfReset { .. }
                    ));
                    assert_eq!(errors[0].severity(), Some(Severity::Warning));
                    assert_eq!(
                        errors[0].code().unwrap().to_string(),
                        "statement_after_if_reset"
                    );
                }
            }
        }
    }
}

#[test]
fn statement_after_if_reset_statement_kinds() {
    for statement in [
        "x = a;",
        "if a { x = 1; } else { x = 0; }",
        "for _i in 0..2 { x = a; }",
        "case a { 0: x = 0; default: x = 1; }",
        "switch { a: x = 1; default: x = 0; }",
        "{x} = a;",
        "let _next: logic = a;",
        "$display(x);",
    ] {
        let code = format!(
            r#"
            module ModuleA (
                clk: input clock,
                rst: input reset,
                a: input logic,
                x: output logic,
            ) {{
                always_ff {{
                    if_reset {{ x = 0; }} else {{ x = a; }}
                    {statement}
                }}
            }}
            "#
        );
        let errors = analyze(&code);
        assert_eq!(errors.len(), 1, "{statement}: {errors:?}");
        let AnalyzerError::StatementAfterIfReset { error_location, .. } = &errors[0] else {
            panic!("{statement}: {errors:?}");
        };
        assert_eq!(
            &code[error_location.offset()..error_location.offset() + error_location.len()],
            statement,
        );
    }
}

#[test]
fn statement_after_if_reset_multiple_statements() {
    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset,
        a: input logic,
        x: output logic,
    ) {
        always_ff {
            if_reset { x = 0; }
            x = a;
            block {
                if a { x = 1; }
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert_eq!(errors.len(), 2);
    assert!(
        errors
            .iter()
            .all(|x| matches!(x, AnalyzerError::StatementAfterIfReset { .. }))
    );
}

#[test]
fn statement_after_if_reset_valid_layouts() {
    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset,
        a: input logic,
        b: input logic,
        x: output logic,
        y: output logic,
    ) {
        always_ff {
            if_reset {
                x = 0;
            } else if a {
                x = 1;
            } else {
                let next: logic = b;
                x = next;
                if b { x = 0; }
            }
            #[allow(unassign_variable)]
            var _unused: logic;
            const _UNUSED: u32 = 1;
            gen _T: type = u32;
        }
        // A following clock-only block must not inherit the reset context.
        always_ff (clk) {
            y = a;
            if b { y = 1; }
        }
    }
    "#;
    assert!(analyze(code).is_empty());
}

#[test]
fn statement_after_if_reset_conditional_compilation() {
    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset,
        a: input logic,
        x: output logic,
    ) {
        always_ff {
            if_reset { x = 0; } else { x = a; }
            #[ifdef(EXTRA)]
            block {
                if a { x = 1; }
                let _next: logic = a;
            }
        }
    }
    "#;
    assert!(analyze_with_defines(code, &[]).is_empty());
    let errors = analyze_with_defines(code, &["EXTRA"]);
    assert_eq!(errors.len(), 2);
    assert!(
        errors
            .iter()
            .all(|x| matches!(x, AnalyzerError::StatementAfterIfReset { .. }))
    );
}

#[test]
fn statement_after_if_reset_testbench() {
    let code = r#"
    #[test(test_a)]
    module ModuleA {
        let clk: clock = 0;
        let rst: reset = 0;
        var _x: logic;
        always_ff (clk, rst) {
            if_reset { _x = 0; }
            _x = 1;
        }
    }
    "#;
    assert!(analyze(code).is_empty());
}
