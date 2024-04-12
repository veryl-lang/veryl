use crate::{Analyzer, AnalyzerError};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn analyze(code: &str) -> Vec<AnalyzerError> {
    let metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1(&"prj", &code, &"", &parser.veryl));
    errors.append(&mut analyzer.analyze_pass2(&"prj", &code, &"", &parser.veryl));
    errors.append(&mut analyzer.analyze_pass3(&"prj", &code, &"", &parser.veryl));
    dbg!(&errors);
    errors
}

#[test]
fn cyclic_type_dependency() {
    let code = r#"
    module ModuleA {
        struct StructA {
            memberA: StructA,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::CyclicTypeDependency { .. }
    ));

    let code = r#"
    module ModuleB {
        inst u: ModuleC;
    }
    module ModuleC {
        inst u: ModuleB;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::CyclicTypeDependency { .. }
    ));

    let code = r#"
    package PackageA {
        import PackageA::*;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::CyclicTypeDependency { .. }
    ));
}

#[test]
fn duplicated_identifier() {
    let code = r#"
    module ModuleA {
        local a: u32 = 1;
        local a: u32 = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));
}

#[test]
fn multiple_assignment() {
    let code = r#"
    module ModuleA {
        var a: logic;

        assign a = 1;
        always_comb {
            a = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MultipleAssignment { .. }
    ));
}

#[test]
fn invalid_allow() {
    let code = r#"
    module ModuleA {
        #[allow(dummy_name)]
        var a: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidAllow { .. }));
}

#[test]
fn invalid_assignment() {
    let code = r#"
    module ModuleA (
        a: input logic,
    ) {
        assign a = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidAssignment { .. }));

    let code = r#"
    module ModuleB (
        a: modport InterfaceA::x,
    ) {
        assign a.a = 1;
    }

    interface InterfaceA {
        var a: logic;

        modport x {
            a: input,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidAssignment { .. }));
}

#[test]
fn invalid_direction() {
    let code = r#"
    module ModuleA (
        a: ref logic,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidDirection { .. }));
}

#[test]
fn invalid_import() {
    let code = r#"
    module ModuleA {
        var a: logic;
        import a::*;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidImport { .. }));
}

#[test]
fn invalid_lsb() {
    let code = r#"
    module ModuleA {
        var a: logic;
        assign a = lsb;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidLsb { .. }));
}

#[test]
fn invalid_msb() {
    let code = r#"
    module ModuleA {
        var a: logic;
        assign a = msb;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMsb { .. }));
}

#[test]
fn invalid_number_character() {
    let code = r#"
    module ModuleA {
        let a: logic = 1'b3;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidNumberCharacter { .. }
    ));
}

#[test]
fn invalid_statement() {
    let code = r#"
    module ModuleA (
        clk: input logic,
        rst: input logic,
    ) {
        always_ff (clk, rst) {
            if_reset {
                if_reset {
                }
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidStatement { .. }));
}

#[test]
fn mismatch_arity() {
    let code = r#"
    module ModuleA {
        function FuncA (
            a: input logic,
        ) -> logic {}

        let _a: logic = FuncA(1, 2);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchArity { .. }));
}

#[test]
fn mismatch_attribute_args() {
    let code = r#"
    module ModuleA {
        #[sv]
        local a: u32 = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAttributeArgs { .. }
    ));
}

#[test]
fn mismatch_type() {
    let code = r#"
    module ModuleA {
        local a: u32 = 1;
        inst u: a;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));
}

#[test]
fn missing_if_reset() {
    let code = r#"
    module ModuleA (
        clk: input logic,
        rst: input logic,
    ) {
        always_ff(clk, rst) {
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MissingIfReset { .. }));
}

#[test]
fn missing_port() {
    let code = r#"
    module ModuleA {
        inst u: ModuleB;
    }

    module ModuleB (
        clk: input logic,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MissingPort { .. }));
}

#[test]
fn missing_reset_signal() {
    let code = r#"
    module ModuleA (
        clk: input logic,
    ) {
        always_ff(clk) {
            if_reset {}
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingResetSignal { .. }
    ));
}

#[test]
fn missing_reset_statement() {
    let code = r#"
    module ModuleA (
        clk: input logic,
        rst: input logic,
    ) {
        var a: logic;

        always_ff(clk, rst) {
            if_reset {
            } else {
                a = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingResetStatement { .. }
    ));
}

#[test]
fn missing_tri() {
    let code = r#"
    module ModuleA (
        x: inout logic,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MissingTri { .. }));
}

#[test]
fn too_large_enum_variant() {
    let code = r#"
    module ModuleA {
        enum EnumA: logic<2> {
            A = 100,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::TooLargeEnumVariant { .. }
    ));
}

#[test]
fn too_large_number() {
    let code = r#"
    module ModuleA {
        local a: u32 = 2'd100;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::TooLargeNumber { .. }));
}

#[test]
fn too_much_enum_variant() {
    let code = r#"
    module ModuleA {
        enum EnumA: logic<2> {
            A,
            B,
            C,
            D,
            E,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::TooMuchEnumVariant { .. }
    ));
}

#[test]
fn undefined_identifier() {
    let code = r#"
    module ModuleA {
        assign a = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));
}

#[test]
fn unknown_attribute() {
    let code = r#"
    module ModuleA {
        #[dummy_name]
        local a: u32 = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownAttribute { .. }));
}

#[test]
fn unknown_embed_lang() {
    let code = r#"
    embed (inline) x{{{
    }}}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownEmbedLang { .. }));
}

#[test]
fn unknown_embed_way() {
    let code = r#"
    embed (x) sv{{{
    }}}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownEmbedWay { .. }));
}

#[test]
fn unknown_member() {
    let code = r#"
    module ModuleA {
        struct StructA {
            memberA: logic,
        }
        var a: StructA;
        assign a.memberB = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));
}

#[test]
fn unknown_msb() {
    let code = r#"
    module ModuleA {
        var a: $sv::SvType;
        let b: logic = a[msb];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMsb { .. }));
}

#[test]
fn uknown_port() {
    let code = r#"
    module ModuleA (
        clk: input logic,
    ) {
        inst u: ModuleB (
            clk
        );
    }

    module ModuleB {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownPort { .. }));
}

#[test]
fn uknown_param() {
    let code = r#"
    module ModuleA {
        inst u: ModuleB #(
            X: 1,
        )();
    }

    module ModuleB {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownParam { .. }));
}

#[test]
fn unused_variable() {
    let code = r#"
    module ModuleA {
        let a: logic = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnusedVariable { .. }));
}

#[test]
fn unused_return() {
    let code = r#"
    module ModuleA {
        function FuncA () -> logic {
            return 1;
        }

        initial {
            FuncA();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnusedReturn { .. }));
}

#[test]
fn break_outside_loop() {
    let code = r#"
    module ModuleA {
        always_comb {
            break;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidStatement { .. }));

    let code = r#"
    module ModuleA {
        always_comb {
            if 1 {
                break;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidStatement { .. }));
}

#[test]
fn unassign_variable() {
    let code = r#"
    module ModuleA {
        var _a: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));
}

#[test]
fn uncovered_branch() {
    let code = r#"
    module ModuleA {
        var a: logic;
        let x: logic = 1;

        always_comb {
            if x {
                a = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UncoveredBranch { .. }));
}

#[test]
fn reserved_identifier() {
    let code = r#"
    module __ModuleA {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReservedIdentifier { .. }
    ));
}
