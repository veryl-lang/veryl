use crate::namespace::Namespace;
use crate::symbol_path::SymbolPath;
use crate::{Analyzer, AnalyzerError, attribute_table, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn analyze(code: &str) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1(&"prj", &"", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &"", &parser.veryl));
    let info = Analyzer::analyze_post_pass2();
    errors.append(&mut analyzer.analyze_pass3(&"prj", &"", &parser.veryl, &info));
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
        clk_a: input '_a clock<2>,
        clk_b: input '_b clock[2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock<2, 2>,
        clk_b: input '_b clock[2, 2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock<2>,
        clk_b: input '_b clock[2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock<2>,
        clk_b: input '_b clock[2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock<2, 2>,
        clk_b: input '_b clock[2, 2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock<2, 2>,
        clk_b: input '_b clock[2, 2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        i_clk_a: input 'a default clock,
        i_clk_b: input 'b default clock,
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
        clk_a: input '_a clock,
        rst_a: input '_a reset<2>,
        clk_b: input '_b clock,
        rst_b: input '_b reset[2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock,
        rst_a: input '_a reset<2, 2>,
        clk_b: input '_b clock,
        rst_b: input '_b reset[2, 2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock,
        rst_a: input '_a reset<2>,
        clk_b: input '_b clock,
        rst_b: input '_b reset[2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock,
        rst_a: input '_a reset<2>,
        clk_b: input '_b clock,
        rst_b: input '_b reset[2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock,
        rst_a: input '_a reset<2, 2>,
        clk_b: input '_b clock,
        rst_b: input '_b reset[2, 2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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
        clk_a: input '_a clock,
        rst_a: input '_a reset<2, 2>,
        clk_b: input '_b clock,
        rst_b: input '_b reset[2, 2]
    ) {
        const POS: u32 = 0;
        var a: '_a logic;
        var b: '_b logic;
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

    let code = r#"
    module ModuleA {
        inst u: ModuleB (
            clk: '0,
            rst: '0,
        );
    }
    module ModuleB (
        clk: input clock,
        rst: input reset,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleA {
        function sum(
            operand: input u32
        ) -> u32 {
            if operand >: 0 {
                return operand + sum(operand - 1);
            } else {
                return 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleA {}
    module ModuleB {
        inst ModuleA: ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        inst ModuleB: ModuleB;
    }
    module ModuleB {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));

    let code = r#"
    interface InterfaceA {}
    module ModuleB {
        inst InterfaceA: InterfaceA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        inst InterfaceB: InterfaceB;
    }
    interface InterfaceB {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA (
        x: input x,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        bind ModuleB <- u: ModuleC;
        bind ModuleB <- u: ModuleC;
    }
    module ModuleB {
    }
    module ModuleC {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::DuplicatedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        bind ModuleB <- u: ModuleC;
    }
    module ModuleB {
        inst u: ModuleC;
    }
    module ModuleC {
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

    let code = r#"
    module ModuleA () {
        var w: logic<2>;
        if 1 :g {
            assign w[0] = 1'b1;
            inst u: ModuleB (
                o: w[1],
            );
        } else {
            assign w = '0;
        }
    }

    module ModuleB (
        o: output logic,
    ) {
        assign o = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    package Pkg {
        function Func::<V: u32>() -> u32 {
            return V;
        }
    }
    module ModuleA {
        import Pkg::Func::<1>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidImport { .. }));

    let code = r#"
    proto package ProtoPkg {
        const C: u32;
    }
    module ModuleA::<PKG: ProtoPkg> {
        import PKG::C;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleA {
        var _foo: logic<2, 3>;
        var _bar: logic<2, 3>;
        for i in 0..2 :g {
            always_comb {
            _foo[i][msb:0] = 0;
            }
        }
        always_comb {
            for i: u32 in 0..2 {
            _bar[i][msb:0] = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleA {
        function FuncA() {
            return 1;
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

    let code = r#"
    proto interface ProtoA {
        function f() -> logic;
        modport mp {
            f: import,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleA {
        function FuncA::<WIDTH: u32> (
            a: input logic,
        ) -> logic {}

        let _a: logic = FuncA::<10>(1, 2);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    module ModuleA {
        function FuncA(a: input u32) {
        }
        always_comb {
            FuncA(0, 1);
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    module ModuleA {
        function FuncA(a: input u32) {
        }
        always_comb {
            FuncA();
        }
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
        function FuncA::<A: u32> () -> logic<A> {}
        let _a: logic = FuncA::<1>();

        function FuncB::<A: u32, B: u32, C: u32> () -> logic<A + B + C> {}
        let _b: logic = FuncB::<1, 2, 3>();

        function FuncC::<A: u32 = 1> () -> logic<A> {}
        let _c: logic = FuncC::<>();

        function FuncD::<A: u32 = 1, B: u32 = 2, C: u32 = 3> () -> logic<A + B + C> {}
        let _d: logic = FuncD::<>();

        function FuncE::<A: u32, B: u32 = 2, C: u32 = 3> () -> logic<A + B + C> {}
        let _e: logic = FuncE::<1>();

        function FuncF::<A: u32, B: u32, C: u32 = 3> () -> logic<A + B + C> {}
        let _f: logic = FuncF::<1, 2>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
        module ModuleB {
            function FuncA::<A: u32 = 1, B: u32, C: u32 = 3> () -> logic<A + B + C> {}
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
            function FuncA::<A: u32 = 1, B: u32 = 2, C: u32> () -> logic<A + B + C> {}
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
fn missing_default_parameter_argument() {
    let code = r#"
    module ModuleA #(
        param A: u32,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingDefaultArgument { .. }
    ));

    let code = r#"
    interface InterfaceA #(
        param A: u32,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MissingDefaultArgument { .. }
    ));

    let code = r#"
    proto module ProtoModuleA #(
        param A: u32,
    );
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface InterfaceA #(
        param A: u32,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn mismatch_generics_arity() {
    let code = r#"
    module ModuleA {
        function FuncA::<T: u32> (
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
        function FuncA::<T: u32, U: u32> (
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
    package PackageC::<W: u32> {
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
        function FuncD::<W: u32> -> logic<W> {
            return 0;
        }
    }
    module SubD::<W: u32> {
        let _d: logic<W> = PackageD::FuncD::<W>();
    }
    module TopD {
        inst u_subd_1: SubD::<1>();
        inst u_subd_2: SubD::<2>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgE {
        function Foo::<V: u32> -> u32 {
            return V;
        }
        struct Bar::<W: u32> {
            bar: logic<W>,
        }
        union Baz::<W: u32> {
            baz: logic<W>,
        }
    }
    module ModuleE {
        import PkgE::Foo;
        let _a: u32 = Foo::<0>();
        let _b: u32 = Foo::<1>();

        import PkgE::Bar;
        var _c: Bar::<1>;
        var _d: Bar::<2>;
        assign _c.bar = 0;
        assign _d.bar = 0;

        import PkgE::Baz;
        var _e: Baz::<1>;
        var _f: Baz::<2>;
        assign _e.baz = 0;
        assign _f.baz = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA::<V: u32> {
        const A: u32 = V;
    }
    module ModuleA {
        import PkgA::<32>::*;
        function get_v::<V: u32> -> u32 {
            return V;
        }
        let _a: u32 = get_v::<A>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if::<W: u32> {
        var a: logic<W>;
        modport mp {
            a: input
        }
    }
    module b_module (
        aif: modport a_if::mp,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchGenericsArity { .. }
    ));
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

    let code = r#"
    module ModuleA {
        #[else(dummy_name)]
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
    proto interface ProtoInterface #(
        param P: u32 = 0,
    ) {}
    interface Interface for ProtoInterface #(
        param P: u32 = 0
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterface #(
        param P: u32 = 0,
    ) {}
    interface Interface for ProtoInterface {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface #(
        param P: u32 = 0,
    ) {}
    interface Interface for ProtoInterface #(
        param P: i32 = 0
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface #(
        param P: u32 = 0,
    ) {}
    interface Interface for ProtoInterface #(
        const P: u32 = 0
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var _v: logic;
    }
    interface Interface for ProtoInterface {
        var _v: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterface {
        var _v: logic;
    }
    interface Interface for ProtoInterface {
        let _v: logic = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterface {
        var _v: logic;
    }
    interface Interface for ProtoInterface {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var _v: logic;
    }
    interface Interface for ProtoInterface {
        var _v: bit;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var _v: logic;
    }
    interface Interface for ProtoInterface {
        let _v: bit = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var _v: logic;
    }
    interface Interface for ProtoInterface {
        type _v = logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        const C: u32;
    }
    interface Interface for ProtoInterface {
        const C: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterface {
        const C: u32;
    }
    interface Interface for ProtoInterface {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        const C: u32;
    }
    interface Interface for ProtoInterface {
        const C: i32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        const C: u32;
    }
    interface Interface for ProtoInterface {
        type C = logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        type T;
    }
    interface Interface for ProtoInterface {
        type T = logic;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterface {
        type T = logic;
    }
    interface Interface for ProtoInterface {
        type T = bit;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        type T;
    }
    interface Interface for ProtoInterface {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        type T;
    }
    interface Interface for ProtoInterface {
        const T: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        function F(
            a: input logic,
        ) -> logic;
    }
    interface Interface for ProtoInterface {
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
    proto interface ProtoInterface {
        function F(
            a: input logic,
        ) -> logic;
    }
    interface Interface for ProtoInterface {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        function F(
            a: input logic,
        ) -> logic;
    }
    interface Interface for ProtoInterface {
        function F(
            a: input logic,
        ){}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        function F(
            a: input logic,
        ) -> logic;
    }
    interface Interface for ProtoInterface {
        const F: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    package PkgA {}
    proto interface ProtoInterface {
        alias package P: ProtoPkgA;
    }
    interface Interface for ProtoInterface {
        alias package P = PkgA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    proto package ProtoPkgB {}
    package PkgB for ProtoPkgB {}
    proto interface ProtoInterface {
        alias package P: ProtoPkgA;
    }
    interface Interface for ProtoInterface {
        alias package P = PkgB;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    proto interface ProtoInterface {
        alias package P: ProtoPkgA;
    }
    interface Interface for ProtoInterface {
        const P: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: input ,
            b: output,
        }
    }
    interface Interface for ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: input ,
            b: output,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: input ,
            b: output,
        }
    }
    interface Interface for ProtoInterface {
        var a: logic;
        var b: logic;
        modport n {
            a: input ,
            b: output,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: input ,
            b: output,
        }
    }
    interface Interface for ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: output,
            b: input ,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: input ,
            b: output,
        }
    }
    interface Interface for ProtoInterface {
        var a: logic;
        var b: logic;
        var c: logic;
        modport m {
            a: output,
            b: input ,
            c: input ,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: input ,
            b: output,
        }
    }
    interface Interface for ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: output,
        }
        modport n {
            b: output,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto interface ProtoInterface {
        var a: logic;
        var b: logic;
        modport m {
            a: input ,
            b: output,
        }
    }
    interface Interface for ProtoInterface {
        var a: logic;
        var b: logic;
        modport n {
            a: input ,
            b: output,
        }
        const m: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

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
        type T = logic;
    }
    package Pkg for ProtoPkg {
        type T = bit;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

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

    let code = r#"
    proto module ProtoModuleA;
    proto package ProtoPkg {
        alias module M: ProtoModuleA;
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto module ProtoModuleA;
    module ModuleA {}
    proto package ProtoPkg {
        alias module M: ProtoModuleA;
    }
    package Pkg for ProtoPkg {
        alias module M = ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto module ProtoModuleA;
    proto module ProtoModuleB;
    module ModuleB for ProtoModuleB {}
    proto package ProtoPkg {
        alias module M: ProtoModuleA;
    }
    package Pkg for ProtoPkg {
        alias module M = ModuleB;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto module ProtoModuleA;
    proto package ProtoPkg {
        alias module M: ProtoModuleA;
    }
    package Pkg for ProtoPkg {
        const M: u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    proto package ProtoPkg {
        alias package P: ProtoPkgA;
    }
    package Pkg for ProtoPkg {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    package PkgA {}
    proto package ProtoPkg {
        alias package P: ProtoPkgA;
    }
    package Pkg for ProtoPkg {
        alias package P = PkgA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    proto package ProtoPkgB {}
    package PkgB for ProtoPkgB {}
    proto package ProtoPkg {
        alias package P: ProtoPkgA;
    }
    package Pkg for ProtoPkg {
        alias package P = PkgB;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::IncompatProto { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    proto package ProtoPkg {
        alias package P: ProtoPkgA;
    }
    package Pkg for ProtoPkg {
        const P: u32 = 0;
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
    module ModuleA {}
    package PkgA {}
    module ModuleB {
        bind PkgA <- u: ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {}
    package PkgA {}
    module ModuleB {
        bind ModuleA <- u: PkgA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoModuleA;
    module ModuleB {}
    bind ProtoModuleA <- u: ModuleB;
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
    module ModuleA {}
    module ModuleB::<T: ModuleA> {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoModuleA;
    module ModuleB::<T: inst ProtoModuleA> {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {}
    module ModuleB::<T: inst ModuleA> {}
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
    interface InterfaceA {}
    module ModuleA::<IF: InterfaceA> {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto interface ProtoInterfaceA {}
    interface InterfaceA {}
    module ModuleA::<IF: ProtoInterfaceA> {
        inst a_if: IF;
    }
    module ModuleB {
        inst u: ModuleA::<InterfaceA>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto interface ProtoInterfaceA {}
    proto interface ProtoInterfaceB {}
    interface InterfaceB for ProtoInterfaceB {}
    module ModuleA::<IF: ProtoInterfaceA> {
        inst a_if: IF;
    }
    module ModuleB {
        inst u: ModuleA::<InterfaceB>;
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
        let _b: logic = FuncA::<if_b>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto interface ProtoInterfaceA {}
    interface InterfaceA for ProtoInterfaceA {}
    module ModuleA {
        function FuncA::<IF: inst ProtoInterfaceA>() {}

        inst if_a: InterfaceA;
        always_comb {
            FuncA::<if_a>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterfaceA {}
    proto interface ProtoInterfaceB {}
    interface InterfaceB for ProtoInterfaceB {}
    module ModuleA {
        function FuncA::<IF: inst ProtoInterfaceA>() {}

        inst if_b: InterfaceB;
        always_comb {
            FuncA::<if_b>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

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
    interface InterfaceA::<W: u32> {
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
    interface InterfaceA::<W: u32> {
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
    interface InterfaceA::<W: u32> {
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
    package PkgA {}
    module ModuleA::<PKG: PkgA> {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto package ProtoPkgA {}
    module ModuleA::<PKG: inst ProtoPkgA> {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    package PkgA {}
    module ModuleA::<PKG: inst PkgA> {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

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
    module ModuleB::<B0: u32, B1: u32> {}

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
    interface InterfaceB::<B0: u32, B1: u32> {}

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
    package PkgB::<B0: u32, B1: u32> {}

    alias package FooPkg = PkgA;
    alias package BarPkg = PkgB::<1, 2>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        enum Foo {
            FOO
        }
    }
    package PkgB::<FOO: PkgA::Foo> {
    }
    alias package PkgC = PkgB::<PkgA::Foo::FOO>;
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

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mp_0 {
            a: input,
        }
        modport mp_1 {
            ..converse(a)
        }
        modport mp_2 {
            ..same(a)
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));
    assert!(matches!(errors[1], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto package ProtoPkg {}
    proto package Pkg {
        alias interface Interface: ProtoPkg;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoModule;
    proto package Pkg {
        alias package Package: ProtoModule;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {}
    package Pkg {
        alias module M = ModuleA;
    }
    module ModuleB {
        inst u: Pkg::M;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        struct Foo {
            foo: logic,
        }
        struct Bar {
            bar: logic,
        }
        const BAR: Bar = Bar'{ bar: 0 };

        function Func::<foo: Foo> {}
        always_comb {
            Func::<BAR>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        union Foo {
            foo: logic,
        }
        union Bar {
            bar: logic,
        }
        const BAR: Bar = Bar'{ bar: 0 };

        function Func::<foo: Foo> {}
        always_comb {
            Func::<BAR>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        enum Foo {
            FOO
        }
        enum Bar {
            BAR
        }

        function Func::<foo: Foo> {}
        always_comb {
            Func::<Bar::BAR>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        enum Foo {
            FOO
        }
        enum Bar {
            BAR
        }
        const BAR: Bar = Bar::BAR;

        function Func::<foo: Foo> {}
        always_comb {
            Func::<BAR>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoModuleA;
    module ModuleB::<M: ProtoModuleA> {
        inst u: M;
    }
    module ModuleC {
        inst u: ModuleB::<1>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA::<W: u32> {
        let _a: logic<W> = 0;
    }
    module ModuleB {}
    module ModuleC {
        inst u: ModuleA::<ModuleB>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto package ProtoPkg {
        const W: u32;
        type  T;
    }
    module ModuleA::<PKG: ProtoPkg, WIDTH: u32, TYPE: type> {
        function FuncA::<W: u32>() -> logic<W> {
            return 0;
        }
        function FuncB::<T: type>() -> T {
            return 0 as T;
        }
        let _a_0: u32 = FuncA::<PKG::W>();
        let _a_1: u32 = FuncA::<WIDTH>();
        let _b_0: u32 = FuncB::<PKG::T>();
        let _b_1: u32 = FuncB::<TYPE>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        function FuncA::<T: type>() -> T {
            return 0 as T;
        }

        function FuncB() -> u32 {
            return FuncA::<u32>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
    const A: u32;
    }

    module ModuleA::<PKG: ProtoPkg> {
    import PKG::*;
    let _a: u32 = 0 as A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package Pkg for ProtoPkg {
    const V: u32 = 0;
    }
    proto package ProtoPkg {
    const V: u32;
    }
    module Sub::<PKG: ProtoPkg> {
    }
    module Top {
    inst u_sub: Sub::<Pkg>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA::<V: u32> {
        struct Foo {
            foo: u32,
        }
        const FOO: Foo = Foo'{ foo: V };
    }
    package PkgB::<V: u32> {
        const BAR: u32 = V;
    }
    module ModuleA {
        import PkgB::<PkgA::<32>::FOO.foo>::*;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package FooPkg::<A: bool = true, B: bool = false> {
    }
    module BarModule {
        import FooPkg::<false, true>::*;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto module ProtoModuleA (
        i_d: input  logic,
        o_d: output logic,
    );
    module ModuleA for ProtoModuleA (
        i_d: input  logic,
        o_d: output logic,
    ) {
        assign o_d = i_d;
    }
    proto package ProtoPackageA {
        alias module A: ProtoModuleA;
    }
    package PackageA {
        alias module A = ModuleA;
    }
    module ModuleB::<PKG_A: ProtoPackageA> (
        i_d: input  logic,
        o_d: output logic,
    ) {
        inst u: PKG_A::A (
            i_d: i_d,
            o_d: o_d,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn invalid_type_declaration() {
    let code = r#"
    interface InterfaceA {
        enum Foo {
            FOO
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidTypeDeclaration { .. }
    ));

    let code = r#"
    interface InterfaceA {
        struct Foo {
            foo: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidTypeDeclaration { .. }
    ));

    let code = r#"
    interface InterfaceA {
        union Foo {
            foo: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidTypeDeclaration { .. }
    ));
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
        inst b: InterfaceB;
    }
    bind ModuleB <- u: ModuleA (
        a: b,
    );
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA {
        const a: u64 = 64'hfff8000000000000;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
        clk_0: input '_0 clock,
        clk_1: input '_1 clock
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
        inst u: 'a ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidClockDomain { .. }
    ));

    let code = r#"
    module ModuleA {
        inst u: 'a $sv::InterfaceA;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    package Pkg {
        enum Foo {
            FOO_0 = 2'b01,
            FOO_1 = 2'b10,
        }

        #[enum_encoding(onehot)]
        enum Bar {
            BAR_0 = Foo::FOO_0,
            BAR_1 = Foo::FOO_1,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
    package Pkg::<A: u32> {}
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

    let code = r#"
    proto interface ProtoA {
        var foo: logic;
        modport mp {
            foo: output,
        }
    }
    module ModuleA::<IF: ProtoA> (
        foo_if: modport IF::mp,
    ) {
        connect foo_if <> 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        struct Foo {
            foo: logic,
        }
        function Func::<foo: Foo>() -> logic {
            return foo.foo;
        }
        const FOO: Foo = Foo'{ foo: 0 };
        let _foo: logic = Func::<FOO>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
    package PkgBase::<AW: u32, DW: u32> {
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
    package PkgBase::<AW: u32, DW: u32> for ProtoPkg {
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

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mp_0 {
            a: input,
        }
        modport mp_1 {
            ..converse(mp)
        }
        modport mp_2 {
            ..same(mp)
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));
    assert!(matches!(
        errors[1],
        AnalyzerError::UndefinedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        embed(inline) sv {{{
            \{ ModuleB \} u_monitor();
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA (
        i_c: input logic,
    ) {
        bind ModuleB <- u_c: ModuleC (
            i_c,
        );
    }
    module ModuleB (
        i_b: input logic,
    ){}
    module ModuleC (
        i_c: input logic,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA (
        i_c: input logic,
    ) {
        bind ModuleB <- u_c: ModuleC (
            i_c: i_c,
        );
    }
    module ModuleB (
        i_b: input logic,
    ){}
    module ModuleC (
        i_c: input logic,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA::<A: u32> {
        let a: logic<A> = 0;
    }
    module ModuleB::<B: u32> (
        b: input logic<B>,
    ) {}
    bind ModuleA::<32> <- u: ModuleB::<32> (
        b: a
    );
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
fn invalid_embed() {
    let code = r#"
    module ModuleA {
        embed (inline) py{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    module ModuleA {
        embed (cocotb) sv{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    module ModuleA {
        embed (cocotb) py{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    interface InterfaceA {
        embed (inline) py{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    interface InterfaceA {
        embed (cocotb) sv{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    interface InterfaceA {
        embed (cocotb) py{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    package PkgA {
        embed (inline) py{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    package PkgA {
        embed (cocotb) sv{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));

    let code = r#"
    package PkgA {
        embed (cocotb) py{{{
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidEmbed { .. }));
}

#[test]
fn invalid_embed_identifier() {
    let code = r#"
    module ModuleA {}
    module ModuleB {
        embed (inline) sv{{{
            \{ ModuleA \} u_module_a ();
        }}}
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {}
    embed (inline) sv {{{
        module ModuleB;
            \{ ModuleA \} u_module_a ();
        endmodule
    }}}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {}
    embed (cocotb) py{{{
        \{ ModuleA \}
    }}}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidEmbedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {}
    embed (cocotb) sv{{{
        \{ ModuleA \}
    }}}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidEmbedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {}
    embed (inline) py{{{
        \{ ModuleA \}
    }}}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidEmbedIdentifier { .. }
    ));
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
    interface InterfaceA {
        var a: logic;

        function Func() -> logic {
            var b: logic;
            b = 0;
            return b;
        }

        modport mp {
            ..input
        }
    }
    module ModuleA (
        a_if: modport InterfaceA::mp,
    ) {
        let _a: logic = a_if.a;
        let _b: logic = a_if.b;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));

    let code = r#"
    module ModuleA {
        let _a: logic = 0;
        let _b: logic = _a._a;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));

    let code = r#"
    interface InterfaceA::<W: u32> {
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
    interface InterfaceA::<W: u32> {
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
    interface InterfaceA::<W: u32> {
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

    let code = r#"
    module ModuleA {
        function FuncA (
            a: input logic
        ) -> logic {
            return a;
        }

        let _a: logic = FuncA(
            aa: 0,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));

    let code = r#"
    package Pkg {
        struct FooBar {
            foo: logic,
            bar: logic,
        }
    }
    module ModuleA #(
        const FooBar: type = Pkg::FooBar,
    )(
        i_a: input  FooBar,
        i_b: input  FooBar,
        o_c: output FooBar,
    ) {
        assign o_c.foo = i_a.foo + i_b.foo;
        assign o_c.bar = i_a.bar + i_b.bar;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package Pkg::<W: u32> {
        const WIDTH: u32 = W;
        struct FooBar {
            foo: logic<WIDTH>,
            bar: logic<WIDTH>,
        }
    }
    module ModuleA::<W: u32 = 2, T: type = Pkg::<W>::FooBar> (
        i_a: input  T,
        i_b: input  T,
        o_c: output T,
    ) {
        assign o_c.foo = i_a.foo + i_b.foo;
        assign o_c.bar = i_a.bar + i_b.bar;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA::<W: u32> {
        var a: logic<W>;
        modport mp {
            a: output,
        }
    }
    alias interface AliasIf = InterfaceA::<10>;
    module ModuleA (
        foo_if: modport AliasIf::mp,
    ){
        assign foo_if.a = 0;

        function FuncA(
            bar_if: modport AliasIf::mp,
        ) {
            bar_if.a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        struct Foo {
            foo: logic,
        }
    }
    proto package ProtoPkgB {
        const FOO: PkgA::Foo;
    }
    module ModuleA::<PKG: ProtoPkgB> {
        let _foo: logic = PKG::FOO.foo;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package CommonPkg {
        struct Foo {
            foo: logic,
        }
        enum Bar {
            BAR,
        }
    }
    proto package ProtoPkg {
        type Foo = CommonPkg::Foo;
        type Bar = CommonPkg::Bar;
    }
    package Pkg for ProtoPkg {
        type Foo = CommonPkg::Foo;
        type Bar = CommonPkg::Bar;
    }
    module ModuleA::<PKG: ProtoPkg> {
        var _foo: PKG::Foo;
        var _bar: PKG::Bar;
        assign _foo.foo = 0;
        assign _bar     = PKG::Bar::BAR;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        struct Foo {
            foo: logic,
        }
    }
    interface InterfaceA::<Pkg: ProtoPkg> {
        var foo: Pkg::Foo;
        modport mp {
            foo: input,
        }
    }
    module ModuleB::<Pkg: ProtoPkg> (
        if_a: modport InterfaceA::<Pkg>::mp,
    ) {
        var _foo: logic;
        assign _foo = if_a.foo.foo;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package FooPkg {
        struct Foo {
            foo: logic,
        }
    }
    package BarPkg {
        type Foo = FooPkg::Foo;
    }
    module ModuleA {
        let _foo: BarPkg::Foo = BarPkg::Foo'{ foo: 0 };
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package FooPkg {
        struct Foo {
            foo: logic,
        }
    }
    proto package BarProtoPkg {
        type Foo = FooPkg::Foo;
    }
    module ModuleA::<PKG: BarProtoPkg> {
        let _foo: PKG::Foo = PKG::Foo'{ foo: 0 };
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package foo_proto_pkg {
        const WIDTH: u32;
        struct foo_struct {
        foo: logic<WIDTH>,
        }
    }
    package foo_pkg::<W: u32> for foo_proto_pkg {
        const WIDTH: u32 = W;
        struct foo_struct {
        foo: logic<WIDTH>,
        }
    }
    interface foo_if::<PKG: foo_proto_pkg> {
        var foo: PKG::foo_struct;
        modport mp {
            foo: input,
        }
    }
    module foo_module::<PKG: foo_proto_pkg> {
        inst foo: foo_if::<PKG>;
        var _foo: logic;
        always_comb {
            _foo = foo.foo.foo;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
    package PackageA::<W: u32> {
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

    let code = r#"
    proto package foo_proto_pkg {
        type Foo;
    }
    module ModuleA::<PKG: foo_proto_pkg> {
        import PKG::*;
        let foo     : Foo   = 0;
        let _msb_foo: logic = foo[msb];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMsb { .. }));

    let code = r#"
    proto package foo_proto_pkg {
        type Foo = logic<2>;
    }
    module ModuleA::<PKG: foo_proto_pkg> {
        import PKG::*;
        let foo     : Foo   = 0;
        let _msb_foo: logic = foo[msb];
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
fn unenclosed_inner_if_expression() {
    let code = r#"
    module ModuleA {
        let _a: u32 = if if 1 ? 2 : 3 ? 4 : 5;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnenclosedInnerIfExpression { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = if (if 1 ? 2 : 3) ? 4 : 5;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: u32 = if 1 ? if 2 ? 3 : 4 : 5;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnenclosedInnerIfExpression { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = if 1 ? (if 2 ? 3 : 4) : 5;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleC {
        var memory: logic<32>[32];
        var _d    : logic<32>    ;

        initial {
            $readmemh("calc.bin", memory);
        }

        assign _d = memory[0];
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleD (
        i_clk:    input 'a clock,
        i_clk_en: input 'a logic,
    ) {
        let clk: 'a default clock = i_clk & i_clk_en;
        var _a : 'a logic;

        always_ff {
            _a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
    module ModuleA::<A: u32>(
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

    let code = r#"
    interface InterfaceA {
        var a: logic;

        modport master {
            a: output,
        }
        modport slave {
            ..converse(master)
        }
    }
    module ModuleA {
        inst a_if: InterfaceA;
        inst b_if: InterfaceA;
        always_comb {
            a_if.master <> b_if.slave;
            b_if.master <> 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;

        modport master {
            a: output,
        }
        modport slave {
            ..converse(master)
        }
    }
    module ModuleA (
        a_if: modport InterfaceA::master,
    ){
        inst b_if: InterfaceA;
        always_comb {
            a_if        <> b_if.slave;
            b_if.master <> 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module Foo (
        bar: output logic,
        baz: output logic,
    ) {
        always_comb {
            if true {
                bar = 0;
                baz = bar;
            } else {
                bar = 1;
                baz = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module Foo (
        bar: output logic,
        baz: output logic,
    ) {
        always_comb {
            if true {
                if true {
                    baz = bar;
                } else {
                    baz = 0;
                }
                if true {
                    bar = 0;
                } else {
                    bar = 1;
                }
            } else {
                bar = 1;
                baz = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module Foo (
        bar: output logic,
        baz: output logic,
    ) {
        always_comb {
            if true {
                baz = bar;
            } else {
                baz = 0;
            }
            bar = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module Foo (
        bar: output logic,
        baz: output logic,
    ) {
        always_comb {
            if true {
                baz = bar;
            } else {
                baz = 0;
            }

            if true {
                bar = 0;
            } else {
                bar = 1;
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
            a = 0;
            a = a + 1;
        }
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

    let code = r#"
    module ModuleC {
        var a: logic;
        let x: logic = 1;
        let y: logic = 1;

        always_comb {
            if x {
                if y {
                    a = 1;
                }
            } else {
                a = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UncoveredBranch { .. }));

    // TODO
    // Adapt 'traverse_assignable_symbol' to interface/struct/union members
    //let code = r#"
    //interface InterfaceA {
    //    var a: logic;
    //
    //    modport master {
    //        a: output,
    //    }
    //}
    //module ModuleA {
    //    inst a_if: InterfaceA;
    //    let x: logic = 1;
    //    always_comb {
    //        if x {
    //            a_if.master <> 0;
    //        }
    //    }
    //}
    //"#;
    //
    //let errors = analyze(code);
    //assert!(matches!(errors[0], AnalyzerError::UncoveredBranch { .. }));
}

#[test]
fn anonymous_identifier() {
    let code = r#"
    module ModuleA (
        i_clk: input '_ clock,
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

    let code = r#"
    module ModuleA::<W: u32> (
        o_a: output logic<W>,
    ) {
        assign o_a = '0;
    }
    module ModuleB {
        inst u: ModuleA::<8> (
            o_a: _
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<W: u32> (
        o_a: output logic<W>,
    ) {
        assign o_a = '0;
    }
    alias module AliasModule = ModuleA::<8>;
    module ModuleB {
        inst u: AliasModule (
            o_a: _
        );
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

    let code = r#"
    module ModuleA::<A: u32> (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var _a: logic;
        always_ff {
            if_reset {
                _a = A;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<A: u32> (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var _a: logic<A>;
        always_ff {
            if_reset {
                _a = {1'b0 repeat A};
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    module ModuleA {
        const A: type = logic + logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        const A: type = {logic};
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA::<t: u32> {
        const A: type = t;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA::<t: type> {
        const A: type = t;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const A: u32  = 0;
        const B: type = A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        const A: type = logic;
        const B: type = A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkgA {
        const A: type;
    }
    module ModuleA::<PKG: ProtoPkgA> {
        const A: type = PKG::A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkgA {
        type A;
    }
    module ModuleA::<PKG: ProtoPkgA> {
        const A: type = PKG::A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: logic = logic + logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA #(
        param T: type = 0,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA #(
        param T: type = logic,
    ) {}
    module ModuleB {
        inst u: ModuleA #(T: 0);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA #(
        param T: type = logic,
    ) {}
    module ModuleB::<t: u32> {
        inst u: ModuleA #(T: t);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA #(
        param T: type = logic,
    ) {}
    module ModuleB::<t: type> {
        inst u: ModuleA #(T: t);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<T: type = 0> {
    }
    alias module A = ModuleA::<>;
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        let _a: u32 = $clog2(logic);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        always_comb {
            $clog2(logic);
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        let _a: u32 = $bits(logic);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let a : logic<2> = 0;
        let _b: logic = a[logic];
        let _c: logic = a[1:logic];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));
    assert!(matches!(errors[1], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module ModuleA {
        var a: logic<2>;
        var b: logic<2>;

        always_comb {
            a[logic:0] = 0;
            b[1:logic] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));
    assert!(matches!(errors[1], AnalyzerError::InvalidFactor { .. }));

    let code = r#"
    module Y (
        a: modport $sv::InterfaceA::slave,
    ) {}

    module X {
        inst x: $sv::InterfaceA;
        inst u: Y (
            a: x,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport slave {
            a: input,
        }
    }
    alias interface InterfaceB = InterfaceA;

    module Y (
        a: modport InterfaceA::slave,
    ) {}

    module X {
        inst x: InterfaceB;
        inst u: Y (
            a: x,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // This will be reported as Mismatch Type error.
    //    let code = r#"
    //    module ModuleA::<T: type> {
    //    }
    //    alias module A = ModuleA::<0>;
    //    "#;
    //
    //    let errors = analyze(code);
    //    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    // This will be reported as Mismatch Type error.
    //    let code = r#"
    //    module ModuleA {
    //        const A: u32 = 0;
    //        type T = A;
    //    }
    //    "#;
    //
    //    let errors = analyze(code);
    //    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

    // This will be reported as Mismatch Type error.
    //    let code = r#"
    //    module ModuleA::<t: u32> {
    //        type T = t;
    //    }
    //    "#;
    //
    //    let errors = analyze(code);
    //    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));
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

    let code = r#"
    interface FooIF {
        var clk: clock;
        var rst: reset;
        modport mp {
            clk: input,
            rst: input,
        }
    }
    module BarModule (
        foo_if: modport FooIF::mp,
    ) {
        let _clk: clock = foo_if.clk as clock;
        let _rst: reset = foo_if.rst as reset;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
        i_clk: input  'a clock,
        i_dat: input  'a logic,
        o_dat: output 'b logic,
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
        i_clk : input  'a clock,
        i_dat0: input  'a logic,
        i_dat1: input  'b logic,
        o_dat : output 'a logic,
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
        i_clk : input  'a clock,
        i_dat0: input  'a logic,
        i_dat1: input  'b logic,
        o_dat : output 'a logic,
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
        i_clk : input  'a clock,
        i_dat0: input  'a logic,
        i_dat1: input  'b logic,
        o_dat : output 'a logic,
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
        i_clk : input  'a clock,
        i_dat0: input  'a logic,
        i_dat1: input  'b logic,
        o_dat : output 'b logic,
    ) {
        var r_dat: 'b logic;

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
        i_clk: input   'a clock,
        i_dat: input   'a logic,
        o_dat: modport 'b InterfaceA::port,
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
        i_clk: input  'a clock,
        i_dat: input  'a logic,
        o_dat: output 'b logic,
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
      i_clk: input 'a clock,
      i_dat: input 'a logic,
    ) {
        inst intf: 'a InterfaceI;
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
      i_clk: input 'a clock,
      i_dat: input 'a logic,
    ) {
        inst intf: 'b InterfaceJ;
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
      i_clk: input  'a clock,
      o_dat: output 'b logic,
    ) {
        inst intf: 'a InterfaceK;

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
        i_clk_a: input 'a clock,
        i_rst_a: input 'a reset,
        i_clk_b: input 'b clock,
        i_rst_b: input 'b reset,
    ) {
        var _a: 'a logic;
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
        i_clk_a: input 'a clock,
        i_clk_b: input 'b clock,
        i_rst_b: input 'b reset,
    ) {
        var _a: 'a logic;
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
        i_clk_a: input 'a clock,
        i_rst_b: input 'b reset,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleO (
        i_clk_a: input 'a default clock,
        i_rst_a: input 'a         reset,
        i_clk_b: input 'b         clock,
        i_rst_b: input 'b default reset,
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

    let code = r#"
    module ModuleA {
        struct event {
            x: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::SvKeywordUsage { .. }));

    let code = r#"
    module ModuleA {
        struct X {
            event: logic,
        }
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

    package PackageA::<T: u32> {
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

    let code = r#"
    module ModuleA::<PKG: ProtoPkg> {
        function FuncB() -> u32 {
            return FuncA::<PKG::T>();
        }
    }
    proto package ProtoPkg {
        type T;
    }
    package Pkg::<W: u32> for ProtoPkg {
        type T = logic<W>;
    }
    module ModuleB {
        inst u: ModuleA::<Pkg::<8>>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericArgument { .. }
    ));

    let code = r#"
    package Pkg {
        function Func::<T: type> {}
    }
    module ModuleA {
        type MyType = logic;
        always_comb {
            Pkg::Func::<MyType>();
        }
    }
    "#;

    let errors = analyze(code);
    // This pattern also causes CyclicTypeDependency error
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnresolvableGenericArgument { .. }))
    );

    let code = r#"
    package Pkg {
        function Func::<V: u32> {}
    }
    module ModuleA {
        const V: u32 = 0;
        always_comb {
            Pkg::Func::<V>();
        }
    }
    "#;

    let errors = analyze(code);
    // This pattern also causes CyclicTypeDependency error
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnresolvableGenericArgument { .. }))
    );

    let code = r#"
    module ModuleA {
        struct Foo {
            foo: logic,
        }
        let foo: Foo = Foo'{ foo: 0 };

        function Func::<foo: Foo> {}
        always_comb {
            Func::<foo>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericArgument { .. }
    ));

    let code = r#"
    package PkgA {
        struct Baz {
            baz: logic,
        }
        struct Bar {
            bar: Baz
        }
    }
    package PkgB {
        import PkgA::*;
        const FOO: Bar = Bar'{ bar: Baz'{ baz: 1 } };
    }
    package PkgC {
        import PkgA::*;
        function Func::<baz: Baz> -> logic {
            return baz.baz;
        }
    }
    module ModuleA {
        import PkgB::*;
        always_comb {
            PkgC::Func::<PkgB::FOO.bar>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
        const I: u32 = if B == 6 ? 10 : 20;
        const J: u32 = $clog2(12);
        const K: u32 = B[2:1];
        const L: u32 = $clog2(8);
        const M: bit<128> = 128'hffff_ffff_ffff_ffff_ffff_ffff_ffff_fff0 + 1;
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
    let l = symbol_table::resolve((&Into::<SymbolPath>::into("L"), &namespace)).unwrap();
    let m = symbol_table::resolve((&Into::<SymbolPath>::into("M"), &namespace)).unwrap();

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
    let l = l.found.evaluate();
    let m = m.found.evaluate();

    assert_eq!(
        (a.get_value(), a.get_total_width()),
        (Some(0.into()), Some(32))
    );
    assert_eq!(
        (b.get_value(), b.get_total_width()),
        (Some(6.into()), Some(32))
    );
    assert_eq!(
        (c.get_value(), c.get_total_width()),
        (Some(15.into()), Some(32))
    );
    assert_eq!(
        (d.get_value(), d.get_total_width()),
        (Some(6.into()), Some(32))
    );
    assert_eq!(
        (e.get_value(), e.get_total_width()),
        (Some(6.into()), Some(32))
    );
    assert_eq!(
        (f.get_value(), f.get_total_width()),
        (Some(6.into()), Some(32))
    );
    assert_eq!(
        (g.get_value(), g.get_total_width()),
        (Some(39.into()), Some(32))
    );
    assert_eq!(
        (h.get_value(), h.get_total_width()),
        (Some(362.into()), Some(9))
    );
    assert_eq!(
        (i.get_value(), i.get_total_width()),
        (Some(10.into()), Some(32))
    );
    assert_eq!(
        (j.get_value(), j.get_total_width()),
        (Some(4.into()), Some(32))
    );
    assert_eq!(
        (k.get_value(), k.get_total_width()),
        (Some(3.into()), Some(2))
    );
    assert_eq!(
        (l.get_value(), l.get_total_width()),
        (Some(3.into()), Some(32))
    );
    assert_eq!(
        (m.get_value(), m.get_total_width()),
        (
            Some(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_fff1u128.into()),
            Some(128)
        )
    );
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

#[test]
fn check_connect_operation() {
    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport master {
            a: output,
        }
        modport slave {
            a: input,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA;
        inst b_if: InterfaceA;
        connect a_if <> b_if.slave;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport master {
            a: output,
        }
        modport slave {
            a: input,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA;
        inst b_if: InterfaceA;
        connect a_if.master <> b_if;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport master {
            a: output,
        }
        modport slave {
            a: input,
        }
    }
    module ModuleA (
        i_a: input logic,
    ) {
        inst b_if: InterfaceA;
        connect i_a <> b_if.slave;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport master {
            a: output,
        }
        modport slave {
            a: input,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA[2];
        inst b_if: InterfaceA;
        connect a_if.master <> b_if.slave;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidConnectOperand { .. }
    ));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport master {
            a: output,
        }
        modport slave {
            a: input,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA;
        inst b_if: InterfaceA[2];
        connect a_if[0].master <> b_if.slave;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidConnectOperand { .. }
    ));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport master {
            a: output,
        }
        modport slave {
            a: input,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA[2, 2];
        inst b_if: InterfaceA;
        connect a_if[0].master <> b_if.slave;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidConnectOperand { .. }
    ));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport master {
            a: output,
        }
        modport slave {
            a: input,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA;
        inst b_if: InterfaceA[2, 2];
        connect a_if[0].master <> b_if[0].slave;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidConnectOperand { .. }
    ));

    let code = r#"
    interface InterfaceA {
        var a: tri logic;
        modport mp {
            a: inout,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA;
        always_comb {
            a_if.mp <> 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidConnectOperand { .. }
    ));

    let code = r#"
    interface InterfaceA {
        type A = logic<2>;

        var a: A;

        modport mp {
            a: output,
        }
    }
    interface InterfaceB {
        var a: logic;

        modport mp {
            a: input,
        }
    }
    module ModuleA {
        inst a_if: InterfaceA;
        inst b_if: InterfaceB;
        connect a_if.mp <> b_if.mp;
        connect b_if.mp <> 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidConnectOperand { .. }
    ));
}

#[test]
fn mixed_function_argument() {
    let code = r#"
    module ModuleA {
        function FuncA (
            a: input logic,
            b: input logic,
        ) -> logic {
            return a + b;
        }

        let _a: logic = FuncA(
            0,
            a: 0,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MixedFunctionArgument { .. }
    ));
}

#[test]
fn ambiguous_elsif() {
    let code = r#"
    module ModuleA {
        #[ifdef(A)]
        let _a: logic = 0;
        #[elsif(B)]
        let _a: logic = 0;
        #[else]
        let _a: logic = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        #[ifdef(A)]
        #[ifdef(B)]
        #[ifdef(C)]
        let _a: logic = 0;
        #[elsif(D)]
        let _a: logic = 0;
        #[else]
        let _a: logic = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::AmbiguousElsif { .. }));

    let code = r#"
    module ModuleA {
        #[ifdef(A)]
        let _a: logic = 0;
        #[elsif(B)]
        #[ifdef(A)]
        let _a: logic = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::AmbiguousElsif { .. }));

    let code = r#"
    module ModuleA {
        #[ifdef(A)]
        let _a: logic = 0;
        #[elsif(B)]
        #[elsif(C)]
        let _a: logic = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::AmbiguousElsif { .. }));

    let code = r#"
    module ModuleA {
        #[elsif(A)]
        let _a: logic = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::AmbiguousElsif { .. }));

    let code = r#"
    module ModuleA {
        #[else]
        let _a: logic = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::AmbiguousElsif { .. }));
}

#[test]
fn unexpandable_modport() {
    let code = r#"
    interface InterfaceA #(
        param WIDTH: u32 = 1
    ) {
        var a: logic<WIDTH>;
        modport mp {
            a: input,
        }
    }
    #[expand(modport)]
    module ModuleA (
        if_a: modport InterfaceA::mp,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnexpandableModport { .. }
    ));

    let code = r#"
    interface InterfaceA {
        type VectorA = logic<2>;
        var a: VectorA;
        modport mp {
            a: input,
        }
    }
    #[expand(modport)]
    module ModuleA (
        if_a: modport InterfaceA::mp,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        struct StructA {
            a: logic,
        }
    }
    interface InterfaceA {
        type StructA = PkgA::StructA;
        var a: StructA;
        modport mp {
            a: input,
        }
    }
    #[expand(modport)]
    module ModuleA (
        if_a: modport InterfaceA::mp,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        enum EnumA {
            A,
        }
    }
    interface InterfaceA {
        type EnumA = PkgA::EnumA;
        var a: EnumA;
        modport mp {
            a: input,
        }
    }
    #[expand(modport)]
    module ModuleA (
        if_a: modport InterfaceA::mp,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA #(
        param WIDTH: u32 = 1
    ) {
        var a: logic<WIDTH>;
        modport mp {
            a: input,
        }
    }
    module ModuleA {
        function Func(
            if_a: modport InterfaceA::mp,
        ) {}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnexpandableModport { .. }
    ));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    module ModuleA {
        function Func(
            if_a: modport InterfaceA::mp [2],
        ) {}
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnexpandableModport { .. }
    ));
}

#[test]
fn recursive_module_instance() {
    let code = r#"
    module ModuleA #(
        param WIDTH: u32 = 2,
    )(
        i_a: input  logic<WIDTH>,
        o_b: output logic       ,
    ) {
        if WIDTH == 1 :g {
            assign o_b = i_a;
        } else if WIDTH == 2 {
            assign o_b = i_a[0] | i_a[1];
        } else {
            var result: logic<2>;

            inst u0: ModuleA #(
                WIDTH: WIDTH / 2
            )(
                i_a: i_a[WIDTH/2-1:0],
                o_b: result[0]       ,
            );

            inst u1: ModuleA #(
                WIDTH: WIDTH / 2
            )(
                i_a: i_a[WIDTH-1:WIDTH/2],
                o_b: result[1]           ,
            );

            inst u2: ModuleA #(
                WIDTH: 2
            )(
                i_a: result,
                o_b: o_b   ,
            );
        }
    }
    module ModuleB (
        i_a: input  logic<8>,
        o_b: output logic   ,
    ){
        inst u: ModuleA #(
            WIDTH: 8
        )(
            i_a,
            o_b,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto module ProtoModuleA (
        i_a: input  logic,
        i_b: input  logic,
        o_c: output logic,
    );
    module ModuleA for ProtoModuleA (
        i_a: input  logic,
        i_b: input  logic,
        o_c: output logic,
    ){
        assign o_c = i_a | i_b;
    }
    module ModuleB::<M: ProtoModuleA>#(
        param WIDTH: u32 = 2,
    )(
        i_a: input  logic<WIDTH>,
        o_b: output logic       ,
    ) {
        if WIDTH == 1 :g {
            assign o_b = i_a;
        } else if WIDTH == 2 {
            inst u: M (
                i_a: i_a[0],
                i_b: i_a[1],
                o_c: o_b   ,
            );
        } else {
            var result: logic<2>;

            inst u0: ModuleB::<M>#(
                WIDTH: WIDTH / 2
            )(
                i_a: i_a[WIDTH/2-1:0],
                o_b: result[0]       ,
            );

            inst u1: ModuleB::<M>#(
                WIDTH: WIDTH / 2
            )(
                i_a: i_a[WIDTH-1:WIDTH/2],
                o_b: result[1]           ,
            );

            inst u2: ModuleB::<M>#(
                WIDTH: 2
            )(
                i_a: result,
                o_b: o_b   ,
            );
        }
    }
    module ModuleC (
        i_a: input  logic<8>,
        o_b: output logic   ,
    ){
        inst u: ModuleB::<ModuleA>#(
            WIDTH: 8
        )(
            i_a,
            o_b,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn unsigned_loop_variable_in_descending_order_for_loop() {
    let code = r#"
    module ModuleA {
        var _a: logic<10>;
        always_comb {
            for i: u32 in rev 0..10 {
                _a += i;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnsignedLoopVariableInDescendingOrderForLoop { .. }
    ));

    let code = r#"
    module ModuleA {
        type my_type = logic<4>;

        var _a: logic<10>;
        always_comb {
            for i: my_type in rev 0..10 {
                _a += i;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnsignedLoopVariableInDescendingOrderForLoop { .. }
    ));

    let code = r#"
    module ModuleA {
        var _a: logic<10>;
        always_comb {
            for i: i32 in rev 0..10 {
                _a += i;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        type my_type = signed logic<4>;

        var _a: logic<10>;
        always_comb {
            for i: my_type in rev 0..10 {
                _a += i;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn fixed_type_with_signed_modifier() {
    let code = r#"
    module ModuleA {
        let _a: signed u32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::FixedTypeWithSignedModifier { .. }
    ));

    let code = r#"
    module ModuleA {
        type my_type = signed u32;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::FixedTypeWithSignedModifier { .. }
    ));

    let code = r#"
    module ModuleA {
        type my_type = u32;
        let _a: signed my_type = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::FixedTypeWithSignedModifier { .. }
    ));

    let code = r#"
    module ModuleA {
        type my_type_0 = u32;
        type my_type_1 = my_type_0;
        let _a: signed my_type_1 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::FixedTypeWithSignedModifier { .. }
    ));
}
