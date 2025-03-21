use crate::namespace::Namespace;
use crate::symbol_path::SymbolPath;
use crate::{Analyzer, AnalyzerError, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn analyze(code: &str) -> Vec<AnalyzerError> {
    symbol_table::clear();

    let metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1(&"prj", &"", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &"", &parser.veryl));
    errors.append(&mut analyzer.analyze_pass3(&"prj", &"", &parser.veryl));
    dbg!(&errors);
    errors
}

#[test]
fn clock_check() {
    let code = r#"
    module ModuleA (
        clk: input clock
    ) {
        var a: logic;
        always_ff (clk) {
            a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleB (
        clk_a: input `_a clock<2>,
        clk_b: input `_b clock[2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a[POS]) {
            a = 0;
        }
        always_ff (clk_b[POS]) {
            b = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleC (
        clk_a: input `_a clock<2, 2>,
        clk_b: input `_b clock[2, 2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a[POS][POS]) {
            a = 0;
        }
        always_ff (clk_b[POS][POS]) {
            b = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleD (
        clk: input logic
    ) {
        var a: logic;
        always_ff (clk) {
            a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidClock { .. }));

    let code = r#"
    module ModuleE (
        clk_a: input `_a clock<2>,
        clk_b: input `_b clock[2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a) {
            a = 0;
        }
        always_ff (clk_b[POS]) {
            b = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidClock { .. }));

    let code = r#"
    module ModuleF (
        clk_a: input `_a clock<2>,
        clk_b: input `_b clock[2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a[POS]) {
            a = 0;
        }
        always_ff (clk_b) {
            b = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidClock { .. }));

    let code = r#"
    module ModuleG (
        clk_a: input `_a clock<2, 2>,
        clk_b: input `_b clock[2, 2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a[POS]) {
            a = 0;
        }
        always_ff (clk_b[POS][POS]) {
            b = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidClock { .. }));

    let code = r#"
    module ModuleH (
        clk_a: input `_a clock<2, 2>,
        clk_b: input `_b clock[2, 2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a[POS][POS]) {
            a = 0;
        }
        always_ff (clk_b[POS]) {
            b = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidClock { .. }));

    let code = r#"
    module ModuleA (
        i_clk_a: input `a default clock,
        i_clk_b: input `b default clock,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MultipleDefaultClock { .. }
    ));
}

#[test]
fn reset_check() {
    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset
    ) {
        var a: logic;
        always_ff (clk, rst) {
            if_reset {
                a = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleB (
        clk_a: input `_a clock,
        rst_a: input `_a reset<2>,
        clk_b: input `_b clock,
        rst_b: input `_b reset[2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a, rst_a[POS]) {
            if_reset {
                a = 0;
            }
        }
        always_ff (clk_b, rst_b[POS]) {
            if_reset {
                b = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleC (
        clk_a: input `_a clock,
        rst_a: input `_a reset<2, 2>,
        clk_b: input `_b clock,
        rst_b: input `_b reset[2, 2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a, rst_a[POS][POS]) {
            if_reset {
                a = 0;
            }
        }
        always_ff (clk_b, rst_b[POS][POS]) {
            if_reset {
                b = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleD (
        clk: input clock,
        rst: input logic
    ) {
        var a: logic;
        always_ff (clk, rst) {
            if_reset {
                a = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidReset { .. }));

    let code = r#"
    module ModuleE (
        clk_a: input `_a clock,
        rst_a: input `_a reset<2>,
        clk_b: input `_b clock,
        rst_b: input `_b reset[2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a, rst_a) {
            if_reset {
                a = 0;
            }
        }
        always_ff (clk_b, rst_b[POS]) {
            if_reset {
                b = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidReset { .. }));

    let code = r#"
    module ModuleF (
        clk_a: input `_a clock,
        rst_a: input `_a reset<2>,
        clk_b: input `_b clock,
        rst_b: input `_b reset[2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a, rst_a[POS]) {
            if_reset {
                a = 0;
            }
        }
        always_ff (clk_b, rst_b) {
            if_reset {
                b = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidReset { .. }));

    let code = r#"
    module ModuleG (
        clk_a: input `_a clock,
        rst_a: input `_a reset<2, 2>,
        clk_b: input `_b clock,
        rst_b: input `_b reset[2, 2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a, rst_a[POS]) {
            if_reset {
                a = 0;
            }
        }
        always_ff (clk_b, rst_b[POS][POS]) {
            if_reset {
                b = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidReset { .. }));

    let code = r#"
    module ModuleH (
        clk_a: input `_a clock,
        rst_a: input `_a reset<2, 2>,
        clk_b: input `_b clock,
        rst_b: input `_b reset[2, 2]
    ) {
        const POS: u32 = 0;
        var a: `_a logic;
        var b: `_b logic;
        always_ff (clk_a, rst_a[POS][POS]) {
            if_reset {
                a = 0;
            }
        }
        always_ff (clk_b, rst_b[POS]) {
            if_reset {
                b = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidReset { .. }));
}

#[test]
fn invalid_modifier() {
    let code = r#"
    module ModuleA (
        i_clk_a: input default logic,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidModifier { .. }));

    let code = r#"
    module ModuleA (
        i_clk_a: input default clock<2>
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidModifier { .. }));

    let code = r#"
    module ModuleA (
        i_clk_a: input default clock[2]
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidModifier { .. }));

    let code = r#"
    module ModuleA (
        i_clk_a: input default reset<2>
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidModifier { .. }));

    let code = r#"
    module ModuleA (
        i_clk_a: input default reset[2]
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidModifier { .. }));
}

#[test]
fn clock_connection_check() {
    let code = r#"
    module ModuleA (
        clk: input logic
    ) {
        inst u: ModuleB (
            clk,
        );
    }

    module ModuleB (
        clk: input clock
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));
}

#[test]
fn reset_connection_check() {
    let code = r#"
    module ModuleA (
        clk: input logic
    ) {
        inst u: ModuleB (
            clk,
        );
    }

    module ModuleB (
        clk: input reset
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));
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
        const a: u32 = 1;
        const a: u32 = 1;
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
    module ModuleB (
        b: import logic,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidDirection { .. }));

    let code = r#"
    module ModuleC {
        function FuncC (
            c: import logic,
        ) {}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidDirection { .. }));

    let code = r#"
    module ModuleD {
        function FuncD (
            D: modport logic,
        ) {}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidDirection { .. }));

    let code = r#"
    interface InterfaceF {
        var f: logic;
        modport mp {
            f: modport,
        }
    }
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

    let code = r#"
    proto package Pkg {}
    module ModuleA {
        import Pkg::*;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidImport { .. }));

    let code = r#"
    proto package Pkg {
        enum FOO {
            BAR
        }
    }
    module ModuleA {
        import Pkg::FOO;
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
fn invalid_modport_item() {
    let code = r#"
    interface InterfaceA {
        var a: logic;
        function f -> logic {
            return 1;
        }

        modport mp {
            a: input ,
            f: import,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceB {
        var a: logic;
        function f -> logic {
            return 1;
        }

        modport mp {
            f: input,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidModportVariableItem { .. }
    ));

    let code = r#"
    interface InterfaceC {
        var a: logic;
        function f -> logic {
            return 1;
        }

        modport mp {
            a: import,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidModportFunctionItem { .. }
    ));

    let code = r#"
    package Pkg {
        function f() -> logic {
            return 0;
        }
    }
    interface Interface {
        import Pkg::f;
        modport mp {
            f: import,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidModportFunctionItem { .. }
    ));
}

#[test]
fn invalid_port_default_value() {
    let code = r#"
    module ModuleA (
        a: output logic = 0,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidPortDefaultValue { .. }
    ));

    let code = r#"
    module ModuleA (
        a: inout tri logic = 0,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidPortDefaultValue { .. }
    ));

    let code = r#"
    module ModuleA {
        function FuncA(
            a: input logic = 1,
        ) {
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidPortDefaultValue { .. }
    ));

    let code = r#"
    module ModuleA {
        function FuncA(
            a: output logic = _,
        ) {
            a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidPortDefaultValue { .. }
    ));
}

#[test]
fn mismatch_function_arity() {
    let code = r#"
    module ModuleA {
        function FuncA (
            a: input logic,
        ) -> logic {}

        let _a: logic = FuncA(1, 2);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    interface InterfaceB {
        function FuncB (
            a: input logic,
        ) -> logic {}
    }

    module ModuleB {
        inst instB: InterfaceB();
        let _b: logic = instB.FuncB(1, 2);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    interface InterfaceC {
        function FuncC (
            a: input logic,
        ) -> logic {}

        modport mp {
            FuncC: import,
        }
    }

    module ModuleC (
        ifc: modport InterfaceC::mp,
    ) {
        let _c: logic = ifc.FuncC(1, 2);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));
}

#[test]
fn missing_default_generic_argument() {
    let code = r#"
    module ModuleA {
        function FuncA::<A: const> () -> logic<A> {}
        let _a: logic = FuncA::<1>();

        function FuncB::<A: const, B: const, C: const> () -> logic<A + B + C> {}
        let _b: logic = FuncB::<1, 2, 3>();

        function FuncC::<A: const = 1> () -> logic<A> {}
        let _c: logic = FuncC::<>();

        function FuncD::<A: const = 1, B: const = 2, C: const = 3> () -> logic<A + B + C> {}
        let _d: logic = FuncD::<>();

        function FuncE::<A: const, B: const = 2, C: const = 3> () -> logic<A + B + C> {}
        let _e: logic = FuncE::<1>();

        function FuncF::<A: const, B: const, C: const = 3> () -> logic<A + B + C> {}
        let _f: logic = FuncF::<1, 2>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
        module ModuleB {
            function FuncA::<A: const = 1, B: const, C: const = 3> () -> logic<A + B + C> {}
            let _a: logic = FuncA::<1, 2, 3> ();
        }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingDefaultArgument { .. }
    ));

    let code = r#"
        module ModuleC {
            function FuncA::<A: const = 1, B: const = 2, C: const> () -> logic<A + B + C> {}
            let _a: logic = FuncA::<1, 2, 3>();
        }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingDefaultArgument { .. }
    ));
}

#[test]
fn mismatch_generics_arity() {
    let code = r#"
    module ModuleA {
        function FuncA::<T: const> (
            a: input logic<T>,
        ) -> logic<T> {}

        let _a: logic = FuncA::<1, 2>(1);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchGenericsArity { .. }
    ));

    let code = r#"
    module ModuleB {
        function FuncA::<T: const, U: const> (
            a: input logic<T>,
        ) -> logic<T> {}

        let _a: logic = FuncA::<1>(1);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchGenericsArity { .. }
    ));

    let code = r#"
    package PackageC::<W: const> {
        struct StructC {
            c: logic<W>,
        }
    }
    module ModuleC {
        var c: PackageC::<2>::StructC;
        assign c.c = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PackageD {
        function FuncD::<W: const> -> logic<W> {
            return 0;
        }
    }
    module SubD::<W: const> {
        let _d: logic<W> = PackageD::FuncD::<W>();
    }
    module TopD {
        inst u_subd_1: SubD::<1>();
        inst u_subd_2: SubD::<2>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn mismatch_attribute_args() {
    let code = r#"
    module ModuleA {
        #[sv]
        const a: u32 = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAttributeArgs { .. }
    ));

    let code = r#"
    module ModuleA {
        #[allow(dummy_name)]
        var a: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAttributeArgs { .. }
    ));
}

#[test]
fn incompat_proto() {
    let code = r#"
    proto package ProtoPkg {
        const C: u32;
    }
    package Pkg for ProtoPkg {
        const C: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        const C: u32;
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        const C: u32;
    }
    package Pkg for ProtoPkg {
        const C: i32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        const C: u32;
    }
    package Pkg for ProtoPkg {
        type C = logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        type T;
    }
    package Pkg for ProtoPkg {
        type T = logic;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        type T;
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        type T;
    }
    package Pkg for ProtoPkg {
        const T: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        enum E {
            FOO,
        }
    }
    package Pkg for ProtoPkg {
        enum E {
            FOO,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        enum E {
            FOO
        }
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        enum E {
            FOO,
        }
    }
    package Pkg for ProtoPkg {
        enum E{
            BAR,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        enum E {
            FOO
        }
    }
    package Pkg for ProtoPkg {
        const E: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        struct S {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
        struct S {
            foo: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        struct S {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        struct S {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
        struct S {
            bar: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        struct S {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
        const S: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        union U {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
        union U {
            foo: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        union U {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        union U {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
        union U {
            bar: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        union U {
            foo: logic,
        }
    }
    package Pkg for ProtoPkg {
        const U: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        function F(
            a: input logic,
        ) -> logic;
    }
    package Pkg for ProtoPkg {
        function F(
            a: input logic,
        ) -> logic {
            return a;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        function F(
            a: input logic,
        ) -> logic;
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        function F(
            a: input logic,
        ) -> logic;
    }
    package Pkg for ProtoPkg {
        function F(
            a: input logic,
        ){}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkg {
        function F(
            a: input logic,
        ) -> logic;
    }
    package Pkg for ProtoPkg {
        const F: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));
}

#[test]
fn mismatch_type() {
    let code = r#"
    module ModuleA {
        const a: u32 = 1;
        inst u: a;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {}
    module ModuleB {
        inst u: ModuleA;
        let _b: u = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        function FuncA() -> logic {
            return 0;
        }
        let _a: FuncA = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {}
    module ModuleB {
        let _a: ModuleA = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {}
    module ModuleA {
        let _a: InterfaceA = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    package PackageA {}
    module ModuleA {
        let _a: PackageA = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        function FuncA::<T: type> -> T {
            var a: T;
            a = 0;
            return a;
        }

        let _a: logic = FuncA::<2>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        function FuncA::<T: type> -> T {
            var a: T;
            a = 0;
            return a;
        }

        type my_logic = logic;
        let _a: logic = FuncA::<my_logic>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {}

    module ModuleA (
        a: modport InterfaceA,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        function FuncA::<T: type> -> T {
            var a: T;
            a = 0;
            return a;
        }

        const X: u32 = 1;
        let _a: logic = FuncA::<X>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoA;
    proto module ProtoB;

    module ModuleA::<T: ProtoA> {
        inst u: T;
    }

    module ModuleB for ProtoB {}

    module ModuleC {
        inst u: ModuleA::<ModuleB>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {}
    interface InterfaceB {
      inst u: InterfaceA();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {}
    interface InterfaceA {
      inst u: ModuleA();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
    }

    module ModuleA {
        function FuncA::<IF: inst InterfaceA>() -> logic {
            return IF.a;
        }

        inst if_a: InterfaceA;
        let _a: logic = FuncA::<if_a>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InerfaceA {
        var a: logic;
    }

    interface InterfaceB {
        var b: logic;
    }

    module ModuleA {
        function FuncA::<IF: inst InerfaceA>() -> logic {
            return IF.a;
        }

        inst if_b: InterfaceB;
        let _b: logic = FuncA::<if_b>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    module ModuleA (
        port_a: input InterfaceA::mp,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA::<W: const> {
        var a: logic<W>;
        modport mp {
            a: input,
        }
    }
    module ModuleA (
        port_a: input InterfaceA::<2>::mp,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
    }
    module ModuleA (
        port_a: input InterfaceA,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA::<W: const> {
        var a: logic<W>;
    }
    module ModuleA (
        port_a: input InterfaceA::<2>,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
    }
    module ModuleA (
        port_a: modport InterfaceA,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA::<W: const> {
        var a: logic<W>;
    }
    module ModuleA (
        port_a: modport InterfaceA::<2>,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        enum EnumA {
            A,
            B,
        }
        type EnumB = EnumA;

        let _a: EnumA = EnumA::A;
        let _b: EnumB = EnumB::B;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkgA {}
    package PkgA for ProtoPkgA {}
    proto package ProtoPkgB {}
    package PkgB for ProtoPkgB {}
    module ModuleA::<PKG: ProtoPkgA> {}
    module ModuleB {
        inst u: ModuleA::<PkgB>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto package ProtoPkg {}
    package Pkg for ProtoPkg {}
    module ModuleA::<PKG: ProtoPkg> {
        inst u: PKG;
    }
    module ModuleB {
        inst u: ModuleA::<Pkg>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {}
    module ModuleB::<B0: const, B1: const> {}

    alias module Foo = ModuleA;
    alias module Bar = ModuleB::<1, 2>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {}
    alias module ModuleA = InterfaceA;
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ModuleA;
    alias module ModuleB = ModuleA;
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {}
    interface InterfaceB::<B0: const, B1: const> {}

    alias interface Foo = InterfaceA;
    alias interface Bar = InterfaceB::<1, 2>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PackageA {}
    alias interface InterfaceA = PackageA;
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    //let code = r#"
    //proto interface InterfaceA {}
    //alias interface InterfaceB = InterfaceA;
    //"#;
    //
    //let errors = analyze(code);
    //assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    package PkgA {}
    package PkgB::<B0: const, B1: const> {}

    alias package FooPkg = PkgA;
    alias package BarPkg = PkgB::<1, 2>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {}
    alias package Pkg = ModuleA;
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto package PkgA {}
    alias package PkgB = PkgA;
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));
}

#[test]
fn mismatch_assignment() {
    let code = r#"
    module ModuleA {
        let _a: logic[2] = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA {
        var _a: logic[2];
        assign _a = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = 'x;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mp {
            a: input
        }
    }
    interface InterfaceB {
        var a: logic;
        modport mp {
            a: input
        }
    }
    module ModuleA (
        a: modport InterfaceA::mp,
    ) {}
    module ModuleB {
        inst x: InterfaceB;
        inst y: ModuleA (a: x);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));
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

    let code = r#"
    module ModuleA {
        inst u: ModuleB;
    }

    module ModuleB (
        i_a: input  logic = 0,
        o_b: output logic = _,
    ) {
        assign o_b = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn missing_clock_signal() {
    let code = r#"
    module ModuleA (
        clk: input clock
    ){
        always_ff {}
        always_ff (clk) {}
        for i in 0..1 : g {
            always_ff {}
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleB (
        clk: input logic
    ){
        always_ff {}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingClockSignal { .. }
    ));

    let code = r#"
    module ModuleC (
        clk_0: input `_0 clock,
        clk_1: input `_1 clock
    ){
        always_ff {}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingClockSignal { .. }
    ));

    let code = r#"
    module ModuleD (
        clk: input clock<2>
    ){
        always_ff {}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingClockSignal { .. }
    ));

    let code = r#"
    module ModuleE (){
        for i in 0..1 : g {
            let _clk: clock = 0;
            always_ff {}
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingClockSignal { .. }
    ));
}

#[test]
fn missing_reset_signal() {
    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset
    ) {
        always_ff {
            if_reset {}
        }
        always_ff (clk) {
            if_reset {}
        }
        always_ff (clk, rst) {
            if_reset {}
        }
        for i in 0..1 : g {
            always_ff {
                if_reset {}
            }
            always_ff (clk) {
                if_reset {}
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleB (
        clk: input clock,
        rst: input logic
    ) {
        always_ff {
            if_reset {}
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingResetSignal { .. }
    ));

    let code = r#"
    module ModuleC (
        clk: input clock,
        rst: input logic
    ) {
        always_ff (clk) {
            if_reset {}
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingResetSignal { .. }
    ));

    let code = r#"
    module ModuleD (
        clk:   input clock,
        rst_0: input reset,
        rst_1: input reset
    ) {
        always_ff {
            if_reset {}
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingResetSignal { .. }
    ));

    let code = r#"
    module ModuleE (
        clk: input clock,
        rst: input reset<2>
    ) {
        always_ff {
            if_reset {}
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingResetSignal { .. }
    ));

    let code = r#"
    module ModuleF (
        clk: input clock
    ) {
        for i in 0..1 : g {
            let _rst: reset = 0;
            always_ff {
                if_reset {}
            }
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
        clk: input clock,
        rst: input reset,
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
fn missing_clock_domain() {
    let code = r#"
    module ModuleA (
        clk0: input clock,
        clk1: input clock,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingClockDomain { .. }
    ));
}

#[test]
fn invalid_clock_domain() {
    let code = r#"
    module ModuleA {}
    module ModuleB {
        inst u: `a ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidClockDomain { .. }
    ));
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

    let code = r#"
    module ModuleB {
        enum EnumB: logic<2> {
            A = 3,
            B,
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
        const a: u32 = 2'd100;
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

    let code = r#"
    module ModuleB {
        enum EnumB: logic {
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
fn unevaluatable_enum_variant() {
    let code = r#"
    module ModuleA {
        enum EnumA: logic<2> {
            A = 2'b0x,
            B = 2'b10,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleB {
        enum EnumA: logic<2> {
            A = 2'b0x,
            B,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnevaluatableEnumVariant { .. }
    ));

    let code = r#"
    module ModuleC {
        #[enum_encoding(onehot)]
        enum EnumA: logic<2> {
            A = 2'bx1,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnevaluatableEnumVariant { .. }
    ));

    let code = r#"
    module ModuleD {
        #[enum_encoding(gray)]
        enum EnumA: logic<2> {
            A = 2'bx0,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnevaluatableEnumVariant { .. }
    ));
}

#[test]
fn invalid_enum_variant() {
    let code = r#"
    module ModuleA {
        #[enum_encoding(onehot)]
        enum EnumA{
            A = 0,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidEnumVariant { .. }
    ));

    let code = r#"
    module ModuleB {
        #[enum_encoding(onehot)]
        enum EnumA{
            A = 3,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidEnumVariant { .. }
    ));

    let code = r#"
    module ModuleC {
        #[enum_encoding(gray)]
        enum EnumA{
            A,
            B = 3,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidEnumVariant { .. }
    ));
}

#[test]
fn invisible_identifier() {
    let code = r#"
    package Pkg::<A: const> {}
    module ModuleA {
        const A: u32 = Pkg::<1>::A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvisibleIndentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        const A: logic = 0;
    }
    module MoudleB {
        const B: logic = ModuleA::A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvisibleIndentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        const A: logic = 0;
    }
    module MoudleB {
        inst u: ModuleA;
        let _b: logic = u.A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvisibleIndentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        enum E {
            FOO,
        }

        let a: E = E::FOO;
        let b: E = a::FOO;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvisibleIndentifier { .. }
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

    let code = r#"
    module ModuleA {
        enum EnumA {
            X,
        }

        // Mangled enum member can't be used directly
        let _a: logic = EnumA_X;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));

    let code = r#"
    package PkgBase::<AW: const, DW: const> {
        type address_t = logic<AW>;
        type data_t    = logic<DW>;
    }
    alias package Pkg = PkgBase::<16, 32>;
    module ModuleA {
        import Pkg::*;
        let _addr:  address_t = 0;
        let _data:  data_t    = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        type address_t;
        type data_t   ;
    }
    package PkgBase::<AW: const, DW: const> for ProtoPkg {
        type address_t = logic<AW>;
        type data_t    = logic<DW>;
    }
    module ModuleA::<pkg: ProtoPkg> {
        import pkg::*;
        let _addr: address_t = 0;
        let _data: data_t    = 0;
    }
    module ModuleB {
        alias package Pkg = PkgBase::<16, 32>;
        inst u: ModuleA::<Pkg>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn referring_package_before_definition() {
    let code = r#"
    module ModuleA {
        const A: u32 = PakcageB::B;
    }
    package PakcageB {
        const B: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringPackageBeforeDefinition { .. }
    ));

    let code = r#"
    interface InterfaceA {
        const A: u32 = PakcageB::B;
    }
    package PakcageB {
        const B: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringPackageBeforeDefinition { .. }
    ));

    let code = r#"
    package PackageA {
        const A: u32 = PakcageB::B;
    }
    package PakcageB {
        const B: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringPackageBeforeDefinition { .. }
    ));

    let code = r#"
    import PakcageB::B;
    package PakcageB {
        const B: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringPackageBeforeDefinition { .. }
    ));
}

#[test]
fn unknown_attribute() {
    let code = r#"
    module ModuleA {
        #[dummy_name]
        const a: u32 = 1;
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

    let code = r#"
    module ModuleA (
        a_if: interface
    ) {
        assign a_if.a = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        a_if: interface::mp
    ) {
        assign a_if.a = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {
        var a: logic;
        var b: logic;

        modport mp_a {
            a: output,
        }

        modport mp_b {
            b: output,
        }
    }

    module ModuleA (
        a_if: modport InterfaceA::mp_a,
    ) {
        assign a_if.b = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));

    let code = r#"
    module ModuleA (
        a: interface      ,
        b: interface::port,
    ) {
        assign a.a = 0;
        assign b.b = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: logic = 0;
        let _b: logic = _a._a;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));

    let code = r#"
    interface InterfaceA::<W: const> {
        var a: logic<W>;
    }
    module ModuleA {
        inst u: InterfaceA::<1>;
        let _a: logic = u.a;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA::<W: const> {
        var a: logic<W>;
        modport mp {
            a: input,
        }
    }
    alias interface InterfaceB = InterfaceA::<1>;
    module ModuleA {
        inst u: InterfaceB;
        let _a: logic = u.a;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA::<W: const> {
        var a: logic<W>;
        modport mp {
            a: input,
        }
    }
    alias interface InterfaceB = InterfaceA::<1>;
    module ModuleA (
        b_if: modport InterfaceB::mp
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        struct StructA {
            a: logic,
        }
        let _a: StructA = StructA'{
            x: 0,
        };
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

    let code = r#"
    interface InterfaceA {
        var a: logic<2>;
        modport mp {
            a: input
        }
    }
    module ModuleA (
        if_a: modport InterfaceA::mp
    ) {
        let a: logic = if_a.a[msb];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMsb { .. }));

    let code = r#"
    interface InterfaceA #(
        param W: u32 = 2
    ){
        var a: logic<W>;
    }
    module ModuleA {
        inst if_a: InterfaceA;
        assign if_a.a = 0;
        let a: logic = if_a.a[msb];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMsb { .. }));

    let code = r#"
    package PackageA::<W: const> {
        struct StructA {
            a: logic<W>,
        }
    }
    package PackageB {
        const B: u32 = 2;
    }
    package PackageC {
        const C: bit<2> = 0;
    }
    module ModuleA {
        var a: PackageA::<PackageB::B>::StructA;
        assign a.a = 0;
        let _b: logic = a.a[msb];
        let _c: logic = PackageC::C[msb];
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleB {
        always_comb {
            let a: logic = 1;
        }
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

    let code = r#"
    interface InterfaceB {
        function FuncB () -> logic {
            return 1;
        }
    }
    module ModuleB {
        inst ifb: InterfaceB ();
        initial {
            ifb.FuncB();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnusedReturn { .. }));

    let code = r#"
    interface InterfaceC {
        modport mp {
            FuncC: import,
        }
        function FuncC() -> logic {
            return 1;
        }
    }
    module ModuleC (
        ifc: modport InterfaceC::mp,
    ){
        initial {
            ifc.FuncC();
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

    let code = r#"
    module ModuleA {
        var a: logic;
        var b: logic;
        always_comb {
            b = a;
            a = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module ModuleA {
        var a: logic;
        always_comb {
            a = a;
            a = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module ModuleA {
        var a: logic;
        var b: logic;
        always_comb {
            if 1 {
                let c: logic = 1;
                a = c;
            } else {
                a = 0;
            }
            if 1 {
                var c: logic;
                b = c;
            } else {
                b = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module ModuleA {
        var a: logic;
        always_comb {
            let b: logic = 1;
            a = b;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic;
        always_comb {
            for i: u32 in 0..1 {
                a = i;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<2>;
        always_comb {
            a[0] = 0;
        }

        always_comb {
            a[1] = a[0];
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a:  logic;
        var b:  logic;

        always_comb {
            a = b;
        }

        assign b = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        o_d:    output logic
    ) {
        assign  o_d = '0;
    }
    module ModuleB {
        var a: logic;
        var b: logic;

        always_comb {
            a = b;
        }

        inst u_sub: ModuleA (
            o_d: b
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {
        function FuncA(
            a: output logic,
        ) {
            a = 0;
        }
        modport mp {
            FuncA: import,
        }
    }
    module ModuleA (
        if_a: modport InterfaceA::mp
    ){
        function FuncB(
            a: output logic,
        ) -> logic {
            a = 0;
            return 0;
        }

        var _a: logic;
        var _b: logic;
        var _c: logic;

        always_comb {
            if_a.FuncA(_a);
        }

        always_comb {
            _b = FuncB(_c);
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<A: const>(
        o_a: output logic,
    ) {
        assign o_a = 0;
    }

    module ModuleB {
        var a: logic;
        inst u: ModuleA::<0> (o_a: a);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto module ProtoA (
        i_a: input logic,
    );

    module ModuleB::<A: ProtoA> {
        var a: logic;
        inst u: A (i_a: a);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    proto module ProtoA (
        o_a: output logic,
    );

    module ModuleB::<A: ProtoA> {
        var a: logic;
        inst u: A (o_a: a);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn unassignable_output() {
    let code = r#"
    module ModuleA {
        inst u: ModuleB (
            x: 1,
        );
    }

    module ModuleB (
        x: output logic,
    ) {
        assign x = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnassignableOutput { .. }
    ));

    let code = r#"
    module ModuleA {
        var y: logic;
        inst u: ModuleB (
            x: y + 1,
        );
    }

    module ModuleB (
        x: output logic,
    ) {
        assign x = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnassignableOutput { .. }
    ));

    let code = r#"
    module ModuleA {
        var y: logic;
        inst u: ModuleB (
            x: {y repeat 2},
        );
    }

    module ModuleB (
        x: output logic,
    ) {
        assign x = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnassignableOutput { .. }
    ));

    let code = r#"
    module ModuleA {
        var y: logic;
        var z: logic;
        inst u: ModuleB (
            x: {y, z},
        );
    }

    module ModuleB (
        x: output logic,
    ) {
        assign x = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn uncovered_branch() {
    let code = r#"
    module ModuleA {
        var a: logic;
        var b: logic;
        let x: logic = 1;

      always_comb {
        if x {
            let y: logic = 1;
            a = y;
        } else {
            a = 0;
        }
      }

      always_comb {
        var z: logic;
        if x {
            z = 1;
            b = z;
        } else {
            b = 0;
        }
      }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleB {
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
fn anonymous_identifier() {
    let code = r#"
    module ModuleA (
        i_clk: input `_ clock,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module _ {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        let a: logic = _ + 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module SubA (
        i_a: input logic
    ) {
    }
    module ModuleA {
        inst u_sub: SubA (
            i_a: _
        );
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module SubA (
        o_a: output logic
    ) {
        assign o_a = 0;
    }
    module ModuleA {
        inst u_sub: SubA (
            o_a: _
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        inst u_sub: $sv::Sub (
            i_a: _,
            o_b: _,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        i_a: input logic = _,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA (
        o_a: output logic = _,
    ) {
        assign o_a = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

#[test]
fn reset_value_non_elaborative() {
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic;
        var b: logic;

        always_ff {
            if_reset {
                a = b;
            } else {
                a = 1'b0;
            }
        }
    }"#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidResetNonElaborative { .. }
    ));
}

#[test]
fn invalid_factor_kind() {
    let code = r#"
    module ModuleA {
        function f (
            a: input logic,
        ) -> logic {
            return a;
        }

        var a: logic;

        assign a = f + 1;
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        let a: logic = $clog2;
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    //TODO this case should be detected as type mismatch
    //let code = r#"
    //interface InterfaceA {
    //    var a: logic;
    //    modport master {
    //        a: input,
    //    }
    //}
    //module ModuleA (
    //    b: modport InterfaceA::master,
    //) {
    //    var a: logic;
    //    always_comb {
    //        a = b;
    //    }
    //}
    //"#;
    //
    //let errors = analyze(code);
    //assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA #(
        param A: bit = 0
    )(
        a: input logic = A,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        let a: logic = ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA (
        a: input logic,
        b: input logic = a,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    package PackageA {
        const A: bit = 0;
    }
    module ModuleA (
        a: input  logic = PackageA::A,
    ){}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn call_non_function() {
    let code = r#"
    module ModuleA {
        function f (
            a: input logic,
        ) -> logic {
            return a;
        }

        var a: logic;
        var b: logic;

        assign a = b() + 1;
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::CallNonFunction { .. }));
}

#[test]
fn invalid_assignment_to_const() {
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        always_comb {
            let y: logic = 1;
            y = 0;
        }
    }"#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidAssignmentToConst { .. }
    ));
}

// TODO disable until adding expression type check
//#[test]
//fn test_factors() {
//    let code = r#"
//    interface InterfaceA {
//        var a: logic;
//        modport master {
//            a: input,
//        }
//    }
//
//    module ModuleA #(
//        param K: i32 = 32,
//    ) (
//        i_clk: input   clock             ,
//        i_rst: input   reset             ,
//        mst  : modport InterfaceA::master,
//    ) {
//
//        enum State: logic<3> {
//            Idle = 3'bxx1,
//            Run0 = 3'b000,
//            Run1 = 3'b010,
//            Run2 = 3'b100,
//            Done = 3'b110,
//        }
//
//        struct S {
//            v: logic,
//        }
//
//        union U {
//            v: logic,
//            w: logic,
//        }
//
//        let state: State = State::Run1;
//        var u    : U    ;
//        var s    : S    ;
//        const J    : i32   = 32;
//
//        for i in 0..1 :g_display {
//            always_ff {
//                $display("%d", i);
//            }
//        }
//
//        function foo () -> logic {
//            return 1'b1;
//        }
//
//        function bar (
//            l: input logic,
//        ) -> logic {
//            return foo();
//        }
//
//        assign u.v = 1'b1;
//        assign s.v = 1'b1;
//        initial {
//            $display("%d", u);
//            $display("%d", s);
//            $display("%d", state);
//            $display("%d", mst.a);
//            $display("%d", i_clk);
//            $display("%d", K);
//            $display("%d", J);
//            $display("%d", foo());
//            // Using $bits as a placeholder SystemFunciton.
//            $display("%d", $bits(S));
//            $display("%d", $bits(U));
//            $display("%d", $bits(foo(), State));
//            $display("%d", bar($bits(State)));
//            $display("%d", $bits(State));
//            // The following 4 cases should be error.
//            $display("%d", bar(S));
//            $display("%d", bar(U));
//            $display("%d", bar(State));
//            $display("%d", $bits(bar(State)));
//        }
//    }"#;
//
//    let errors = analyze(code);
//    assert!(errors.len() == 4);
//    for error in errors {
//        assert!(matches!(error, AnalyzerError::InvalidFactor { .. }));
//    }
//}

#[test]
fn enum_non_const_exception() {
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {

        enum State: logic<3> {
            Idle = 3'bxx1,
            Run0 = 3'b000,
            Run1 = 3'b010,
            Run2 = 3'b100,
            Done = 3'b110,
        }
        var state: State;

        always_ff {
            if_reset {
                state = State::Idle;
            }
        }

    }"#;
    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn invalid_case_condition_expression() {
    let code = r#"
    module ModuleA (
        i_sel: input  logic<3>,
        i_a  : input  logic<4>,
        o_b  : output logic,
        o_c  : output logic,
    ) {
        const ONE: bit <3> = 3'd1;

        always_comb {
          case i_sel {
            3'd0   : o_b = i_a[0];
            ONE    : o_b = i_a[1];
            2..=3  : o_b = i_a[2];
            3'b1xx : o_b = i_a[3];
            default: o_b = i_a[3];
          }
        }

        assign o_c = case i_sel {
            3'd0   : i_a[0],
            ONE    : i_a[1],
            2..=3  : i_a[2],
            3'b1xx : i_a[3],
            default: i_a[3],
        };
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleB (
        i_sel: input  logic<2>,
        i_a  : input  logic<3>,
        o_b  : output logic,
    ) {
        let c: logic<2> = 2'd0;

        always_comb {
          case i_sel {
            c      : o_b = i_a[0];
            default: o_b = i_a[1];
          }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidCaseConditionNonElaborative { .. }
    ));

    let code = r#"
    module ModuleC (
        i_sel: input  logic<2>,
        i_a  : input  logic<3>,
        o_b  : output logic,
    ) {
        let c: logic<2> = 2'd1;

        always_comb {
          case i_sel {
            0..=c  : o_b = i_a[0];
            default: o_b = i_a[1];
          }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidCaseConditionNonElaborative { .. }
    ));

    let code = r#"
    module ModuleD (
        i_sel: input  logic<2>,
        i_a  : input  logic<3>,
        o_b  : output logic,
    ) {
        let c: logic<2> = 2'd0;

        assign o_b = case i_sel {
            c      : i_a[0],
            default: i_a[1],
        };
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidCaseConditionNonElaborative { .. }
    ));

    let code = r#"
    module ModuleE (
        i_sel: input  logic<2>,
        i_a  : input  logic<3>,
        o_b  : output logic,
    ) {
        let c: logic<2> = 2'd1;

        assign o_b = case i_sel {
            0..=c  : i_a[0],
            default: i_a[1],
        };
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidCaseConditionNonElaborative { .. }
    ));
}

#[test]
fn invalid_cast() {
    let code = r#"
    module ModuleA {
        let a: clock = 1;
        let _b: reset = a as reset;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidCast { .. }));
}

#[test]
fn invalid_test() {
    let code = r#"
    module ModuleA {}

    #[test(TestA)]
    embed (cocotb) py {{{
    }}}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidTest { .. }));
}

#[test]
fn invalid_select() {
    let code = r#"
    module ModuleA {
        let _a: logic<2> = 1;
        let _b: logic<2> = _a[2];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));

    let code = r#"
    module ModuleA {
        let _a: logic[2] = '{1, 1};
        let _b: logic<2> = _a[2];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));

    let code = r#"
    module ModuleA {
        let _a: logic<2> = 1;
        let _b: logic<2> = _a[0:1];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));

    let code = r#"
    module ModuleA {
        let _a: logic[2] = '{1, 1};
        let _b: logic[2] = _a[1:0];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));
}

#[test]
fn clock_domain() {
    let code = r#"
    module ModuleA (
        i_clk: input  `a clock,
        i_dat: input  `a logic,
        o_dat: output `b logic,
    ) {
        assign o_dat = i_dat;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleB (
        i_clk : input  `a clock,
        i_dat0: input  `a logic,
        i_dat1: input  `b logic,
        o_dat : output `a logic,
    ) {
        assign o_dat = {i_dat0, i_dat1};
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleC (
        i_clk : input  `a clock,
        i_dat0: input  `a logic,
        i_dat1: input  `b logic,
        o_dat : output `a logic,
    ) {
        inst u: ModuleD (
            i_dat: i_dat1,
            o_dat,
        );
    }

    module ModuleD (
        i_dat: input  logic,
        o_dat: output logic,
    ) {
        assign o_dat = i_dat;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleE (
        i_clk : input  `a clock,
        i_dat0: input  `a logic,
        i_dat1: input  `b logic,
        o_dat : output `a logic,
    ) {
        inst u: $sv::Module (
            i_dat: i_dat1,
            o_dat,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleF (
        i_clk : input  `a clock,
        i_dat0: input  `a logic,
        i_dat1: input  `b logic,
        o_dat : output `b logic,
    ) {
        var r_dat: `b logic;

        always_ff {
            r_dat = i_dat1;
        }

        assign o_dat = r_dat;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleG (
        i_clk: input   `a clock,
        i_dat: input   `a logic,
        o_dat: modport `b InterfaceA::port,
    ) {
        assign o_dat.a = i_dat;
    }

    interface InterfaceA {
        var a: logic;

        modport port {
            a: output,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleH (
        i_clk: input  `a clock,
        i_dat: input  `a logic,
        o_dat: output `b logic,
    ) {
        always_comb {
            o_dat = i_dat;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    interface InterfaceI {
      var v: logic;
    }
    module ModuleI (
      i_clk: input `a clock,
      i_dat: input `a logic,
    ) {
        inst intf: `a InterfaceI;
        assign intf.v = i_dat;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceJ {
      var v: logic;
    }
    module ModuleJ (
      i_clk: input `a clock,
      i_dat: input `a logic,
    ) {
        inst intf: `b InterfaceJ;
        assign intf.v = i_dat;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    interface InterfaceK {
      var v: logic;
    }
    module ModuleK (
      i_clk: input  `a clock,
      o_dat: output `b logic,
    ) {
        inst intf: `a InterfaceK;

        assign intf.v = '0;
        assign o_dat  = intf.v;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleL (
        i_clk_a: input `a clock,
        i_rst_a: input `a reset,
        i_clk_b: input `b clock,
        i_rst_b: input `b reset,
    ) {
        var _a: `a logic;
        always_ff (i_clk_a, i_rst_b) {
            if_reset {
                _a = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleM (
        i_clk_a: input `a clock,
        i_clk_b: input `b clock,
        i_rst_b: input `b reset,
    ) {
        var _a: `a logic;
        always_ff (i_clk_a) {
            if_reset {
                _a = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleO (
        i_clk_a: input `a clock,
        i_rst_b: input `b reset,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleO (
        i_clk_a: input `a default clock,
        i_rst_a: input `a         reset,
        i_clk_b: input `b         clock,
        i_rst_b: input `b default reset,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));
}

#[test]
fn r#unsafe() {
    let code = r#"
    module ModuleA {
        unsafe(x) {
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownUnsafe { .. }));
}

#[test]
fn detect_recursive() {
    let code = r#"
    module ModuleA (
        x: input x,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));
}

#[test]
fn sv_keyword_usage() {
    let code = r#"
    module ModuleA {
        var always: logic;
        assign always = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::SvKeywordUsage { .. }));

    let code = r#"
    module ModuleA {
        var r#always: logic;
        assign r#always = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::SvKeywordUsage { .. }));
}

#[test]
fn sv_with_implicit_reset() {
    let code = r#"
    module ModuleA {
        var rst: reset;

        inst u: $sv::Module (
            rst,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::SvWithImplicitReset { .. }
    ));

    let code = r#"
    module ModuleB {
        var rst: reset_async_low;

        inst u: $sv::Module (
            rst,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var rst: reset;

        inst u: $sv::Module (
            rst: rst as reset_sync_high,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn conflict_with_mangled_enum_member() {
    let code = r#"
    module ModuleA {
        enum EnumA: logic {
            MemberA,
        }
        var EnumA_MemberA: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));
}

#[test]
fn unresolvable_generic_argument() {
    let code = r#"
    module ModuleA {
        const X: u32 = 1;
        const Y: u32 = PackageA::<X>::W;
    }

    package PackageA::<T: const> {
        const W: u32 = T;
    }
    "#;

    let errors = analyze(code);
    // This pattern also causes CyclicTypeDependency error
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnresolvableGenericArgument { .. }))
    );
}

#[test]
fn wrong_seperator() {
    let code = r#"
    package A {
        enum B {
            C,
        }
    }
    module Module {
        var _a: A::B;

        always_comb {
            _a = A.B.C;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::WrongSeparator { .. }));

    let code = r#"
    package A {
        enum B {
            C,
        }
    }
    module Module {
        var _a: A::B;

        always_comb {
            _a = A::B.C;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::WrongSeparator { .. }));

    let code = r#"
    module Module {
        struct B {
            b: logic,
        }

        var _a: B;
        always_comb {
            _a::b = '0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::WrongSeparator { .. }));

    let code = r#"
    interface B {
        var c: logic;
        modport mp {
            c: input,
        }
    }
    module Module (
        b: modport B::mp
    ) {
        var _a: logic;
        always_comb {
            _a = b::c;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::WrongSeparator { .. }));

    let code = r#"
    interface A {
        var b: logic;
    }
    module Module {
        inst a: A;
        always_comb {
            a::b = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::WrongSeparator { .. }));
}

#[test]
fn evaluator() {
    let code = r#"
    module ModuleA {
        const A: u32 = 0;
        const B: u32 = 1 + 2 - 3 + 4 * 3 / 2;
        const C: u32 = 2 ** 2 + 5 % 2 + (1 << 3) + (16 >> 3);
        const D: u32 = (1 >: 0) + (1 >= 1) + (3 <: 5) + (10 <= 10) + (3 == 3) + (5 != 2);
        const E: u32 = (1 && 1) + (1 || 0) + (1 & 1) + (1 | 0) + (1 ^ 0) + ~(1 ^~ 0);
        const F: u32 = &4'hf + |4'h1 + ~&4'h1 + ~|4'h0 + ^4'h8 + ~^4'h6;
        const G: u32 = A + B + C + D + E + F;
        const H: u32 = {1'b1, 2'h1, 2'd2 repeat 3};
        const I: u32 = if B == 6 { 10 } else { 20 };
        const J: u32 = $clog2(12);
        const K: u32 = B[2:1];
    }
    "#;

    let _ = analyze(code);

    let namespace: Namespace = "prj::ModuleA".into();

    let a = symbol_table::resolve((&Into::<SymbolPath>::into("A"), &namespace)).unwrap();
    let b = symbol_table::resolve((&Into::<SymbolPath>::into("B"), &namespace)).unwrap();
    let c = symbol_table::resolve((&Into::<SymbolPath>::into("C"), &namespace)).unwrap();
    let d = symbol_table::resolve((&Into::<SymbolPath>::into("D"), &namespace)).unwrap();
    let e = symbol_table::resolve((&Into::<SymbolPath>::into("E"), &namespace)).unwrap();
    let f = symbol_table::resolve((&Into::<SymbolPath>::into("F"), &namespace)).unwrap();
    let g = symbol_table::resolve((&Into::<SymbolPath>::into("G"), &namespace)).unwrap();
    let h = symbol_table::resolve((&Into::<SymbolPath>::into("H"), &namespace)).unwrap();
    let i = symbol_table::resolve((&Into::<SymbolPath>::into("I"), &namespace)).unwrap();
    let j = symbol_table::resolve((&Into::<SymbolPath>::into("J"), &namespace)).unwrap();
    let k = symbol_table::resolve((&Into::<SymbolPath>::into("K"), &namespace)).unwrap();

    let a = a.found.evaluate();
    let b = b.found.evaluate();
    let c = c.found.evaluate();
    let d = d.found.evaluate();
    let e = e.found.evaluate();
    let f = f.found.evaluate();
    let g = g.found.evaluate();
    let h = h.found.evaluate();
    let i = i.found.evaluate();
    let j = j.found.evaluate();
    let k = k.found.evaluate();

    assert_eq!((a.get_value(), a.get_total_width()), (Some(0), Some(32)));
    assert_eq!((b.get_value(), b.get_total_width()), (Some(6), Some(32)));
    assert_eq!((c.get_value(), c.get_total_width()), (Some(15), Some(32)));
    assert_eq!((d.get_value(), d.get_total_width()), (Some(6), Some(32)));
    assert_eq!((e.get_value(), e.get_total_width()), (Some(6), Some(32)));
    assert_eq!((f.get_value(), f.get_total_width()), (Some(6), Some(32)));
    assert_eq!((g.get_value(), g.get_total_width()), (Some(39), Some(32)));
    assert_eq!((h.get_value(), h.get_total_width()), (Some(362), Some(9)));
    assert_eq!((i.get_value(), i.get_total_width()), (Some(10), Some(32)));
    assert_eq!((j.get_value(), j.get_total_width()), (Some(4), Some(32)));
    assert_eq!((k.get_value(), k.get_total_width()), (Some(3), Some(2)));
}

#[test]
fn skip_disabled_generate_block() {
    let code = r#"
    module ModuleA {
        const X: u32 = 1;

        if X == 1 :label {
            let _a: u32 = 1;
        } else {
            // This statement contains MismatchAssignment
            // But it should be ignored because this block is disabled
            let _a: u32 = 'x;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn parameter_override() {
    let code = r#"
    module ModuleA #(
        param X: u32 = 1,
    ) {
        let _a: logic<2> = 1;
        let _b: logic    = _a[X];
    }

    module ModuleB {
        inst u: ModuleA #(X: 3);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));
}

#[test]
fn exceed_limit() {
    let code = r#"
    module ModuleA #(
        param X: u32 = 1,
    ) {
        inst u: ModuleA #(X: X + 1);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::ExceedLimit { .. }));
}

#[test]
fn infinite_recursion() {
    let code = r#"
    module ModuleA {
        inst u: ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InfiniteRecursion { .. }));
}

#[test]
fn define_context() {
    let code = r#"
    module ModuleA {
        #[ifdef(A)]
        var _a: logic;

        #[ifdef(B)]
        var _a: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        #[ifdef(A)]
        let _a: logic = 1;

        #[ifndef(A)]
        let _a: logic = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}
