use crate::{symbol_table, Analyzer, AnalyzerError};
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
    errors.append(&mut analyzer.analyze_pass1(&"prj", &code, &"", &parser.veryl));
    Analyzer::analyze_post_pass1();
    errors.append(&mut analyzer.analyze_pass2(&"prj", &code, &"", &parser.veryl));
    errors.append(&mut analyzer.analyze_pass3(&"prj", &code, &"", &parser.veryl));
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
    interface InterfaceE {
        var e: logic;
        modport mp {
            e: ref,
        }
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
    module ModuleB_0 {}
    module ModuleB_1 {
        inst u: ModuleB_0;
        let _b: u = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleC {
        function FuncC() -> logic {
            return 0;
        }
        let _c: FuncC = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleD_0 {}
    module ModuleD_1 {
        let _d: ModuleD_0 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceE {}
    module ModuleE {
        let _e: InterfaceE = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    package PackageF {}
    module ModuleF {
        let _f: PackageF = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleG {
        function FuncG::<T: type> -> T {
            var g: T;
            g = 0;
            return g;
        }

        let _g: logic = FuncG::<2>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleH {
        function FuncH::<T: type> -> T {
            var h: T;
            h = 0;
            return h;
        }

        type my_logic = logic;
        let _h: logic = FuncH::<my_logic>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceI {}

    module ModuleI (
        a: modport InterfaceI,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleJ {
        function FuncJ::<T: type> -> T {
            var g: T;
            g = 0;
            return g;
        }

        const X: u32 = 1;
        let _g: logic = FuncJ::<X>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoK0;
    proto module ProtoK1;

    module ModuleK0::<T: ProtoK0> {
        inst u: T;
    }

    module ModuleK1 for ProtoK1 {}

    module ModuleK2 {
        inst u: ModuleK0::<ModuleK1>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceL0 {}
    interface InterfaceL1 {
      inst u: InterfaceL0();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleM0 {}
    interface InterfaceM1 {
      inst u: ModuleM0();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceN1 {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    module ModuleN1 (
        port_n1: input InterfaceN1::mp,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceN2::<W: const> {
        var a: logic<W>;
        modport mp {
            a: input,
        }
    }
    module ModuleN2 (
        port_n2: input InterfaceN2::<2>::mp,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceN3 {
        var a: logic;
    }
    module ModuleN3 (
        port_n3: input InterfaceN3,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceN4::<W: const> {
        var a: logic<W>;
    }
    module ModuleN4 (
        port_n4: input InterfaceN4::<2>,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceO1 {
        var a: logic;
    }
    module ModuleO1 (
        port_o1: modport InterfaceO1,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    interface InterfaceO2::<W: const> {
        var a: logic<W>;
    }
    module ModuleO2 (
        port_o2: modport InterfaceO2::<2>,
    ){}
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
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericArgument { .. }
    ));
}
