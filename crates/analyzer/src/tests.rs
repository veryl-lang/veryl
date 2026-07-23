use crate::conv::Context;
use crate::ir::Ir;
use crate::{Analyzer, AnalyzerError, attribute_table, symbol_table};
use std::collections::HashMap;
use std::thread;
use veryl_metadata::{Lint, Metadata, ProjectProperty};
use veryl_parser::Parser;
use veryl_parser::doc_comment_table;

#[track_caller]
fn analyze(code: &str) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
    dbg!(&errors);
    errors
}

#[track_caller]
fn analyze_with_lint(code: &str, lint: Lint) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.lint = lint;
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
    dbg!(&errors);
    errors
}

#[track_caller]
fn analyze_multiple_inputs(inputs: &[&str]) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let prj_name = "prj";
    let metadata = Metadata::create_default(prj_name).unwrap();

    let mut contexts = vec![];
    let mut errors = vec![];
    for (i, input) in inputs.iter().enumerate() {
        let path = format!("test_{}.veryl", i);

        let parser = Parser::parse(input, &path).unwrap();
        let analyzer = Analyzer::new(&metadata);
        errors.append(&mut analyzer.analyze_pass1(prj_name, &parser.veryl));

        contexts.push((parser, analyzer));
    }

    errors.append(&mut Analyzer::analyze_post_pass1());

    let mut analyzer_context = Context::default();
    let mut ir = Ir::default();
    for (parser, analyzer) in &contexts {
        errors.append(&mut analyzer.analyze_pass2(
            &parser.veryl,
            &mut analyzer_context,
            Some(&mut ir),
        ));
    }

    errors.append(&mut Analyzer::analyze_post_pass2(&ir));

    dbg!(&errors);
    errors
}

#[track_caller]
fn analyze_with_ir(code: &str) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
    dbg!(&errors);
    errors
}

#[track_caller]
fn analyze_with_large_stack(code: &str) -> Vec<AnalyzerError> {
    let code = code.to_string();

    // cargo test uses 2MB stack by default
    // some tests like recursive check need more stack size
    let builder = thread::Builder::new().stack_size(16 * 1024 * 1024);
    let handler = builder
        .spawn(move || {
            symbol_table::clear();
            attribute_table::clear();
            doc_comment_table::clear();

            let metadata = Metadata::create_default("prj").unwrap();
            let parser = Parser::parse(&code, &"").unwrap();
            let analyzer = Analyzer::new(&metadata);
            let mut context = Context::default();
            let mut ir = Ir::default();

            let mut errors = vec![];
            errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
            errors.append(&mut Analyzer::analyze_post_pass1());
            errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
            errors.append(&mut Analyzer::analyze_post_pass2(&ir));
            dbg!(&errors);
            errors
        })
        .unwrap();
    handler.join().unwrap()
}

fn analyze_with_project_properties(
    code: &str,
    properties: HashMap<String, ProjectProperty>,
) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let mut metadata = Metadata::create_default("prj").unwrap();
    for (name, value) in properties {
        metadata.properties.insert(name, value);
    }

    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
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
    assert!(matches!(errors[0], AnalyzerError::MultipleDefault { .. }));
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
    assert!(matches!(
        errors[0],
        AnalyzerError::ImplicitClockConversion { .. }
    ));

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
fn implicit_clock_conversion() {
    // logic variable wired into a clock input port
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
        o_q  : output logic,
    ) {
        var w_gclk: logic;
        assign w_gclk = i_clk;
        inst u: ModuleB ( clk: w_gclk, rst: i_rst, o_q: o_q );
    }
    module ModuleB (
        clk: input clock,
        rst: input reset,
        o_q: output logic,
    ) {
        always_ff (clk, rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ImplicitClockConversion { .. }
    ));

    // inline expression at a clock port (clock-ness does not propagate
    // through operators)
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_en : input logic,
        i_rst: input reset,
        o_q  : output logic,
    ) {
        inst u: ModuleB ( clk: i_clk & i_en, rst: i_rst, o_q: o_q );
    }
    module ModuleB (
        clk: input clock,
        rst: input reset,
        o_q: output logic,
    ) {
        always_ff (clk, rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ImplicitClockConversion { .. }
    ));

    // logic wired into a reset input port (reset is symmetric)
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_raw: input logic,
        o_q  : output logic,
    ) {
        inst u: ModuleB ( clk: i_clk, rst: i_raw, o_q: o_q );
    }
    module ModuleB (
        clk: input clock,
        rst: input reset,
        o_q: output logic,
    ) {
        always_ff (clk, rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ImplicitClockConversion { .. }
    ));

    // binding-point conversion: a clock-typed binding accepts a logic
    // value, and the bound clock connects to a clock port cleanly
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_en : input logic,
        i_rst: input reset,
        o_q  : output logic,
    ) {
        let w_gclk: '_ clock = i_clk & i_en;
        inst u: ModuleB ( clk: w_gclk, rst: i_rst, o_q: o_q );
    }
    module ModuleB (
        clk: input clock,
        rst: input reset,
        o_q: output logic,
    ) {
        always_ff (clk, rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn clock_binding_source_shape() {
    // a multi-bit bus bound to a clock-typed binding keeps the
    // mismatch warning (silent LSB truncation would hide a mistake)
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
        i_bus: input logic<32>,
        o_q  : output logic,
    ) {
        let w_clk: '_ clock = i_bus;
        always_ff (w_clk, i_rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    // a struct value bound to a clock-typed binding keeps the warning
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
        o_q  : output logic,
    ) {
        struct Pair {
            a: logic,
            b: logic,
        }
        var s: Pair;
        assign s.a = 1'b0;
        assign s.b = 1'b0;
        let w_clk: '_ clock = s;
        always_ff (w_clk, i_rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));
}

#[test]
fn implicit_clock_conversion_function_arg() {
    // a logic value passed to a clock-typed function argument is an
    // implicit conversion, just like at a module input port
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_x  : input logic,
        o_q  : output logic,
    ) {
        function pass (
            c: input clock,
        ) -> logic {
            return 1'b0;
        }
        assign o_q = pass(i_x);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ImplicitClockConversion { .. }
    ));

    // passing an actual clock is clean
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        o_q  : output logic,
    ) {
        function pass (
            c: input clock,
        ) -> logic {
            return 1'b0;
        }
        assign o_q = pass(i_clk);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn implicit_clock_conversion_single_diagnostic() {
    // a cross-kind port connection (reset value into a clock port)
    // reports exactly one diagnostic: the dedicated error, without the
    // generic mismatch warning on the same connection
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
        o_q  : output logic,
    ) {
        inst u: ModuleB ( clk: i_rst, o_q: o_q );
    }
    module ModuleB (
        clk: input clock,
        o_q: output logic,
    ) {
        var r_q: logic;
        always_ff (clk) { r_q = ~r_q; }
        assign o_q = r_q;
    }
    "#;

    let errors = analyze(code);
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0],
        AnalyzerError::ImplicitClockConversion { .. }
    ));
}

#[test]
fn invalid_clock_assignment() {
    // clock-typed variable assigned in always_ff (an FF divider must be
    // written as a logic toggle + clock-typed binding instead)
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
        o_q  : output logic,
    ) {
        var w_div: '_ clock;
        always_ff (i_clk, i_rst) {
            if_reset { w_div = 0; } else { w_div = ~w_div; }
        }
        always_ff (w_div, i_rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidClockAssignment { .. }
    ));

    // the B-form divider is clean: logic toggle FF + clock-typed binding
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
        o_q  : output logic,
    ) {
        var r_tog: logic;
        always_ff (i_clk, i_rst) {
            if_reset { r_tog = 0; } else { r_tog = ~r_tog; }
        }
        let w_div: '_ clock = r_tog;
        always_ff (w_div, i_rst) {
            if_reset { o_q = 0; } else { o_q = ~o_q; }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn reset_connection_check() {
    let code = r#"
    module ModuleA (
        clk: input clock
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
    assert!(matches!(
        errors[0],
        AnalyzerError::ImplicitClockConversion { .. }
    ));
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
    module ModuleA {
        union UnionA {
            memberA: UnionA,
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

    let code = r#"
    package PkgA::<V: u32> {
        const A: u32 = V;
    }
    package PkgB {
        const B: u32 = 32;
        const A: u32 = PkgA::<B>::A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::CyclicTypeDependency { .. }
    ));

    let code = r#"
    package PkgA::<V: u32> {
        const A: u32 = V;
    }
    package PkgB {
        const B: u32 = 32;
        alias package PKG = PkgA::<B>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::CyclicTypeDependency { .. }
    ));

    let code = r#"
    package Pkg::<a: u32, b: u32> {
        const A: u32 = a;
        const B: u32 = b;
    }
    alias package Pkg0 = Pkg::<1, 2>;
    alias package Pkg1 = Pkg::<Pkg0::B, Pkg0::A>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package proto_pkg {
        const A: u32;
        const B: u32;
    }
    package pkg::<a: u32, b: u32> for proto_pkg {
        const A: u32 = a;
        const B: u32 = b;
    }
    interface if_a::<PKG: proto_pkg> {
        var a: logic<PKG::A>;
        var b: logic<PKG::B>;
        modport mp {
            ..output
        }
    }
    module module_a::<PKG: proto_pkg> {
        inst ifa: if_a::<pkg::<PKG::B, PKG::A>>;
    }
    module module_b {
        inst u: module_a::<pkg::<1, 2>>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package c_pkg::<c0: u32, c1: u32> {
        const C0: u32 = c0;
        const C1: u32 = c1;
    }
    package b_pkg::<b0: u32, b1: u32> {
        alias package c = c_pkg::<b0, b1>;
    }
    alias package c = c_pkg::<b_pkg::<1, 2>::c::C0, 3>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let inputs = vec![
        r#"
        alias package d = d_pkg::<b_pkg::c::d::D0, 3>;
        "#,
        r#"
        package d_pkg::<d0: u32, d1: u32> {
            const D0: u32 = d0;
            const D1: u32 = d1;
        }
        package c_pkg::<c0: u32, c1: u32> {
            alias package d = d_pkg::<c0, c1>;
        }
        package b_pkg {
            alias package c = c_pkg::<1, 2>;
        }
        "#,
    ];

    let errors = analyze_multiple_inputs(&inputs);
    assert!(errors.is_empty());

    // A const in a function body initialized by a call to a sibling function in
    // the same package must not be reported as a cyclic dependency of the
    // package with itself.
    let code = r#"
    package a_pkg {
        function f0() -> u32 {
            return 1;
        }
        function f1() -> u32 {
            const C: u32 = f0();
            return C;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // Mutual recursion between sibling functions is a genuine cycle and must
    // still be reported (not overflow) — the sibling-call fix above must not
    // over-suppress it.
    let code = r#"
    package a_pkg {
        function f0() -> u32 {
            return f1();
        }
        function f1() -> u32 {
            return f0();
        }
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
    module ModuleA {
        var a: logic;

        always_comb {
            a = 0;
        }
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

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic<10>;

        always_ff {
            a = 0;
            a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic<10>;

        always_ff {
            a[1:0] = 0;
            a[9:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic<4>;

        always_ff {
            a[3:0] = 0;
            a[3:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic<10>;

        always_ff {
            a = 0;
        }
        always_ff {
            a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MultipleAssignment { .. }
    ));

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic<10>;

        always_ff {
            a[1:0] = 0;
        }
        always_ff {
            a[9:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic<10>;

        always_ff {
            a[3:0] = 0;
        }
        always_ff {
            a[3:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MultipleAssignment { .. }
    ));

    let code = r#"
    module ModuleA {
        var a: logic<10>;

        always_comb {
            a = 0;
            a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<10>;

        always_comb {
            a[1:0] = 0;
            a[9:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<10>;

        always_comb {
            a[3:0] = 0;
            a[9:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<10>;

        always_comb {
            a = 0;
        }
        always_comb {
            a = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MultipleAssignment { .. }
    ));

    let code = r#"
    module ModuleA {
        var a: logic<10>;

        always_comb {
            a[1:0] = 0;
        }
        always_comb {
            a[9:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<10>;

        always_comb {
            a[3:0] = 0;
        }
        always_comb {
            a[9:2] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MultipleAssignment { .. }
    ));

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: logic[8];
        let b: logic<3> = 0;

        always_ff {
            a[0] = 0;
            if b != 0 {
                a[b] = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {
        var a: logic;
        var b: logic;
        modport mst {
            a: output,
            b: output,
        }
    }

    module ModuleA {
        inst u: InterfaceA [2];

        inst x0: ModuleB (
            p: u[0],
        );

        inst x1: ModuleB (
            p: u[1],
        );
    }

    module ModuleB (
        p: modport InterfaceA::mst,
    ) {
        assign p.a = 0;
        assign p.b = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<4*2>[2];
        for i in 0..8 :g {
            always_comb {
                a[i[2]][2*i[1:0]+:2] = '0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<4*2>[2];
        always_comb {
            for i in 0..8 {
                a[i[2]][2*i[1:0]+:2] = '0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        struct struct_a {
            a: logic<2>,
        }
        var _a: struct_a<2, 2>;
        for i in 0..2 :g {
            always_comb {
            _a[i] = '0;
            }
        }
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

    let code = r#"
    package PkgA {
    }
    module ModuleA {
        import PkgA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidImport { .. }));

    let code = r#"
    package a_pkg::<a: u32> {
        const A: u32 = a;
    }
    package b_pkg::<b: u32> {
        alias package a = a_pkg::<b>;
    }
    module c_module {
        import b_pkg::<32>::a;
        import a::*;
        const C: u32 = A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package a_proto_pkg {
        const A: u32;
    }
    package a_pkg::<a: u32> for a_proto_pkg {
        const A: u32 = a;
    }
    proto package b_proto_pkg {
        alias package a: a_proto_pkg;
    }
    package b_pkg::<b: u32> for b_proto_pkg {
        alias package a = a_pkg::<b>;
    }
    module c_module::<pkg: b_proto_pkg> {
        import pkg::*;
        import a::*;
        const C: u32 = A;
    }
    module d_module::<pkg: b_proto_pkg> {
        import pkg::*;
        import a::A;
        const D: u32 = A;
    }
    module e_module {
        inst u0: c_module::<b_pkg::<32>>;
        inst u1: d_module::<b_pkg::<32>>;
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
            for i in 0..2 {
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
fn invalid_for_range() {
    let code = r#"
    module ModuleA {
        always_comb {
            for i in 4 {
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForRange { .. })),
        "{errors:?}"
    );

    let code = r#"
    module ModuleA {
        for i in 4 :blk {
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForRange { .. })),
        "{errors:?}"
    );

    let code = r#"
    module ModuleA (
        o: output logic<32>,
    ) {
        always_comb {
            var acc: logic<32>;
            acc = 0;
            for i in 0..4 {
                acc += i;
            }
            for j in 0..=4 {
                acc += j;
            }
            o = acc;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForRange { .. })),
        "{errors:?}"
    );
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
        AnalyzerError::InvalidModportItem { .. }
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
        AnalyzerError::InvalidModportItem { .. }
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
        AnalyzerError::InvalidModportItem { .. }
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

    let code = r#"
    module ModuleA #(
        param A: bit = 0
    )(
        a: input logic = A,
    ){}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidPortDefaultValue { .. }
    ));

    let code = r#"
    module ModuleA (
        a: input logic,
        b: input logic = a,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidPortDefaultValue { .. }
    ));

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

    //let code = r#"
    //module ModuleA {
    //    initial {
    //        $readmemh();
    //    }
    //}
    //"#;

    //let errors = analyze(code);
    //assert!(matches!(
    //    errors[0],
    //    AnalyzerError::MismatchFunctionArity { .. }
    //));

    let code = r#"
    module ModuleA () {
        function func (
            a: input logic<2>,
            b: input logic   ,
        ) -> logic {
            return 0;
        }

        inst u: $sv::IF;

        always_comb {
            u.a = func(1'b1);
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
fn mismatch_function_arg() {
    let code = r#"
    module ModuleA {
        let _a: u32 = $clog2(logic);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArg { .. }
    ));

    let code = r#"
    module ModuleA {
        always_comb {
            $clog2(logic);
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArg { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = $bits(logic);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn missing_default_generic_argument() {
    let code = r#"
    module ModuleA {
        function FuncA::<A: u32> () -> logic<A> { return 0; }
        let _a: logic = FuncA::<1>();

        function FuncB::<A: u32, B: u32, C: u32> () -> logic<A + B + C> { return 0; }
        let _b: logic = FuncB::<1, 2, 3>();

        function FuncC::<A: u32 = 1> () -> logic<A> { return 0; }
        let _c: logic = FuncC::<>();

        function FuncD::<A: u32 = 1, B: u32 = 2, C: u32 = 3> () -> logic<A + B + C> { return 0; }
        let _d: logic = FuncD::<>();

        function FuncE::<A: u32, B: u32 = 2, C: u32 = 3> () -> logic<A + B + C> { return 0; }
        let _e: logic = FuncE::<1>();

        function FuncF::<A: u32, B: u32, C: u32 = 3> () -> logic<A + B + C> { return 0; }
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

    // Extra arguments to a single-argument attribute must be rejected, not silently
    // dropped (they used to mask typos).
    let code = r#"
    package PkgA {
        #[enum_encoding(sequential, garbage_extra)]
        enum E: logic<2> {
            A,
            B,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchAttributeArgs { .. }))
    );

    let code = r#"
    package PkgB {
        #[enum_member_prefix(P, garbage_extra)]
        enum E: logic<2> {
            A,
            B,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchAttributeArgs { .. }))
    );
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
        modport mp {
            a: input,
        }
    }
    module ModuleA {
        function FuncA::<IF: inst InterfaceA>() -> logic {
            return IF.a;
        }

        inst if_a: InterfaceA;
        assign if_a.a = 0;
        let _a: logic = FuncA::<if_a>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InerfaceA {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    interface InterfaceB {
        var b: logic;
        modport mp {
            b: input,
        }
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
    package PkgA {}
    module ModuleA::<PKG: ProtoPkgA> {}
    alias module A = ModuleA::<PkgA>;
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
    interface IfA {
        var a: logic;
        modport mp_0 {
            a: input,
            ..same(mp_0)
        }
        modport mp_1 {
            a: input,
            ..converse(mp_1)
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
        let _a_0: logic<32> = FuncA::<PKG::W>();
        let _a_1: logic<32> = FuncA::<WIDTH>();
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
    package FooPkg::<A: bbool = true, B: bbool = false> {
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

    let code = r#"
    proto module a_proto_module;
    module a_module for a_proto_module {}
    package b_pkg::<MOD: a_proto_module> {
        alias module A_MODULE = MOD;
    }
    proto package c_proto_pkg {
        alias module A_MODULE: a_proto_module;
    }
    package c_pkg::<MOD: a_proto_module> for c_proto_pkg {
        alias module A_MODULE = MOD;
    }
    module d_module::<PKG: c_proto_pkg> {
        import PKG::*;
        alias package B_PKG = b_pkg::<A_MODULE>;
    }
    alias package C_PKG    = c_pkg::<a_module>;
    alias module  D_MODULE = d_module::<C_PKG>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<WIDTH: u32> {
        let _a: logic<WIDTH> = 0 as WIDTH;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package Proto {
        const X: u32;
    }

    package Package for Proto {
        const X: u32 = 1;
    }

    interface InterfaceA::<PKG: Proto> {
        var a: logic;
        modport master {
            a: output,
        }
    }

    module ModuleC {
        inst u: ModuleA::<Package>;
    }

    module ModuleA::<PKG: Proto> {
        inst a: InterfaceA::<PKG>;

        inst u: ModuleB::<PKG> (
            p: a,
        );
    }

    module ModuleB::<PKG: Proto> (
        p: modport InterfaceA::<PKG>::master,
    ) {
        assign p.a = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        gen A: u32 = logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA {
        gen A: type = 2;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA::<W: u32, T: type> (
        a: input logic<W>,
        b: input T       ,
    ){
    }
    module ModuleB::<A: u32, B: u32, C: u32> {
        gen W: u32  = A + B;
        gen T: type = logic<C>;
        inst u: ModuleA::<W, T> (
            a: '0,
            b: '0,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto module ProtoModuleA;
    module ModuleB {
        gen B: ProtoModuleA = 2;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoModuleA;
    module ModuleB {
        gen B: ProtoModuleA = logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    proto module ProtoModuleA;
    proto module ProtoModuleB;
    module ModuleB for ProtoModuleB {}
    module ModuleC {
        gen C: ProtoModuleA = ModuleB;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA #(
        param VALUE: u32 = 1,
        param WIDTH: p32 = 1,
    ) (
        o: output logic<WIDTH>,
    ) {
        assign o = VALUE as WIDTH;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA #(
        param VALUE: u32 = 1,
        param WIDTH: i32 = 1,
    ) (
        o: output logic<WIDTH>,
    ) {
        assign o = VALUE as WIDTH;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package ab_pkg {
        struct a_struct {
            a: u32,
        }
        struct b_struct {
            b: a_struct,
        }
    }
    proto package c_proto_pkg {
        const C: ab_pkg::b_struct;
    }
    package c_pkg::<a: u32> for c_proto_pkg {
        const C: ab_pkg::b_struct = ab_pkg::b_struct'{
            b: ab_pkg::a_struct'{ a: a },
        };
    }
    module d_module::<pkg: c_proto_pkg> {
        import ab_pkg::*;
        const C: b_struct = pkg::C;
        const D: u32      = func_a::<C.b>();
        function func_a::<a: a_struct>() -> u32 {
            return a.a;
        }
    }
    module e_module {
        inst u: d_module::<c_pkg::<32>>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    module b_module (
        a: modport a_if,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        &errors[0],
        AnalyzerError::MismatchType {
            kind: crate::analyzer_error::MismatchTypeKind::SymbolKind { actual, .. },
            ..
        } if actual == "interface a_if"
    ));

    let code = r#"
    interface a_if {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    module b_module (
        a: modport a_if::mp,
    ) {
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto module ProtoModuleA;
    module ModuleA::<M: ProtoModuleA = 1> {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));

    let code = r#"
    module ModuleA::<M: u32 = 1> {}
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
    module ModuleA (
        a: modport InterfaceA::mp,
    ) {}
    module ModuleB {
        inst y: ModuleA (a: 0);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    interface a_if {
        var a: logic;
        var b: logic;
        modport mp_a {
            a: input,
        }
        modport mp_ab {
            a: input,
            b: input,
        }
    }
    module b_module (
        a: modport a_if::mp_a
    ) {
        inst c: c_module (
            ab: a,
        );
    }
    module c_module (
        ab: modport a_if::mp_ab,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    interface a_if {
        var a: logic;
        var b: logic;
        modport mp_a {
            a: input,
        }
        modport mp_ab {
            a: input,
            b: input,
        }
    }
    module b_module () {
        inst a: a_if();
        assign a.a = '0;
        assign a.b = '0;
        inst c: c_module (
            ab: a,
        );
    }
    module c_module (
        ab: modport a_if::mp_ab,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if::<W: u32> {
        var a: logic<W>;
        modport mp {
          a: input,
        }
    }
    module b_module (
        a: modport a_if::<32>::mp,
    ) {
        inst u: c_module (
            a: a,
        );
    }
    module c_module (
        a: modport a_if::<32>::mp,
    ) {}
    module d_module::<D: u32> {
        inst a: a_if::<32>;
        assign a.a = 0;
        inst b: b_module (
            a: a,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    //let code = r#"
    //interface InterfaceA {
    //    var a: logic;
    //    modport mp {
    //        a: input
    //    }
    //}
    //interface InterfaceB {
    //    var a: logic;
    //    modport mp {
    //        a: input
    //    }
    //}
    //module ModuleA (
    //    a: modport InterfaceA::mp,
    //) {}
    //module ModuleB {
    //    inst b: InterfaceB;
    //}
    //bind ModuleB <- u: ModuleA (
    //    a: b,
    //);
    //"#;

    //let errors = analyze(code);
    //assert!(matches!(
    //    errors[0],
    //    AnalyzerError::MismatchAssignment { .. }
    //));

    let code = r#"
    module ModuleA {
        const a: u64 = 64'hfff8000000000000;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoA {
        const WIDTH: u32;

        var ready: logic       ;
        var valid: logic       ;
        var data : logic<WIDTH>;

        function ack() -> logic ;

        modport master {
            ready: input ,
            valid: output,
            data : output,
            ack  : import,
        }
    }
    interface InterfaceA::<W: u32> for ProtoA {
        const WIDTH: u32 = W;

        var ready: logic       ;
        var valid: logic       ;
        var data : logic<WIDTH>;

        function ack () -> logic {
            return ready && valid;
        }

        modport master {
            ready: input ,
            valid: output,
            data : output,
            ack  : import,
        }
    }
    module ModuleA::<BUS_IF: ProtoA> (
        bus_if: modport BUS_IF::master,
    ) {
        connect bus_if <> 0;
    }
    module ModuleB {
        inst bus_if: InterfaceA::<8>;

        assign bus_if.ready = 1;

        inst u: ModuleA::<InterfaceA::<8>> (
            bus_if: bus_if,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA #(
        param T: type = 0,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA::<t: u32> {
        const A: type = t;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

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
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

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
    module ModuleA #(
        param T: type = logic,
    ) {}
    module ModuleB {
        inst u: ModuleA #(T: 0);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA #(
        param T: type = logic,
    ) {}
    module ModuleB::<t: u32> {
        inst u: ModuleA #(T: t);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA::<T: type = 0> {
    }
    alias module A = ModuleA::<>;
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    interface InterfaceA #(
        param N: u32 = 1,
    ){
        var a: logic;
        modport mp {
            a: input
        }
    }
    module ModuleA (
        a: modport InterfaceA::mp,
    ) {}
    module ModuleB {
        inst x: InterfaceA #( N: 10 );
        assign x.a = 0;
        inst y: ModuleA (a: x);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const A: u32    = 8 ;
        const B: bit<A> = '0;
        const C: bit<A> = B + 1 as A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const A: u32 = 0;
        const B: u32 = $sv::pkg::a | A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        enum EnumA: bit<_> {
            A = 1'bx,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA {
        enum EnumA {
            A,
        }
        const A: EnumA = EnumA::A;
        const B: bit   = A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchAssignment { .. }
    ));

    let code = r#"
    module ModuleA {
        enum EnumA: bit<_> {
            A,
        }
        const A: EnumA = EnumA::A;
        const B: bit   = A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // Array size from a `$sv::` constant: unknown dimensions should match any size.
    let code = r#"
    module Inner (
        i_data: input  logic [4],
        o_data: output logic [4],
    ) {
        assign o_data = i_data;
    }
    module ModuleA #(
        param CHANNELS: u32 = $sv::pkg::CHANNELS,
    ) (
        i_data: input  logic [CHANNELS / 2],
        o_data: output logic [CHANNELS / 2],
    ) {
        inst u: Inner (
            i_data: i_data,
            o_data: o_data,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // A function argument sharing a module port's name must not shadow the
    // port's entry in `port_types`.
    let code = r#"
    module Callee (
        i_arr: input  logic<2> [4],
        o_or : output logic<2>    ,
    ) {
        function decode (
            i_arr: input logic<2>,
        ) -> logic<2> {
            return ~i_arr;
        }
        always_comb {
            o_or = decode(i_arr[0]) | i_arr[1] | i_arr[2] | i_arr[3];
        }
    }
    module ModuleA (
        i_arr: input  logic<2> [4],
        o_or : output logic<2>    ,
    ) {
        inst u_callee: Callee (
            i_arr: i_arr,
            o_or : o_or ,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn missing_if_reset() {
    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset,
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

    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset,
    ) {
        var a: logic<2>;

        always_ff(clk, rst) {
            if_reset {
                a[0] = 0;
            } else {
                a[0] = 1;
            }
        }

        assign a[1] = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        clk: input clock,
        rst: input reset,
    ) {
        var a: logic;

        always_ff(clk, rst) {
            if_reset {
                a = 0;
            } else {
                let x: logic = 1;
                a = x;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        var a: $sv::StructA;

        always_ff {
            if_reset {
                a.a = 0;
                a.b = 0;
            } else {
                a = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module jtag_tap2 (
        i_tck : input clock,
        i_trst: input reset,
    ) {
        var data_shift_reg: logic<40>;

        always_ff {
            if_reset {
                data_shift_reg = '0;
            } else {
                {data_shift_reg[31:0]} = {data_shift_reg[31:0]};
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
fn unevaluable_value_enum_variant() {
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
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

    let code = r#"
    module ModuleC {
        #[enum_encoding(onehot)]
        enum EnumA: logic<2> {
            A = 2'bx1,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

    let code = r#"
    module ModuleD {
        #[enum_encoding(gray)]
        enum EnumA: logic<2> {
            A = 2'bx0,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

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
fn duplicate_enum_variant() {
    // Sequential auto-numbering after an explicit value collides with an earlier
    // member (A=0, B=1, C=1, D=2): the emitted SV is rejected by SV tools.
    let code = r#"
    module ModuleA {
        enum E: logic<3> {
            A,
            B,
            C = 1,
            D,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::DuplicateEnumVariant { .. }))
    );

    // Two explicit members sharing a value.
    let code = r#"
    module ModuleB {
        enum E: logic<2> {
            A = 1,
            B = 1,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::DuplicateEnumVariant { .. }))
    );

    // Distinct values must NOT be flagged.
    let code = r#"
    module ModuleC {
        enum E: logic<2> {
            A,
            B,
            C,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::DuplicateEnumVariant { .. }))
    );
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

    let code = r#"
    proto package a_proto_pkg {
        const A_TYPE: type;
    }
    package a_pkg::<a_type: type> for a_proto_pkg {
        const A_TYPE: type = a_type;
    }
    proto package b_proto_pkg {
        alias package A_PKG: a_proto_pkg;
    }
    package b_pkg::<a_type: type> for b_proto_pkg {
        alias package A_PKG = a_pkg::<a_type>;
    }
    module c_module::<B_PKG: b_proto_pkg> {
        let _c: B_PKG::A_PKG::A_TYPE = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package a_pkg {
        const A: u32 = 1;
    }
    package b_pkg {
        const B: u32 = 1;
    }
    package c_pkg::<c: u32> {
        const C: u32 = c;
    }
    package d_pkg::<a: u32, b: u32> {
        gen   a_b: u32 = a + b;
        const C  : u32 = c_pkg::<a_b>::C;
    }
    alias package pkg = d_pkg::<a_pkg::A, b_pkg::B>;
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if {
        var a: logic;
        modport mp_a {
            a: input,
        }
    }
    interface b_if {
        var b: logic;
        modport mp_b {
            b: input,
        }
    }
    interface ab_if {
        mixin a_if;
        mixin b_if;
        modport mp_ab {
            a: input,
            b: input,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if {
        var a: logic;
        modport mp_a {
            a: input,
        }
    }
    interface ab_if {
        mixin a_if;
        modport mp_ab {
            a: input,
            b: input,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
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

    let code = r#"
    interface a_if::<T: type> {
        var ready  : logic;
        var valid  : logic;
        var payload: T    ;
        modport slave {
            ready  : output,
            valid  : input ,
            payload: input ,
        }
    }
    proto package b_proto_pkg {
        const WIDTH: u32;
        struct b_struct {
            b: logic<WIDTH>,
        }
    }
    package b_pkg::<W: u32> for b_proto_pkg {
        const WIDTH: u32 = W;
        struct b_struct {
            b: logic<WIDTH>,
        }
    }
    interface c_if::<B_PKG: b_proto_pkg> {
        var ready  : logic          ;
        var valid  : logic          ;
        var payload: B_PKG::b_struct;

        function connect_if(
            aif: modport a_if::<B_PKG::b_struct>::slave,
        ) {
            aif.ready = ready;
            valid     = aif.valid;
            payload.b = aif.payload.b;
        }

        modport master {
            ready     : input ,
            valid     : output,
            payload   : output,
            connect_if: import,
        }
    }
    module d_module {
        alias package PKG = b_pkg::<32>;
        inst aif: a_if::<PKG::b_struct>;
        inst cif: c_if::<PKG>          ;
        always_comb {
            aif.valid     = '0;
            aif.payload.b = '0;
        }

        always_comb {
            cif.ready = '0;
            cif.connect_if(aif);
        }
    }
    module e_module (
        aif: modport a_if::<b_pkg::<32>::b_struct>::slave   ,
        bif: modport c_if::<b_pkg::<32>>::master            ,
        cif: modport a_if::<b_pkg::<32>::b_struct>::slave[1],
        dif: modport c_if::<b_pkg::<32>>::master[1]         ,
    ) {
        always_comb {
            bif.connect_if(aif);
        }

        always_comb {
            dif[0].connect_if(aif: cif[0]);
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<D: u32> #() {
        var a: logic<D>;
        let _b: logic = a;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    //let code = r#"
    //interface InterfaceA {
    //    var x: logic;
    //    modport master {
    //        x: output,
    //    }
    //}

    //module ModuleA {
    //    let _a: logic = b.x;
    //
    //    inst b: InterfaceA;
    //    assign b.x = 0;
    //}
    //"#;

    //let errors = analyze(code);
    //assert!(matches!(
    //    errors[0],
    //    AnalyzerError::UndefinedIdentifier { .. }
    //));

    let code = r#"
    package PkgA::<W: u32> {
        type T = logic<W>;
    }
    module ModuleB::<W: u32> {
        gen WW: u32 = 2 * W;
        let _a: PkgA::<WW>::T = '0;
    }
    module ModuleC {
        inst u: ModuleB::<1>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package a_proto_pkg {
        const WIDTH: u32;
    }
    package a_pkg::<W: u32> for a_proto_pkg {
        const WIDTH: u32 = W;
    }
    interface b_if::<PKG: a_proto_pkg> {
        var b: logic<PKG::WIDTH>;
        modport mp {
            b: input,
        }
    }
    package c_pkg {
        gen W: u32 = 1 + 1;
        alias package a = a_pkg::<W>;
    }
    module d_module (
        bif: modport b_if::<c_pkg::a>::mp,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package a_proto_pkg {
        type T;
    }
    package a_pkg::<W: u32> for a_proto_pkg {
        type T = logic<W>;
    }
    package b_pkg {
        gen W: u32 = 1 + 1;
        alias package a = a_pkg::<W>;
    }
    package c_pkg {
        import b_pkg::a::T;
        const C: T = 0 as T;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package ab_pkg {
        type A = logic;
        type B = logic;
    }
    module c_module {
        import ab_pkg::{A, B};
        let _a: A = 0;
        let _b: B = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package ab_pkg {
        type A = logic;
        type B = logic;
    }
    module c_module {
        import ab_pkg::{A};
        let _a: A = 0;
        let _b: B = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));

    let code = r#"
    package abc_pkg {
        type A = logic;
        type B = logic;
    }
    module d_module {
        import abc_pkg::{A, B, C};
        let _a: A = 0;
        let _b: B = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UndefinedIdentifier { .. }
    ));

    let code = r#"
    module ModuleA {
        import $sv::ab_pkg::{A, B};
        let _a: $sv::A = 0;
        let _b: $sv::B = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn referring_before_definition() {
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
        AnalyzerError::ReferringBeforeDefinition { .. }
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
        AnalyzerError::ReferringBeforeDefinition { .. }
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
        AnalyzerError::ReferringBeforeDefinition { .. }
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
        AnalyzerError::ReferringBeforeDefinition { .. }
    ));

    let code = r#"
    module ModuleA {
        assign a = 1;
        var a: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringBeforeDefinition { .. }
    ));

    let code = r#"
    module ModuleA {
        let a: logic = b + 1;
        var b: logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringBeforeDefinition { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: logic = c.x;

        struct StructA {
            x: logic,
        }

        var c: StructA;
        assign c = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringBeforeDefinition { .. }
    ));

    let code = r#"
    module ModuleA #(
        param A: bit<B> = 0,
        param B: u32    = 8
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringBeforeDefinition { .. }
    ));

    let code = r#"
    package A {
        const A: u32  = 1;
        const X: type = logic<A>;

        struct Y {
            x: X,
        }

        const Z: type = Y;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA #(
        param A: u32    = 16,
        param B: bit<A> = 0 ,
    ) {}
    module ModuleB {
        inst u: ModuleA #(
            A: 32,
            B: 1 ,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtoPkg {
        const INFO     : bbool;
        const INFO_TYPE: type ;
    }

    package PackageA::<info: bbool = false, info_type: type = bbool,> for ProtoPkg {
        const INFO     : bbool = info;
        const INFO_TYPE: type  = info_type;
    }

    module ModuleA::<PKG: ProtoPkg> {
        import PKG::*;

        let info: INFO_TYPE = 0;
        let _a  : logic     = info;

        const X: logic = INFO;
    }

    module ModuleB {
        inst u: ModuleA::<PackageA::<>>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        function func_a() -> u32 {
            return 8;
        }
        function func_b() -> u32 {
            return func_a();
        }
    }
    module ModuleA {
        import PkgA::*;
        const A: u32 = func_b();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: u32 = func();
        function func() -> u32 {
            return 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let a : u32 = 8;
        let _b: u32 = func();
        function func() -> u32 {
            return a;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: u32 = func();
        let b : u32 = 0;
        function func() -> u32 {
            return b;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringBeforeDefinition { .. }
    ));

    let code = r#"
    module ModuleA {
        const A: u32 = func();
        function func() -> u32 {
            return 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA #(
        param N: u32 = 4,
    ) {
        let _a: u32 = func();
        function func() -> u32 {
            var a: u32;
            for _i in 0..N {
                a = 0;
            }
            return a;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: u32 = func();
        function func() -> u32 {
            const A: u32 = 0;
            return A;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: u32 = func();

        const W: u32 = 32;
        function func() -> bit<W> {
            return 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::ReferringBeforeDefinition { .. }
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
fn member_access_on_array() {
    let code = r#"
    module ModuleA {
        struct StructA {
            a: logic,
        }
        var x: StructA[10];
        let _y: logic = x.a;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MemberAccessOnArray { .. }
    ));

    let code = r#"
    module ModuleA {
        struct StructA {
            a: logic,
        }
        var x: StructA[10];
        let _y: logic = x[0].a;
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MemberAccessOnArray { .. }))
    );

    let code = r#"
    module ModuleA {
        struct StructA {
            a: logic,
        }
        var x: StructA;
        let _y: logic = x.a;
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MemberAccessOnArray { .. }))
    );

    let code = r#"
    module ModuleA {
        struct StructA {
            a: logic,
        }
        var x: StructA[10, 20];
        let _y: logic = x[0].a;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MemberAccessOnArray { .. }))
    );

    let code = r#"
    module ModuleA {
        struct StructA {
            a: logic,
        }
        var x: StructA[10, 20];
        let _y: logic = x[0][0].a;
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MemberAccessOnArray { .. }))
    );
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
        assign u.a = 0;
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
        assign u.a = 1;
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

    let code = r#"
    interface IfA {
        var a: logic;
        var b: logic;
        var c: logic;
        function get_a() -> logic {
            return a;
        }
        function get_b() -> logic {
            return b;
        }
        function get_c() -> logic {
            return c;
        }
        modport mp_a {
            a    : input ,
            get_a: import,
        }
        modport mp_b {
            b    : input ,
            get_b: import,
            ..same(mp_a)
        }
        modport mp_c {
            c    : input ,
            get_c: import,
            ..same(mp_b)
        }
    }
    module ModuleA (
        if_a: modport IfA::mp_c,
    ) {
        let _a: logic = if_a.get_a();
        let _b: logic = if_a.get_b();
        let _c: logic = if_a.get_c();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package a_proto_pkg {
        struct foo_struct {
            foo: u32,
        }
        const FOO: foo_struct;
    }
    package a_pkg::<V: u32> for a_proto_pkg {
        struct foo_struct {
            foo: u32,
        }
        const FOO: foo_struct = foo_struct'{ foo: V };
    }
    module b_module::<PKG: a_proto_pkg> #(
        const BAR: u32 = FOO.bar, // bar is unknown member
    ) {
        import PKG::*;
    }
    alias package A = a_pkg::<32>;
    alias module  B = b_module::<A>;
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));

    let code = r#"
    proto package a_proto_pkg {
        const WIDTH: u32;
        struct a_struct {
            a: logic<WIDTH>,
        }
    }
    package a_pkg::<W: u32> for a_proto_pkg {
        const WIDTH: u32 = W;
        struct a_struct {
            a: logic<WIDTH>,
        }
    }
    package b_pkg::<PKG: a_proto_pkg> {
        struct b_struct {
            b: PKG::a_struct,
        }
    }
    interface c_interface::<PKG: a_proto_pkg> {
        var c: PKG::a_struct;
        modport mp {
            c: input,
        }
    }
    module c_module {
        alias package A_PKG = a_pkg::<32>;
        var _b: b_pkg::<A_PKG>::b_struct;
        assign _b.b.a = 0;
        inst _c: c_interface::<A_PKG>;
        assign _c.c.a = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if {
        var a: u32;
        function f() -> u32 {
            var a: u32;
            a = 0;
            return a;
        }
        modport master {
            a: output,
        }
        modport slave {
            ..converse(master)
        }
    }
    module b_module (
        aif: modport a_if::slave,
    ) {
        let _b: u32 = aif.a;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {
        var a: u32;
        var b: u32;
        modport mp_a {
            a: input,
        }
        modport mp_b {
            b: input,
        }
        modport mp_ab {
            ..same(mp_a, mp_b)
        }
    }
    module ModuleA (
        ab_if: modport InterfaceA::mp_ab,
    ){
        let _a: u32 = ab_if.a;
        let _b: u32 = ab_if.b;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {
        var a: u32;
        var b: u32;
        modport mp_a {
            a: output,
        }
        modport mp_b {
            b: output,
        }
        modport mp_ab {
            ..converse(mp_a, mp_b)
        }
    }
    module ModuleA (
        ab_if: modport InterfaceA::mp_ab,
    ){
        let _a: u32 = ab_if.a;
        let _b: u32 = ab_if.b;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module a_module {
        var _a: $sv::foo_bar;
        var _b: $sv::foo_bar;
        always_comb {
            _a = $sv::foo_bar'{
                foo: 0,
                bar: 1,
            };

            _b.foo = 0;
            _b.bar = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if {
        var a: logic;
        modport mp_a {
            a: output,
        }
    }
    interface b_if {
        var b: logic;
        modport mp_b {
            b: output,
        }
    }
    interface ab_if {
        mixin a_if;
        mixin b_if;
    }
    module c_module (
        a: modport ab_if::mp_a,
        b: modport ab_if::mp_b,
    ) {
        assign a.a = 0;
        assign b.b = 0;
    }
    module d_module {
        inst ab: ab_if;
        let _a: logic = ab.a;
        let _b: logic = ab.b;
        inst c: c_module (a: ab, b: ab);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if {
        var a: logic;
        modport mp_a {
            a: input,
        }
    }
    interface b_if {
        var b: logic;
        modport mp_b {
            b: input,
        }
    }
    interface ab_if {
        mixin a_if;
        mixin b_if;
        modport mp_ab {
            ..same(mp_a, mp_b)
        }
    }
    module c_module (
        ab: modport ab_if::mp_ab,
    ) {
        let _a: logic = ab.a;
        let _b: logic = ab.b;
    }
    module d_module {
        inst ab: ab_if;
        assign ab.a = 0;
        assign ab.b = 1;
        inst c: c_module(ab: ab);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface a_if {
        var a: logic;
        modport mp_a {
            a: input,
        }
    }
    interface ab_if {
        mixin a_if;
    }
    module c_module {
        inst ab: ab_if;
        assign ab.a = 0;
        assign ab.b = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));

    let code = r#"
    interface ab_if {
        var a: logic;
        var b: logic;
        modport slave_a {
            a: input,
        }
        modport slave_b {
            b: input,
        }
        modport slave {
            ..same(slave_a, slave_b)
        }
        modport master {
            ..converse(slave)
        }
    }
    module c_module (
        ab: modport ab_if::master,
    ) {
        assign ab.a = 0;
        assign ab.b = 0;
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

    let code = r#"
    module ModuleA::<N: u32> {
        function FuncA (
            x: input logic<N>,
        ) -> logic {
            return x[msb];
        }
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
        let _a: u32 = if if 1'b0 ? 1'b0 : 1'b1 ? 4 : 5;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnenclosedInnerIfExpression { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = if (if 1'b1 ? 1'b0 : 1'b1) ? 4 : 5;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: u32 = if 1'b1 ? if 1'b0 ? 3 : 4 : 5;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnenclosedInnerIfExpression { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = if 1'b1 ? (if 1'b0 ? 3 : 4) : 5;
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

    let code = r#"
    interface IfA {
        var a: logic;
        modport mp {
            ..input
        }
    }
    interface IfB {
        var b: logic;
        modport mp {
            ..output
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
            if true {
                let c: logic = 1;
                a = c;
            } else {
                a = 0;
            }
            if true {
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
            for i in 0..1 {
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

    let code = r#"
    module ModuleA {
        var a: logic<4, 4>;
        for i in 0..4: g {
            always_comb {
                a[i] = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA #(
        param A: type = logic,
    ) (
        o: output A,
    ) {
        assign o = '0;
    }

    module ModuleB {
        struct StructA {
            x: logic,
            y: logic,
        }

        var a: StructA;

        inst u0: ModuleA #(
            A: StructA,
        ) (
            o: a,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        struct StructA {
            x: logic,
            y: logic,
        }

        var a: StructA;

        inst u: $sv::SvModule (
            a: a,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mst {
            a: output,
        }
    }

    module ModuleA {
        inst a: InterfaceA;

        inst u: $sv::SvModule (
            a: a,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PackageA {
        struct StructA {
            x: logic,
            y: logic,
        }
    }

    interface InterfaceA {
        var a: logic            ;
        var b: PackageA::StructA;
        modport mst {
            a: output,
            b: output,
        }
    }

    module ModuleA {
        inst a: InterfaceA;

        inst u: ModuleB (
            p: a,
        );
    }

    module ModuleB (
        p: modport InterfaceA::mst,
    ) {
        assign p.a = 0;
        assign p.b = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_sel: input logic<2>,
    ){
        var a: logic<4>;

        always_ff {
            a[i_sel] = 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package foo_proto_pkg {
        const FOO: bbool;
    }

    module bar_module::<PKG: foo_proto_pkg> {
        import PKG::*;
        var bar: u32;
        if FOO :g {
            assign bar = 0;
        } else {
            assign bar = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<X: u32> {
        var a: logic;
        if X == 0 :g {
            assign a = 0;
        } else {
            assign a = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<X: u32 = 0> {
        var a: logic;
        if X == 0 :g {
        } else {
            assign a = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module ModuleA (
        i_clk: input  clock,
        i_rst: input  reset,
    ) {
        let a: logic = 1;
        var b: logic;

        function f() -> logic {
            return a;
        }

        always_ff {
            if f() {
                b = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface IfA {
        var a: logic;
        function get_a() -> logic {
            return a;
        }
    }
    module ModuleA (
        i_clk: input clock,
    ) {
        inst if_a: IfA;
        assign if_a.a = '1;

        var d: logic;
        always_ff {
            if if_a.get_a() {
                d = '1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA #(
        param N: u32 = 4
    ) {
        initial {
            func();
        }
        function func() {
            const D: u32 = $clog2(N);
            var a: u32;
            for i in 0..D {
                a = i;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    function func::<N: p32> {
        const DEPTH: u32 = $clog2(N);
        var n: u32;
        for i in 0..DEPTH {
            n = i;
        }
    }
    module ModuleA {
        const ENTRIES: u32 = 2;
        always_comb {
            func::<ENTRIES>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<8> [4];
        always_comb {
            for i in 0..4 {
                a[i] = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<8> [4];
        always_comb {
            for i in 0..3 {
                a[i] = 0;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnassignVariable { .. }))
    );

    // Partial-slice assignment whose unassigned bits are never read is OK.
    let code = r#"
    module ModuleA (
        i_clk : input  clock     ,
        i_rst : input  reset     ,
        i_addr: input  logic <48>,
        o_addr: output logic <48>,
    ) {
        var r_addr: logic<48>;

        always_ff (i_clk, i_rst) {
            if_reset {
                r_addr[47:3] = '0;
            } else {
                r_addr[47:3] = i_addr[47:3] + 1;
            }
        }

        assign o_addr = {r_addr[47:3], 3'b0};
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // But reading an unassigned bit-slice still fires the warning.
    let code = r#"
    module ModuleA (
        i_clk : input  clock     ,
        i_rst : input  reset     ,
        i_addr: input  logic <48>,
        o_addr: output logic <48>,
    ) {
        var r_addr: logic<48>;

        always_ff (i_clk, i_rst) {
            if_reset {
                r_addr[47:3] = '0;
            } else {
                r_addr[47:3] = i_addr[47:3] + 1;
            }
        }

        assign o_addr = {r_addr[47:3], r_addr[2:0]};
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnassignVariable { .. }))
    );

    // Instance output driving an unpacked-array range slice must mark each
    // covered element as assigned.
    let code = r#"
    module Inner (
        i_data: input  logic<8>   ,
        o_data: output logic<8> [2],
    ) {
        assign o_data[0] = i_data;
        assign o_data[1] = i_data;
    }
    module ModuleA (
        i_data: input  logic<8>,
        o_data: output logic<8>,
    ) {
        var w_arr: logic<8> [4];
        for n in 0..2 :g {
            inst u_inner: Inner (
                i_data: i_data        ,
                o_data: w_arr[2 * n+:2],
            );
        }
        assign o_data = w_arr[0] ^ w_arr[1] ^ w_arr[2] ^ w_arr[3];
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // `inout (tri logic)` pads are driven externally; no internal assignment needed.
    let code = r#"
    module ModuleA (
        io_pad: inout  tri logic<4>,
        o_data: output     logic<4>,
    ) {
        assign o_data = io_pad;
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

    let code = r#"
    module ModuleA (
        o: output logic,
    ) {
        var y: logic<2>;
        inst u: $sv::SvModule (
            x: y[0],
        );
        // Read y[1] so the unassigned bit is not treated as dead.
        assign o = y[1];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnassignVariable { .. }));

    let code = r#"
    module ModuleA {
        var y: logic;
        var z: logic;
        inst u: $sv::SvModule (
            x: {y, z},
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA (
        o_out: output logic,
    ) {
        assign o_out = 0;
    }

    module Top {
        inst foo_if: $sv::foo_if;
        inst u_a: ModuleA (o_out: foo_if.value);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn combinational_loop_oversized_array_underdetect() {
    // Regression: a dynamic-index write to a very large array. The dynamic
    // index makes `compute_assign_target` yield arr_idx = None, which routes
    // through `comb_loop_detect`'s per-element expansion — O(elements) and a
    // hang without the `OVERSIZED_ARRAY` guard. Analysis must finish and report
    // no combinational loop.
    let code = r#"
    module ModuleA (
        clk:  input  clock,
        rst:  input  reset,
        idx:  input  logic<23>,
        wd:   input  logic<32>,
        rd:   output logic<32>,
    ) {
        var mem: logic<32> [8388608];
        always_ff {
            if_reset {
                mem[idx][7:0] = 0;
            } else {
                mem[idx][7:0]   = wd[7:0];
                mem[idx][15:8]  = wd[15:8];
                mem[idx][23:16] = wd[23:16];
                mem[idx][31:24] = wd[31:24];
            }
        }
        assign rd = mem[idx];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "oversized dynamic-index array must not be flagged as a comb loop",
    );
}

#[test]
fn combinational_loop() {
    // 2-block ring: assign b = c + a; assign c = b + 1
    let code = r#"
    module ModuleA (
        a: input  logic<8>,
        b: output logic<8>,
        c: output logic<8>,
    ) {
        assign b = c + a;
        assign c = b + 1;
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::CombinationalLoop { .. }));

    // FF-broken feedback: assign x = y, always_ff y <= x. No loop.
    let code = r#"
    module ModuleA (
        clk: input  clock,
        a:   input  logic<8>,
        b:   output logic<8>,
    ) {
        var y: logic<8>;
        assign b = y;
        always_ff (clk) {
            y = a;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());

    // Function call: caller-side feedthrough links read x -> write x.
    let code = r#"
    module ModuleA (
        a: input  logic<8>,
        b: output logic<8>,
    ) {
        function ident (
            x: input logic<8>,
        ) -> logic<8> {
            return x;
        }

        var c: logic<8>;
        assign b = ident(c);
        assign c = b;
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::CombinationalLoop { .. }));

    // Disjoint partial-write self-reference: `a[1] = a[0]` is not a loop
    // because the read bit and write bit don't overlap.
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

    // Block-local variables (declared inside `always_comb`) do not create
    // combinational loops under Veryl's blocking semantics.
    let code = r#"
    module ModuleA (
        a: input  logic<32>,
        b: output logic<32>,
    ) {
        always_comb {
            var c: logic<32>;
            var d: logic<32>;
            c = a;
            d = 2 * c;
            c = d;
            b = c;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());

    // Module instance feedthrough: child has `assign out = in;`. Parent
    // closes the loop with `assign x = y`. Should detect.
    let code = r#"
    module Buf (
        i: input  logic<8>,
        o: output logic<8>,
    ) {
        assign o = i;
    }

    module Top (
        a: input  logic<8>,
        b: output logic<8>,
    ) {
        var x: logic<8>;
        var y: logic<8>;
        inst u: Buf (
            i: x,
            o: y,
        );
        assign x = y;
        assign b = y;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Module instance with FF-driven output: no loop should be detected.
    let code = r#"
    module Reg (
        clk: input  clock,
        i:   input  logic<8>,
        o:   output logic<8>,
    ) {
        always_ff (clk) {
            o = i;
        }
    }

    module Top (
        clk: input  clock,
        a:   input  logic<8>,
        b:   output logic<8>,
    ) {
        var x: logic<8>;
        var y: logic<8>;
        inst u: Reg (
            clk: clk,
            i: x,
            o: y,
        );
        assign x = y;
        assign b = y;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Continuous-assign self-reference: real combinational loop.
    let code = r#"
    module ModuleA (
        a: output logic<8>,
    ) {
        assign a = a + 1;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Conditional self-reference: one branch reads x before assigning,
    // synthesizing to `x = cond ? a : (b + x)` which closes the loop.
    let code = r#"
    module ModuleA (
        cond: input  logic,
        a:    input  logic<8>,
        b:    input  logic<8>,
        x:    output logic<8>,
    ) {
        always_comb {
            if cond {
                x = a;
            } else {
                x = b + x;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Procedural overwrite within always_comb is NOT a loop: the first
    // assignment dominates the second statement's read of `a`.
    let code = r#"
    module ModuleA (
        a: output logic<8>,
    ) {
        always_comb {
            a = 0;
            a = a + 1;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Both branches assign x with no self-read. Not a loop.
    let code = r#"
    module ModuleA (
        cond: input  logic,
        a:    input  logic<8>,
        b:    input  logic<8>,
        x:    output logic<8>,
    ) {
        always_comb {
            if cond {
                x = a;
            } else {
                x = b;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Pre-assign before conditional self-reference is dominating and
    // thus NOT a loop. `x = 0` covers all bits before the if/else.
    let code = r#"
    module ModuleA (
        cond: input  logic,
        b:    input  logic<8>,
        x:    output logic<8>,
    ) {
        always_comb {
            x = 0;
            if cond {
                x = b;
            } else {
                x = b + x;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Bit-precise NodeKey distinguishes ca[0]/ca[1]/ca[2] so the
    // forward chain through bit-select inst outputs is not a loop
    // (regression from perlindgren/vips).
    let code = r#"
    module FullAdder (
        a    : input  logic,
        b    : input  logic,
        c    : input  logic,
        sum  : output logic,
        carry: output logic,
    ) {
        assign sum   = a ^ b ^ c;
        assign carry = (a & b) | (c & (a ^ b));
    }

    module Arith (
        a  : input  logic<2>,
        b  : input  logic<2>,
        sub: input  logic   ,
        r  : output logic<2>,
        c  : output logic   ,
    ) {
        var ca: logic<3>;
        assign ca[0] = sub;
        assign c     = ca[2];

        var cl_0: logic;
        var cl_1: logic;
        assign cl_0 = ca[0];
        assign cl_1 = ca[1];

        inst u0: FullAdder (
            a    : a[0]      ,
            b    : b[0] ^ sub,
            c    : cl_0      ,
            sum  : r[0]      ,
            carry: ca[1]     ,
        );
        inst u1: FullAdder (
            a    : a[1]      ,
            b    : b[1] ^ sub,
            c    : cl_1      ,
            sum  : r[1]      ,
            carry: ca[2]     ,
        );
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Forward array chain c[i] -> c[i+1] is not a loop -- requires
    // per-element index resolution (regression from celox/linear_sorter).
    let code = r#"
    module Buf (
        i: input  logic<8>,
        o: output logic<8>,
    ) {
        assign o = i;
    }

    module Top (
        d_in:  input  logic<8>,
        d_out: output logic<8>,
    ) {
        var c: logic<8> [3];
        assign c[0] = d_in;
        for i in 0..2 :cell {
            inst u: Buf (
                i: c[i],
                o: c[i + 1],
            );
        }
        assign d_out = c[2];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Bit-disjoint feedback through `x`: x = a[1], a[0] = x. Bit
    // precision distinguishes bit 0 from bit 1, so not a loop.
    let code = r#"
    module Top (
        b_in: input  logic,
        a:    output logic<2>,
    ) {
        var x: logic;
        var y: logic;
        assign a[0] = x;
        assign a[1] = y;
        assign x    = a[1];
        assign y    = b_in;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );

    // Dst-side bit-disjoint writes: `t1[0] = 0; t1[1] = t3;`. Reads of
    // t3 must edge only to t1[1], not t1[0], so no `t1[0]→t2→t3→t1[1]`
    // false cycle through inst feedthrough.
    let code = r#"
    module ModuleAOk2 (
        a: input  logic,
        b: input  logic,
        x: output logic,
    ) {
        always_comb {
            x = a & ~b;
        }
    }

    module ModuleBOk2 (
        a: input  logic,
        y: output logic,
    ) {
        always_comb {
            y = a;
        }
    }

    module ModuleCOk2 (
        a: input  logic,
        y: output logic,
    ) {
        var t1: logic<2>;
        var t2: logic;
        var t3: logic;

        inst mb: ModuleBOk2 (
            a: t1[0],
            y: t2,
        );

        inst ma: ModuleAOk2 (
            a: a,
            b: t2,
            x: t3,
        );

        always_comb {
            t1[0] = 0;
            t1[1] = t3;
            y     = t3;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "false positive: {errors:?}"
    );

    // Src-side bit-disjoint reads: `b = a[0]; c = a[1];` in one decl.
    // The other decl writes `a[1] = b`; per-decl aggregation would
    // incorrectly tie b's flow to a@bit1, closing a false cycle.
    let code = r#"
    module ModuleA (
        d:   input  logic<2>,
        out: output logic,
    ) {
        var a: logic<2>;
        var b: logic;
        var c: logic;

        always_comb {
            b = a[0];
            c = a[1];
        }

        always_comb {
            a[0] = d[0];
            a[1] = b;
        }

        assign out = c;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "source-side false positive: {errors:?}"
    );

    // False-positive cycle through unrelated assigns in the same comb block.
    // `op1_fp32 = op1; op2_fp32 = op2;` are independent.
    // All writes in the same reader_decl are collected as destinations,
    // incorrectly linking op1_fp32 and op2_fp32 and forming a spurious cycle.
    let code = r#"
    module FPComp (
        op1: input logic<32>,
        op2: input logic<32>,

        less_than: output logic,
    ) {
        struct FP32 {
            sign: logic    ,
            exp : logic<8> ,
            frac: logic<23>,
        }

        var op1_fp32: FP32;
        var op2_fp32: FP32;

        always_comb {
            op1_fp32 = op1;
            op2_fp32 = op2;

            if (op1_fp32.exp == op2_fp32.exp) {
                less_than = op1_fp32.frac <: op2_fp32.frac;
            } else {
                less_than = op1_fp32.exp <: op2_fp32.exp;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn comb_loop_ifstmt_feed_forward_array() {
    // False positive: an always_comb for-loop over cross-coupled arrays that
    // reads index i and writes i+1 (a feed-forward / acyclic CORDIC-style
    // chain) written with an `if`/`else` STATEMENT was wrongly rejected. The
    // condition read `yw[i]` (no assign_target) was wired to every same-array
    // write, forming a false cross-index cycle; it is dominated by the earlier
    // write to `yw[i]`, so the undominated filter now drops it. The
    // byte-identical ternary form was already accepted.
    let if_stmt = r#"
    module Chain (
        x0: input  signed logic<16>,
        y0: input  signed logic<16>,
        xo: output signed logic<16>,
        yo: output signed logic<16>,
    ) {
        var xw: signed logic<16> [5];
        var yw: signed logic<16> [5];
        always_comb {
            xw[0] = x0;
            yw[0] = y0;
            for i in 0..4 {
                if yw[i] <: 0 {
                    xw[i + 1] = xw[i] - (yw[i] >>> i);
                    yw[i + 1] = yw[i] + (xw[i] >>> i);
                } else {
                    xw[i + 1] = xw[i] + (yw[i] >>> i);
                    yw[i + 1] = yw[i] - (xw[i] >>> i);
                }
            }
            xo = xw[4];
            yo = yw[4];
        }
    }
    "#;
    let errors = analyze(if_stmt);
    assert!(
        errors.is_empty(),
        "feed-forward array chain (if-statement form) must not be a comb loop: {errors:?}"
    );

    // Guard against over-correcting: a genuine condition-driven loop where the
    // condition read is UNDOMINATED (`a` is written only under `if b`, `b` only
    // under `if a`) must still be rejected.
    let real_cond_loop = r#"
    module RealLoop (
        o: output logic,
    ) {
        var a: logic;
        var b: logic;
        always_comb {
            if a {
                b = 1;
            } else {
                b = 0;
            }
            if b {
                a = 1;
            } else {
                a = 0;
            }
            o = a;
        }
    }
    "#;
    let errors = analyze(real_cond_loop);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "a real condition-driven loop must still be detected: {errors:?}"
    );

    // And a real array loop `m[0] = m[1]; m[1] = m[0]` must still be rejected.
    let real_array_loop = r#"
    module RealArray (
        o: output logic<8>,
    ) {
        var m: logic<8> [2];
        always_comb {
            m[0] = m[1];
            m[1] = m[0];
            o = m[0];
        }
    }
    "#;
    let errors = analyze(real_array_loop);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "a real array comb loop must still be detected: {errors:?}"
    );
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

    let code = r#"
    module ModuleA {
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

    let code = r#"
    module ModuleA {
        var a: logic<2>;
        var b: logic;

        always_comb {
            if b {
                a[0] = 1;
            } else {
                a[1] = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UncoveredBranch { .. }));

    let code = r#"
    module ModuleA {
        var a: logic<2>;

        always_comb {
            if true {
                a[0] = 1;
                a[1] = 1;
            } else {
                a[0] = 1;
                a[1] = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

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

    let code = r#"
    module ModuleA {
        var a: logic<2>;

        always_comb {
            if true {
                a[0] = 1;
            } else {
                a[0] = 1;
            }
            if true {
                a[1] = 1;
            } else {
                a[1] = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic;

        always_comb {
            a = 0;

            if true {
                a = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic;

        always_comb {
            a = 0;

            if true {
            } else if true {
            } else {
                a = 1;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic;

        initial {
            if true {
                $readmemh("a.hex", a);
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    package PkgA::<T: type> {
        const TYPE: type = T;
    }
    module ModuleB::<T: type> (
        b: output T,
    ) {
        assign b = 0 as T;
    }
    module ModuleC {
        import PkgA::<bbool>::*;
        inst u: ModuleB::<TYPE> (
            b: _,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA #(
        param A: bit<_> = 0,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA #(
        param A: bit[_] = '{0}
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA #(
        const A: bit<_> = 0,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA #(
        const A: bit[_] = '{0},
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA (
        a: input logic<_>,
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA (
        a: input logic[_],
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        var _a: logic<_>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        var _a: logic[_];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: logic<_> = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: logic[_] = '{0};
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        type A = logic<_>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        type A = logic[_];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        struct A {
            a: logic<_>,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        enum A: logic<_, 1> {
            A,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));

    let code = r#"
    module ModuleA {
        function F() -> logic<_> {
            return 0;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::AnonymousIdentifierUsage { .. }
    ));
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
fn unevaluable_value_reset_value() {
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
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

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

    let code = r#"
    module ModuleA::<A: u32> (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        const B: u32 = A + 1;
        var _a: logic<B>;
        always_ff {
            if_reset {
                _a = 0 as B;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package Pkg {
        const W: u32 = 8;
    }
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        import Pkg::*;
        var _d: logic<W>;
        always_ff {
            if_reset {
                _d = 0 as W;
            } else {
                _d = 1 as W;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn unevaluable_value_const_value() {
    let code = r#"
    module ModuleA (
        a: input logic,
    ) {
        const x: logic = a;
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

    let code = r#"
    module ModuleA {
        struct StructA {
            X: u32,
            Y: u32,
        }
        const x: StructA = StructA'{
            X: 1,
            Y: 2,
        };
        const y: logic = x.X + x.Y;
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PackageA {
        struct StructA {
            X: u32,
            Y: u32,
        }
        function FuncA -> StructA {
            var ret: StructA;
            ret.X = 1;
            ret.Y = 2;
            return ret;
        }
    }
    module ModuleA {
        const x: PackageA::StructA = PackageA::FuncA();
        const y: logic = x.X;
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const y: logic = $sv::func();
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const A: $sv::a_struct = $sv::a_struct'{ a: 1 };
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const A: bit<5>[1] = '{$sv::FOO};
        const B: bit<5>[1] = A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const A: u32[1] = '{ 1 + 1 };
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto package ProtPkg {
        function func() -> u32;
    }
    package Pkg::<n: u32> for ProtPkg {
        function func() -> u32 {
            return n;
        }
    }
    module ModuleA::<pkg: ProtPkg> {
        const A: u32 = pkg::func();
    }
    module ModuleB {
        inst u: ModuleA::<Pkg::<32>>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package a_pkg {
        const A: bit<4> = {2'b00, 2'b00} as 4;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn invalid_operand() {
    let code = r#"
    module ModuleA {
        const a: logic = +logic;
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidOperand { .. }));

    let code = r#"
    module ModuleA {
        const A: type = logic + logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidOperand { .. }));

    let code = r#"
    module ModuleA {
        const A: type = {logic};
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidOperand { .. }));

    let code = r#"
    module ModuleA {
        let _a: logic = logic + logic;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidOperand { .. }));

    let code = r#"
    module ModuleA {
        function func::<N: u32> {
            gen W: u32 = N;
            var a: u32;
            a = 0 as W;
        }
        always_comb {
            func::<8>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA::<N: u32> {
        const W: u32 = $clog2(N);
        function func() -> bit<W> {
            var n: bit<W>;
            n = N as W;
            return N as W;
        }
    }
    module ModuleB::<N: u32> {
        const B: bit<PkgA::<N>::W> = PkgA::<N>::func();
    }
    module ModuleC {
        inst u: ModuleB::<4>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        const WIDTH: u32 = 8;
        const SIZE : u32 = 4;
        const ARRAY: bit<WIDTH> [SIZE] = '{8'h11, 8'h22, 8'h33, 8'h44};
        function f() -> logic<WIDTH> {
            var result: logic<WIDTH>;
            result = '0;
            for i in 0..SIZE {
                result = result + ARRAY[i];
            }
            return result;
        }
    }
    module ModuleA (
        o_d: output logic<8>,
    ) {
        assign o_d = PkgA::f();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn invalid_logical_operand() {
    let code = r#"
    module ModuleA {
        let _a: logic = 1 && 1;
    }"#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidLogicalOperand { .. }
    ));

    let code = r#"
    module ModuleA {
        let a: logic<2> = 1;
        let _b: logic = true && a;
    }"#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::InvalidLogicalOperand { .. }
    ));

    let code = r#"
    module ModuleA {
        let a: logic<2> = 1;
        let _b: logic = true && a[0];
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<A: u32> {
        var a: logic<A>;

        always_comb {
            if a[0] {}
        }
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        struct StructA {
            x: logic,
            y: logic,
        }
        var a: StructA;
        assign a = 0;

        always_comb {
            if a[0] {}
            if a.x {}
        }
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic   ;
        var b: logic<2>;

        always_comb {
            a = 0;
            b = 0;
            if b[a] {}
        }
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        var a: logic<2>;
        always_comb {
            a = if 1'b1 | 1'b1 ? 0 : 0;
        }

    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const X : u32      = $sv::pkg::X;
        let a : logic<2> = 1;
        let _b: logic    = a[X - 1] && 1'b1;
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // `inside` comparing an enum reached through a $sv-imported interface
    // must yield a 1-bit result (not the enum's width).
    let code = r#"
    package Pkg {
        enum op_t: logic<3> {
            A = 3'd0,
            B = 3'd1,
            C = 3'd2,
        }
        struct payload_t {
            val: logic<32>,
            op : op_t     ,
        }
    }
    module ModuleA (
        o_drop: output logic,
    ) {
        inst fo_if: $sv::Bus #( PAYLOAD: Pkg::payload_t );
        always_comb {
            fo_if.valid   = '0;
            fo_if.payload = '0;
        }
        always_comb {
            o_drop = inside fo_if.payload.op {Pkg::op_t::A, Pkg::op_t::B, Pkg::op_t::C};
        }
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // Bit-select on a `param` must narrow the type to 1 bit.
    let code = r#"
    module ModuleA #(
        param USE_PIPE: u32 = 1'b1,
    ) (
        i_data: input  logic<32>,
        o_data: output logic<32>,
    ) {
        assign o_data = if USE_PIPE[0] ? i_data : '0;
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // Same kind-inheritance bug triggered via `if (enum == enum)` condition.
    let code = r#"
    package Pkg {
        enum op_t: logic<3> {
            A = 3'd0,
            B = 3'd1,
        }
        struct payload_t {
            val: logic<32>,
            op : op_t     ,
        }
    }
    module ModuleA (
        o_st: output logic<2>,
    ) {
        inst control_if: $sv::Bus #( PAYLOAD: Pkg::payload_t );
        always_comb {
            control_if.valid   = '0;
            control_if.payload = '0;
        }
        always_comb {
            if control_if.payload.op == Pkg::op_t::A {
                o_st = 2'h1;
            } else {
                o_st = 2'h2;
            }
        }
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn invalid_factor() {
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

    let code = r#"
    module ModuleA {
        let a: logic = ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidFactor { .. }));

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
}

#[test]
fn invalid_factor_kind() {
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
        param T: type = logic,
    ) {}
    module ModuleB::<t: type> {
        inst u: ModuleA #(T: t);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

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
        assign x.a = 0;
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

    let code = r#"
    interface foo_if {
        var ready  : logic;
        var valid  : logic;
        modport master {
            ready: input ,
            valid: output,
        }
        modport slave {
            ..converse(master)
        }
    }
    module c_module {
        inst u: b_module;
    }
    module b_module {
        inst a_if: foo_if;
        inst b_if: foo_if;
        always_comb {
            a_if.master <> b_if.slave;
        }
        assign a_if.ready = 1;
        assign b_if.valid = 1;
    }
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

    let code = r#"
    module ModuleA {
        var a: logic;

        assign a = $sv::pkg::func() + 1;
    }"#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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
fn unevaluable_value_case_condition() {
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
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

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
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

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
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

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
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));
}

#[test]
fn empty_exclusive_range_rejected() {
    // An exclusive `lo..hi` with constant `lo >= hi` is empty; in particular the
    // `..0` case (e.g. a `param N: u32 = 0` upper bound) made the emitter's
    // `(hi)-1` underflow to a near-universal range. Reject the empty range.
    for body in ["2..N", "2..0", "5..3"] {
        let code = format!(
            r#"
            module ModuleA #(
                param N: u32 = 0,
            ) (
                sel: input  logic<8>,
                out: output logic,
            ) {{
                assign out = inside sel {{{body}}};
            }}
            "#
        );

        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidRange { .. })),
            "{body}: {errors:?}"
        );
    }

    // Non-empty exclusive ranges and inclusive ranges must NOT be flagged.
    for body in ["2..10", "0..10", "1..=10", "0..=0"] {
        let code = format!(
            r#"
            module ModuleB (
                sel: input  logic<8>,
                out: output logic,
            ) {{
                assign out = inside sel {{{body}}};
            }}
            "#
        );

        let errors = analyze(&code);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidRange { .. })),
            "{body}: {errors:?}"
        );
    }
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
fn tb_component_outside_test() {
    let code = r#"
    module ModuleA {
        inst clk: $tb::clock_gen;
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidTbUsage { .. }));

    // Inside a test module should be fine
    let code = r#"
    #[test(test_mod)]
    module test_mod {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        initial {
            rst.assert();
            clk.next(10);
            $finish();
        }
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(errors.is_empty());
}

#[test]
fn missing_tb_port() {
    let code = r#"
    #[test(test_mod)]
    module test_mod {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen;

        initial {
            rst.assert();
            $finish();
        }
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MissingTbPort { .. }))
    );
}

#[test]
fn unknown_tb_port() {
    let code = r#"
    #[test(test_mod)]
    module test_mod {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk: clk, foo: clk);

        initial {
            rst.assert();
            $finish();
        }
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnknownTbPort { .. }))
    );

    let code = r#"
    #[test(test_mod)]
    module test_mod {
        inst clk: $tb::clock_gen(foo: 0);

        initial {
            clk.next(1);
            $finish();
        }
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnknownTbPort { .. }))
    );
}

#[test]
fn out_of_range_select_in_dead_branch() {
    // A dead-branch out-of-range select must not be reported, even when the
    // guard mixes a constant-decisive term (`i >: 3`) with a runtime one: for
    // i >= 4 the `else` is dead and the short-circuit fold drops it.
    let code = r#"
    module ModuleA (
        i_sel: input  logic    ,
        i_d:   input  logic<80>,
        o:     output logic<17>,
    ) {
        var w: logic<17> [8];
        always_comb {
            for i in 0..8 {
                if i >: 3 || i_sel {
                    w[i] = 0;
                } else {
                    w[i] = i_d[17 * i+:17];
                }
            }
        }
        assign o = w[0];
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvalidSelect { .. }))
    );

    // A reachable out-of-range select (guarded only by a runtime signal) is
    // still reported.
    let code = r#"
    module ModuleA (
        i_sel: input  logic    ,
        i_d:   input  logic<80>,
        o:     output logic    ,
    ) {
        var w: logic;
        always_comb {
            w = 0;
            if i_sel {
                w = i_d[135];
            }
        }
        assign o = w;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvalidSelect { .. }))
    );
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

    let code = r#"
    module ModuleA {
        let _a: u32[4] = '{0, 1, 2, 3};
        let _b: u32[2] = _a[0:1];
        let _c: u32[2] = _a[0+:2];
        let _d: u32[2] = _a[3-:2];
        let _e: u32[2] = _a[1 step 2];
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // Assignment LHS with a wrong-order (descending) array range must be
    // rejected, not silently dropped (#9).
    let code = r#"
    module ModuleA (
        o: output logic<8> [4],
    ) {
        assign o[2:0] = '{8'd0, 8'd0, 8'd0};
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvalidSelect { .. }))
    );

    // Valid array-range assignment LHS expands element-wise, so every covered
    // element is driven (no false unassign) and overlaps are still caught (#9).
    let code = r#"
    module ModuleA (
        o: output logic<8> [4],
    ) {
        assign o[0+:2] = '{8'd0, 8'd0};
        assign o[2+:2] = '{8'd1, 8'd1};
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // Multi-dim follow-up: an element with multi-dim-packed width is expanded
    // element-wise too (each covered element gets its scalar literal).
    let code = r#"
    module ModuleA (
        o: output logic<10, 10> [4],
    ) {
        assign o[0+:2] = '{100, 200};
        assign o[2+:2] = '{300, 400};
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // Multi-dim follow-up: a range over the outer dim of a 2-D unpacked array
    // expands the nested literal per outer element (no false unassign).
    let code = r#"
    module ModuleA (
        o: output logic<8> [2, 2],
    ) {
        assign o[0+:2] = '{'{8'd1, 8'd2}, '{8'd3, 8'd4}};
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // A range-LHS literal whose item count differs from the slice width must be
    // reported (else it is silently dropped and illegal SystemVerilog is emitted).
    for (lhs, rhs) in [
        ("o[0+:2]", "'{8'd1, 8'd2, 8'd3}"),
        ("o[0+:3]", "'{8'd1, 8'd2}"),
    ] {
        let code = format!(
            r#"
    module ModuleA (
        o: output logic<8> [4],
    ) {{
        assign {lhs} = {rhs};
    }}
    "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|x| matches!(x, AnalyzerError::MismatchType { .. })),
            "{lhs} = {rhs}"
        );
    }

    // A range-LHS that can't lower to fixed element indices — dynamic base/bound,
    // out-of-range, or reversed — must be rejected, not silently lowered (which
    // miscompiles the simulator while the emitter keeps the original slice).
    for lhs in [
        "o[n+:2]", "o[0+:n]", "o[n:0]", "o[3+:2]", "o[1:0]", "o[1-:3]",
    ] {
        let code = format!(
            r#"
    module ModuleA (
        n: input  logic<2>,
        o: output logic<8> [4],
    ) {{
        assign {lhs} = '{{8'd1, 8'd2}};
    }}
    "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|x| matches!(x, AnalyzerError::InvalidRangeAssign { .. })),
            "{lhs}"
        );
    }

    // An op-assign (`+=` etc.) to an array range slice has no valid lowering and
    // emits illegal SystemVerilog, so it must be rejected, not silently dropped.
    let code = r#"
    module ModuleA (
        o: output logic<8> [4],
    ) {
        always_comb {
            o = '{8'd1, 8'd2, 8'd3, 8'd4};
            o[0+:2] += '{8'd10, 8'd20};
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvalidRangeAssign { .. }))
    );

    // A descending `-:` slice that reaches index 0 is a valid, count-matching
    // constant range (regression guard for the `-:` low-bound off-by-one).
    let code = r#"
    module ModuleA (
        o: output logic<8> [4],
    ) {
        assign o[3-:4] = '{8'd1, 8'd2, 8'd3, 8'd4};
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let _a: logic<2> = 1;
        let _b: logic<2> = _a[0][0];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));

    let code = r#"
    module ModuleA {
        let _a: logic[2] = '{1, 1};
        let _b: logic[2] = _a[0][0][0];
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));

    let code = r#"
    module module_a {
        let _a: logic<32> = 0;
        let _b: logic<16> = _a[1 step 16];
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module module_a {
        enum EnumA {
            X,
            Y,
        }
        let _a: EnumA = 0;
        let _b: logic = _a[0];
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        enum a_enum: logic {
            FOO,
            BAR,
        }
        struct b_struct {
            e: a_enum,
            d: a_enum,
            c: logic<32>,
            b: logic<32>,
            a: logic<32>,
        }
        let _a: b_struct = b_struct'{
            a: '0,
            b: '0,
            c: '0,
            d: a_enum::FOO,
            e: a_enum::BAR,
        };
        let _b: logic = inside _a.e { a_enum::BAR };
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // A `+:`/`-:`/`step` part-select with a runtime width is unsynthesizable
    // (SV requires a constant width) and must be rejected.
    let code = r#"
    module module_a (
        i_n: input  logic<3>,
        i_a: input  logic<8>,
        o  : output logic<8>,
    ) {
        assign o = i_a[0+:i_n];
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. }))
    );

    // A constant width (literal or parameter) is fine even with a runtime base.
    let code = r#"
    module module_a (
        i_idx: input  logic<4>,
        i_a  : input  logic<8>,
        o    : output logic<4>,
    ) {
        assign o = i_a[i_idx+:4];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. }))
    );

    let code = r#"
    package a_pkg::<A_VALUE: u32> {
        const A: u32 = A_VALUE;
    }
    package b_pkg {
        alias package a = a_pkg::<2>;
    }
    interface c_if {
        var c: logic;
        modport mp {
            c: output,
        }
    }
    module e_module {
        const N: u32 = b_pkg::a::A;
        inst c: c_if[N];
        connect c[0].mp <> 0;
        connect c[1].mp <> 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn multiple_driver_detection_is_order_independent() {
    // Regression: one dynamic-index assign made the accumulated entry's
    // `maybe` sticky, suppressing later definite double-drive conflicts on
    // the same variable when it appeared first.
    for order in [
        ["assign x[i] = 1;", "assign x[0] = 2;", "assign x[0] = 3;"],
        ["assign x[0] = 2;", "assign x[0] = 3;", "assign x[i] = 1;"],
        ["assign x[0] = 2;", "assign x[i] = 1;", "assign x[0] = 3;"],
    ] {
        let body = order.join("\n        ");
        let code = format!(
            r#"
            module Top (i: input logic, o: output logic<8>) {{
                var x: logic<8> [2];
                {body}
                assign o = x[0] + x[1];
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
            "{body}: {errors:?}"
        );
    }

    // Dynamic + single definite per element stays accepted.
    let code = r#"
    module Top (i: input logic, o: output logic<8>) {
        var x: logic<8> [2];
        assign x[i] = 1;
        assign o = x[0] + x[1];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
        "{errors:?}"
    );
}

#[test]
fn anonymous_identifier_in_type_position_rejected() {
    // Regression: `var v: _;` passed check/build and emitted a declaration
    // with an empty type keyword (invalid SystemVerilog).
    let code = r#"
    module Top (
        i_a: input  logic,
        o_x: output logic,
    ) {
        var v: _;
        always_comb {
            v   = i_a;
            o_x = v;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::AnonymousIdentifierUsage { .. })),
        "{errors:?}"
    );
}

#[test]
fn zero_width_literal_rejected() {
    // Regression: `0'1` emitted `0'b` (zero size, empty digits) and `0'h0`
    // passed the too-large check (0 bits is not > width 0) — both illegal
    // SystemVerilog emitted with no diagnostic.
    for lit in ["0'1", "0'h0", "0'b0"] {
        let code = format!(
            r#"
            module Top {{
                var a: logic<8>;
                assign a = {lit} as 8;
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::ZeroWidthNumber { .. })),
            "{lit}: {errors:?}"
        );
    }

    // Normal widths stay accepted.
    let code = r#"
    module Top {
        var a: logic<8>;
        assign a = 1'1 as 8;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::ZeroWidthNumber { .. })),
        "{errors:?}"
    );
}

#[test]
fn attribute_string_args_rejected() {
    // Regression: align/fmt/expand silently skipped non-identifier (string)
    // arguments, and a string top argument of #[test] silently selected the
    // default top module.
    for attr in ["#[align(number, \"junk\")]", "#[fmt(skip, \"junk\")]"] {
        let code = format!(
            r#"
            module Top {{
                {attr}
                var a: logic;
                assign a = 1;
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::MismatchAttributeArgs { .. })),
            "{attr}: {errors:?}"
        );
    }

    // Valid identifier arguments stay accepted.
    let code = r#"
    module Top {
        #[align(number)]
        var a: logic;
        assign a = 1;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchAttributeArgs { .. })),
        "{errors:?}"
    );
}

#[test]
fn non_constant_select_width_on_struct_member() {
    // Regression: the member-access select loops skipped
    // check_part_select_width, so `p.data[0+:i_w]` with a runtime width
    // passed the build and emitted illegal SystemVerilog.
    let code = r#"
    module ModuleA (
        i_w: input  logic<3>,
        i_d: input  logic<8>,
        o_d: output logic<4>,
    ) {
        struct Pair {
            data: logic<8>,
        }
        var p: Pair;
        always_comb {
            p.data = i_d;
            o_d = p.data[0+:i_w] as 4;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );

    // LHS variant.
    let code = r#"
    module ModuleA (
        i_w: input  logic<3>,
        i_d: input  logic<8>,
        o_d: output logic<8>,
    ) {
        struct Pair {
            data: logic<8>,
        }
        var p: Pair;
        always_comb {
            p.data = 0;
            p.data[0+:i_w] = i_d as 8;
            o_d = p.data;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );

    // Constant widths on member selects stay accepted.
    let code = r#"
    module ModuleA (
        i_d: input  logic<8>,
        o_d: output logic<4>,
    ) {
        struct Pair {
            data: logic<8>,
        }
        var p: Pair;
        always_comb {
            p.data = i_d;
            o_d = p.data[0+:4];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );
}

#[test]
fn non_constant_range_part_select_bounds() {
    // Regression: a `[msb:lsb]` range part-select only had its width checked
    // for `+:`/`-:`/`step`, never for `Colon`, so a runtime bound (msb or lsb)
    // was accepted and emitted as illegal SystemVerilog.

    // Runtime lsb (const msb).
    let code = r#"
    module ModuleA (
        i_i: input  logic<3>,
        i_d: input  logic<8>,
        o_d: output logic<8>,
    ) {
        var r: logic<8>;
        always_comb {
            r = 0;
            r[7:i_i] = i_d;
        }
        assign o_d = r;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );

    // Runtime msb (const lsb).
    let code = r#"
    module ModuleA (
        i_i: input  logic<3>,
        i_d: input  logic<8>,
        o_d: output logic<8>,
    ) {
        assign o_d = i_d[i_i:0];
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );

    // Constant bounds stay accepted.
    let code = r#"
    module ModuleA (
        i_d: input  logic<8>,
        o_d: output logic<8>,
    ) {
        assign o_d = i_d[7:0];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );
}

#[test]
fn enum_xz_variant_checks() {
    // Regression: a variant value containing any x/z bit bypassed the
    // too-large and width-inference checks (8'b1111111x in a 2-bit enum was
    // silently truncated).
    let code = r#"
    module Top {
        enum Foo: logic<2> {
            A = 8'b1111111x,
        }
        var _v: Foo;
        assign _v = Foo::A;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TooLargeEnumVariant { .. })),
        "{errors:?}"
    );

    // Fitting x values stay accepted.
    let code = r#"
    module Top {
        enum Foo: logic<2> {
            A = 2'b1x,
            B = 2'b00,
        }
        var _v: Foo;
        assign _v = Foo::B;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TooLargeEnumVariant { .. })),
        "{errors:?}"
    );
}

#[test]
fn enum_forward_reference_rejected() {
    // Regression: a bare-name forward reference between variants read the
    // stale ImplicitValue(0) placeholder, baking inconsistent values into
    // the checks (false duplicate against the phantom value) and emitting
    // SV with a forward reference.
    let code = r#"
    module Top {
        enum Foo: logic<8> {
            A = B + 1,
            B,
            C = 1,
        }
        var _v: logic<8>;
        assign _v = Foo::C;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::ReferringBeforeDefinition { .. })),
        "{errors:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::DuplicateEnumVariant { .. })),
        "no false duplicate: {errors:?}"
    );

    // Backward references stay accepted.
    let code = r#"
    module Top {
        enum Foo: logic<8> {
            A,
            B = A + 4,
        }
        var _v: logic<8>;
        assign _v = Foo::B;
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn invalid_range_assign() {
    let code = r#"
    proto package a_proto_pkg {
        type T;
    }
    package a_pkg::<W: u32> for a_proto_pkg {
        type T = logic<W>;
    }
    interface b_if::<PKG: a_proto_pkg> {
        var b: PKG::T;

        modport mp {
            b: output,
        }
    }
    module c_module::<PKG: a_proto_pkg> (
        b: modport b_if::<PKG>::mp[2]
    ) {
        assign b[0].b = 0;
        assign b[1].b = 0;
    }
    module d_module::<D: u32> {
        inst e: e_module;
    }
    module e_module {
        inst b: b_if::<a_pkg::<32>>[2];
        inst c: c_module::<a_pkg::<32>>(b: b[0:1]);
    }
    module f_module {
        inst d: d_module::<8>;
    }
    "#;

    let error = analyze(code);
    assert!(error.is_empty());
}

#[test]
fn out_of_range_lhs_select_rejected() {
    // Regression: a constant out-of-range index/bit-select on an assignment
    // LHS was never validated; it wrapped modulo the array shape (emitting
    // out-of-bounds SV, suppressing unassigned warnings, and producing a
    // bogus multiple_assignment against the aliased element).
    for code in [
        // array index out of range
        r#"
        module Top (o: output logic<8>) {
            var f: logic<8> [2];
            assign f[5] = 1;
            assign f[0] = 3;
            assign o = f[0] + f[1];
        }
        "#,
        // bit-select out of range
        r#"
        module Top (o: output logic) {
            var g: logic<8>;
            assign g[12] = 1;
            assign o = g[0];
        }
        "#,
        // out-of-range bit-select on a struct member: it would otherwise
        // remap past the member into an adjacent field via to_base_select.
        r#"
        module Top (o: output logic<12>) {
            struct S { a: logic<4>, b: logic<8> }
            var s: S;
            always_comb { s.a[10] = 1; o = s; }
        }
        "#,
        // out-of-range bit-select range on a struct member
        r#"
        module Top (o: output logic<12>) {
            struct S { a: logic<4>, b: logic<8> }
            var s: S;
            always_comb { s.a = 0; s.a[11:8] = 0; o = s; }
        }
        "#,
    ] {
        let errors = analyze(code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidSelect { .. })),
            "{errors:?}"
        );
    }

    // In-range LHS selects stay accepted, including struct-member selects.
    for code in [
        r#"
        module Top (o: output logic<8>) {
            var f: logic<8> [2];
            assign f[0] = 3;
            assign f[1] = 1;
            assign o = f[0] + f[1];
        }
        "#,
        r#"
        module Top (o: output logic<12>) {
            struct S { a: logic<4>, b: logic<8> }
            var s: S;
            always_comb { s.a = 0; s.b = 0; s.a[3] = 1; s.b[7:4] = 0; o = s; }
        }
        "#,
    ] {
        let errors = analyze(code);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidSelect { .. })),
            "{errors:?}"
        );
    }
}

#[test]
fn clock_domain_function_call() {
    // Regression: a function call's comptime was built from the return type
    // alone (clock_domain None), so routing a signal through any function
    // laundered the crossing — via the return value and via output args.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_a    : input  'a logic,
        o_b    : output 'b logic,
    ) {
        function FuncF (
            x: input logic,
        ) -> logic {
            return x;
        }
        assign o_b = FuncF(i_a);
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Output-argument variant.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_a    : input  'a logic,
        o_b    : output 'b logic,
    ) {
        var t: 'b logic;
        function FuncG (
            x: input  logic,
            y: output logic,
        ) {
            y = x;
        }
        always_comb {
            FuncG(i_a, t);
        }
        assign o_b = t;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain calls stay accepted.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_a    : input  'a logic,
        o_a    : output 'a logic,
    ) {
        function FuncF (
            x: input logic,
        ) -> logic {
            return x;
        }
        assign o_a = FuncF(i_a);
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_system_function() {
    // Regression: $signed/$unsigned laundered the operand's clock domain to
    // None, so a crossing through them passed CDC silently.
    let code = r#"
    module ModuleA (
        i_clk_b: input  'b clock,
        i_a    : input  'a logic<8>,
        o_b    : output 'b logic<8>,
    ) {
        var r: 'b logic<8>;
        always_ff {
            r = $signed(i_a);
        }
        assign o_b = r;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // $unsigned in a binary operand must not launder the domain either.
    let code = r#"
    module ModuleA (
        i_clk_b: input  'b clock,
        i_a    : input  'a logic<8>,
        i_b    : input  'b logic<8>,
        o_b    : output 'b logic<8>,
    ) {
        var r: 'b logic<8>;
        always_ff {
            r = $unsigned(i_a) + i_b;
        }
        assign o_b = r;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain $signed stays accepted.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_a    : input  'a logic<8>,
        o_a    : output 'a logic<8>,
    ) {
        var r: 'a logic<8>;
        always_ff {
            r = $signed(i_a);
        }
        assign o_a = r;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_select_index() {
    // Regression: a bit/array-select index expression's clock domain was
    // neither checked nor merged, so `dat_'b[sel_'a]` passed CDC silently
    // while `o_b = i_sel` was flagged.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_sel  : input  'a logic,
        i_dat  : input  'b logic<2>,
        o_b    : output 'b logic,
    ) {
        assign o_b = i_dat[i_sel];
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain (and constant) indices stay accepted.
    let code = r#"
    module ModuleA (
        i_clk_b: input  'b clock,
        i_sel  : input  'b logic,
        i_dat  : input  'b logic<2>,
        o_b    : output 'b logic,
    ) {
        var t: 'b logic;
        assign t   = i_dat[i_sel];
        assign o_b = t & i_dat[0];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_array_literal_compound_element() {
    // Regression: an array-literal element wrapped in any operator was left
    // unevaluated at gather time (clock_domain None), so the crossing that is
    // flagged for a bare-identifier element was silently laundered.
    for element in ["i_dat & 1'b1", "~i_dat", "i_dat"] {
        let code = format!(
            r#"
            module ModuleA (
                i_clk_a: input  'a clock,
                i_clk_b: input  'b clock,
                i_dat  : input  'a logic,
                o_dat  : output 'b logic [2],
            ) {{
                assign o_dat = '{{{element}, 1'b0}};
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
            "element {element}: {errors:?}"
        );
    }

    // Same-domain compound elements stay accepted.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_dat  : input  'a logic,
        o_dat  : output 'a logic [2],
    ) {
        assign o_dat = '{i_dat & 1'b1, 1'b0};
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
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

    // A clock domain must survive a binary operation. Previously the result of
    // any binary/unary/ternary operation defaulted to ClockDomain::None, which
    // is compatible with everything, so the crossing went undetected.
    let code = r#"
    module ModuleBinary (
        i_dat0: input  'a logic<8>,
        i_dat1: input  'a logic<8>,
        o_dat : output 'b logic<8>,
    ) {
        assign o_dat = i_dat0 & i_dat1;
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleUnary (
        i_dat: input  'a logic<8>,
        o_dat: output 'b logic<8>,
    ) {
        assign o_dat = ~i_dat;
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleTernary (
        i_sel : input  'a logic,
        i_dat0: input  'a logic<8>,
        i_dat1: input  'a logic<8>,
        o_dat : output 'b logic<8>,
    ) {
        assign o_dat = if i_sel ? i_dat0 : i_dat1;
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    // A trailing constant element must not launder the concatenation's domain.
    let code = r#"
    module ModuleConcatConst (
        i_dat: input  'a logic<8>,
        o_dat: output 'b logic<9>,
    ) {
        assign o_dat = {i_dat, 1'b0};
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    module ModuleArrayLit (
        i_dat0: input  'a logic<8>,
        i_dat1: input  'a logic<8>,
        o_dat : output 'b logic<8> [2],
    ) {
        assign o_dat = '{i_dat0, i_dat1};
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    let code = r#"
    package PkgStruct {
        struct S {
            x: logic<8>,
        }
    }
    module ModuleStructCtor (
        i_dat: input  'a logic<8>,
        o_dat: output 'b PkgStruct::S,
    ) {
        assign o_dat = PkgStruct::S'{ x: i_dat };
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchClockDomain { .. }
    ));

    // Same-domain operations must NOT be flagged (no false positive).
    let code = r#"
    module ModuleSameDomain (
        i_sel : input  'a logic,
        i_dat0: input  'a logic<8>,
        i_dat1: input  'a logic<8>,
        o_dat : output 'a logic<8>,
    ) {
        assign o_dat = if i_sel ? (i_dat0 & i_dat1) : ~i_dat0;
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());
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
        let rst: reset = 0;

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
        let rst: reset_async_low = 0;

        inst u: $sv::Module (
            rst,
        );
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        let rst: reset = 0;

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
            .any(|e| matches!(e, AnalyzerError::ReferringBeforeDefinition { .. }))
    );

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
            .any(|e| matches!(e, AnalyzerError::UnresolvableGenericExpression { .. }))
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
            .any(|e| matches!(e, AnalyzerError::UnresolvableGenericExpression { .. }))
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
        AnalyzerError::UnresolvableGenericExpression { .. }
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
        var _a: logic;
        always_comb {
            _a = PkgC::Func::<PkgB::FOO.bar>();
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    function func::<W: u32>(
        a: input logic<W>,
        b: input logic<W>,
    ) -> logic<W> {
        return a + b;
    }
    module ModuleA #(
        param W: u32 = 8,
    )(
        a: input  logic<W>,
        b: input  logic<W>,
        c: output logic<W>,
    ) {
        assign c = func::<W>(a, b);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    function func::<T: type>(
        a: input T,
        b: input T,
    ) -> T {
        return a + b;
    }
    module ModuleA #(
        param W: u32 = 8,
    )(
        a: input  logic<W>,
        b: input  logic<W>,
        c: output logic<W>,
    ) {
        type T = logic<W>;
        assign c = func::<T>(a, b);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<A: u32> {}
    module ModuleB {
        gen B: u32 = 1;
        inst u: ModuleA::<B>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn unresolvable_generic_expression() {
    let code = r#"
    package Pkg::<a: u32> {
        const A: u32 = a;
    }
    function func() -> u32 {
        return Pkg::<8>::A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));

    let code = r#"
    package Pkg::<W: u32> {
        type T = logic<W>;
    }
    function func() -> Pkg::<8>::T {
        var a: Pkg::<8>::T;
        a = 0 as Pkg::<8>::T;
        return a;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));
    assert!(matches!(
        errors[1],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));
    assert!(matches!(
        errors[2],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));

    let code = r#"
    package Pkg {
        function func::<a: u32>() -> u32 {
            return a;
        }
    }
    function func() -> u32 {
        return Pkg::func::<8>();
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));

    let code = r#"
    package Pkg {
        const A: u32 = 8;
    }
    function func() -> u32 {
        return Pkg::A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package Pkg {
        type T = logic<8>;
    }
    function func() -> Pkg::T {
        var a: Pkg::T;
        a = 0 as Pkg::T;
        return a;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package Pkg {
        function func() -> u32 {
            return 8;
        }
    }
    function func() -> u32 {
        return Pkg::func();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    function func_a::<a: u32> -> u32 {
        return a;
    }
    function func_b() -> u32 {
        return func_a::<8>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        const A: u32 = 1;
        const B: u32 = 2;
        gen   C: u32 = A + B;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));

    let code = r#"
    module ModuleA {
        const W: u32  = 8;
        gen   T: type = logic<W>;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));

    let code = r#"
    module ModuleA {
        const A: type = logic<8>;
        gen   B: type = A;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::UnresolvableGenericExpression { .. }
    ));

    let code = r#"
    module ModuleA::<A: u32, B: u32> {
        gen C: u32 = A + B;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        gen A: u32 = 1;
        gen B: u32 = 2;
        gen C: u32 = A + B;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        const A: u32 = 1;
    }
    package PkgB {
        const B: u32= 2;
    }
    module ModuleC {
        import PkgA::*;
        import PkgB::*;
        gen C: u32 = A + B;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        gen W: u32  = 8;
        gen T: type = logic<W>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<W: u32> {
        gen T: type = logic<W>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        gen A: type = logic<8>;
        gen B: type = A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<A: type> {
        gen B: type = A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        type A = logic<8>;
    }
    module ModuleB {
        import PkgA::*;
        gen B: type = A;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package PkgA {
        struct struct_a {
            a: u32,
            b: u32,
            c: u32,
        }
    }
    proto package ProtoPkgB {
        const B: PkgA::struct_a;
    }
    package PkgB::<a: u32, b: u32, c: u32> for ProtoPkgB {
        const B: PkgA::struct_a = PkgA::struct_a'{
            a: a,
            b: b,
            c: c,
        };
    }
    module ModuleA::<PKG: ProtoPkgB> {
        gen innter_a: u32 = PKG::B.a + 1;
        alias package innter_pkg = PkgB::<innter_a, PKG::B.b, PKG::B.c>;
    }
    module ModuleB {
        inst u: ModuleA::<PkgB::<1, 2, 3>>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        gen A: u32 = 8;
        gen B: u32 = $clog2(A);
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

    let errors = analyze_with_large_stack(code);
    assert!(matches!(errors[0], AnalyzerError::ExceedLimit { .. }));

    let code = r#"
    module ModuleA {
        let _a: logic = {1'b1 repeat 10000000};
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(errors[0], AnalyzerError::ExceedLimit { .. }));

    let code = r#"
    module ModuleA {
        var _a: logic<10000000>;
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(errors[0], AnalyzerError::ExceedLimit { .. }));

    let code = r#"
    module ModuleA {
        var a: logic<10>;

        always_comb {
            for i in 0..10 {
                if i == 0 {
                    a[i] = 0;
                } else {
                    a[i] = a[i-1];
                }
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module TopModule {
        const W: u32 = 64;
        function ext::<WIDTH: u32> () -> logic<64> {
            return {1'b0 repeat W - 1, 1'b0};
        }
        let _a: logic<64> = ext::<64>();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

    let code = r#"
    package Pkg {
        function f::<N: u32> -> logic<N> {
            gen M: u32 = N;
            return f::<M>();
        }
    }
    module ModuleB {
        let _a: logic<4> = Pkg::f::<4>();
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InfiniteRecursion { .. }))
    );
}

#[test]
fn recursive_generic_function() {
    let code = r#"
    package Pkg {
        function f::<N: u32> -> logic<N> {
            gen M: u32 = N - 1;
            var out: logic<N>;
            if N == 1 {
                out = 0;
            } else {
                out = {1'b0, f::<M>()};
            }
            return out;
        }
    }
    module ModuleB {
        let _a: logic<4> = Pkg::f::<4>();
    }
    "#;

    let errors = analyze_with_large_stack(code);
    assert!(errors.is_empty());

    let code = r#"
    package Pkg {
        function f::<N: u32> -> logic<N> {
            gen M: u32 = N - 1;
            var out: logic<N>;
            if N == 1 {
                out = 0;
            } else {
                out = {1'b0, f::<M>()};
            }
            return out;
        }
    }
    module ModuleB {
        let _a: logic<20> = Pkg::f::<20>();
    }
    "#;

    let errors = analyze_with_large_stack(code);
    assert!(errors.is_empty());
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
fn duplicate_argument() {
    // The same named argument connected twice (port x bound twice, y never): the
    // emitted SV connects one port twice and leaves another unconnected.
    let code = r#"
    module ModuleA {
        function Add (
            x: input u32,
            y: input u32,
        ) -> u32 {
            return x + y;
        }

        let _a: u32 = Add(x: 10, x: 20);
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::DuplicateArgument { .. }))
    );

    // Distinct named arguments must NOT be flagged.
    let code = r#"
    module ModuleB {
        function Add (
            x: input u32,
            y: input u32,
        ) -> u32 {
            return x + y;
        }

        let _a: u32 = Add(x: 10, y: 20);
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::DuplicateArgument { .. }))
    );
}

#[test]
fn mixed_struct_union_member() {
    let code = r#"
    package Pkg {
        struct StructA {
            a: logic,
            b: bit  ,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MixedStructUnionMember { .. }
    ));

    let code = r#"
    package Pkg {
        union UnionA {
            a: logic,
            b: bit  ,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MixedStructUnionMember { .. }
    ));

    let code = r#"
    package Pkg {
        enum EnumA {
            A
        }
        struct StructA {
            a: EnumA,
            b: bit  ,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MixedStructUnionMember { .. }
    ));

    let code = r#"
    package Pkg {
        enum EnumA: bit<_> {
            A
        }
        struct StructA {
            a: EnumA,
            b: logic,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MixedStructUnionMember { .. }
    ));

    let code = r#"
    package Pkg::<T: type> {
        struct StructA {
            a: logic,
            b: T   ,
        }
    }
    module ModuleA {
        import Pkg::<u32>::StructA;
        let _a: StructA = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MixedStructUnionMember { .. }
    ));

    let code = r#"
    module ModuleA {
        struct StructA::<T: type> {
            a: logic,
            b: T    ,
        }
        const A: StructA::<u32> = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MixedStructUnionMember { .. }
    ));

    let code = r#"
    module ModuleA {
        struct StructA::<T: type> {
            a: bit,
            b: T  ,
        }
        const A: StructA::<u32> = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    package pkg::<T: type> {
        struct Struct {
            x: T,
        }
        function make () -> Struct {
            return 0;
        }
    }
    package types {
        type T = bit;
    }
    module top () {
        let _: pkg::<types::T>::Struct = pkg::<types::T>::make();
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    // $sv-imported members don't claim either 2-state or 4-state, so the
    // state-uniformity check must accept them in any combination.
    let code = r#"
    package Pkg {
        struct all_sv_struct {
            a: $sv::Cfg   ,
            b: $sv::Cfg   ,
            c: $sv::Cfg<2>,
        }
        struct sv_with_2state {
            a: $sv::Cfg,
            b: bit<8>  ,
        }
        struct sv_with_4state {
            a: $sv::Cfg,
            b: logic<8>,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn generic_inference_failed() {
    // Argument width cannot be determined → inference fails. Should
    // surface a dedicated error rather than `mismatch_generics_arity`.
    let code = r#"
    module ModuleA {
        function FuncId::<T: u32> (
            x: input logic<T>,
        ) -> logic<T> {
            return x;
        }

        let _r: logic<8> = FuncId(8'd0);
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::GenericInferenceFailed { .. }))
    );
}

#[test]
fn type_inference_var_conflict() {
    // Two assigns to the same untyped `var` with mismatched widths
    // should be rejected as not inferable.
    let code = r#"
    module ModuleA {
        let _a: logic<8>  = 0;
        let _b: logic<16> = 0;
        var _v;
        assign _v = _a;
        always_comb {
            _v = _b;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TypeInferenceConflict { .. }))
    );
}

#[test]
fn type_inference_not_supported() {
    // Binary arithmetic is not an inferable expression shape.
    let code = r#"
    module ModuleA {
        let _a: logic<8> = 0;
        let _b           = _a + 1;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::TypeInferenceNotSupported { .. }
    ));

    // Unsized literal is not inferable.
    let code = r#"
    module ModuleA {
        let _a = 42;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::TypeInferenceNotSupported { .. }
    ));

    // A struct/union/enum inference has no SV scalar type name, so the
    // emitter would declare it as a 1-bit `logic`; it must be rejected.
    let code = r#"
    module ModuleA {
        struct Pair {
            a: logic<8>,
            b: logic<8>,
        }
        var p: Pair;
        var q;
        always_comb {
            p.a = 1;
            p.b = 2;
            q   = p;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TypeInferenceNotSupported { .. }))
    );

    let code = r#"
    module ModuleA {
        enum Mode {
            A,
            B,
        }
        var m: Mode;
        var r;
        always_comb {
            m = Mode::A;
            r = m;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TypeInferenceNotSupported { .. }))
    );

    let code = r#"
    module ModuleA {
        union U {
            a: logic<8>,
            b: logic<8>,
        }
        var u: U;
        always_comb {
            u.a = 1;
        }
        let _v = u;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TypeInferenceNotSupported { .. }))
    );
}

#[test]
fn type_inference_always_comb_var() {
    let code = r#"
    module ModuleA {
        let _a: logic<8> = 0;
        var _v;
        always_comb {
            _v = _a;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn type_inference_multiple_generic_params() {
    let code = r#"
    module ModuleA {
        function FuncAB::<A: u32, B: u32> (
            x: input logic<A>,
            y: input logic<B>,
        ) -> logic<A> {
            return x;
        }

        let _a: logic<8>  = 0;
        let _b: logic<16> = 0;
        let _r: logic<8>  = FuncAB(_a, _b);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn type_inference_generic_default() {
    let code = r#"
    module ModuleA {
        function FuncDef::<A: u32, B: u32 = 2> (
            x: input logic<A>,
        ) -> logic<A> {
            return x;
        }

        let _a: logic<8> = 0;
        let _r: logic<8> = FuncDef(_a);
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
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

#[test]
fn positive_type_validation_zero() {
    let code = r#"
    module ModuleA {
        const A: p8 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::NonPositiveValue { .. }));

    let code = r#"
    module ModuleA {
        const A: p16 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::NonPositiveValue { .. }));

    let code = r#"
    module ModuleA {
        const A: p32 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::NonPositiveValue { .. }));

    let code = r#"
    module ModuleA {
        const A: p64 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::NonPositiveValue { .. }));

    let code = r#"
    module ModuleA #(
        param P: p32 = 0
    ) {}
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::NonPositiveValue { .. }));

    let code = r#"
    module ModuleA {
        let _a: p8 = 0;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::NonPositiveValue { .. }));
}

#[test]
fn positive_type_validation_max() {
    let code = r#"
    module ModuleA {
        const A: p8 = 8'd255;
        const B: p16 = 16'd65535;
        const C: p32 = 32'd4294967295;
        const D: p64 = 64'd18446744073709551615;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn positive_type_validation() {
    let code = r#"
    module ModuleA {
        const A: p8 = 1;
        const B: p16 = 255;
        const C: p32 = 65535;
        const D: p64 = 32'hFFFFFFFF;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleF #(
        param P: p32 = 100
    ) {}
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleI {
        const A: p32 = 10;
        const B: u32 = 0;
        const C: p16 = 1;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleJ {
        const A: p8 = 8'hFF;
        const B: p16 = 16'h1234;
        const C: p32 = 32'h1;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleK {
        const A: p8 = 8'b1;
        const B: p16 = 16'b1111_1111_1111_1111;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleL {
        let a: p8 = -1;
    }
    "#;

    let errors = analyze(code);
    let non_pos_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::NonPositiveValue { .. }))
        .collect();
    assert_eq!(non_pos_errors.len(), 1);

    let code = r#"
    module ModuleL {
        let a: p8 = 'x;
    }
    "#;

    let errors = analyze(code);
    let non_pos_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::NonPositiveValue { .. }))
        .collect();
    assert_eq!(non_pos_errors.len(), 1);

    let code = r#"
    module ModuleL {
        let a: p8 = 'z;
    }
    "#;

    let errors = analyze(code);
    let non_pos_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::NonPositiveValue { .. }))
        .collect();
    assert_eq!(non_pos_errors.len(), 1);

    let code = r#"
    module ModuleM {
        let b: p16 = -1;
        let c: p32 = -100;
    }
    "#;

    let errors = analyze(code);
    let non_pos_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::NonPositiveValue { .. }))
        .collect();
    assert_eq!(non_pos_errors.len(), 2);

    let code = r#"
    module ModuleM {
        let b: p16 = -(-1);
    }
    "#;

    let errors = analyze(code);
    let non_pos_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::NonPositiveValue { .. }))
        .collect();
    assert_eq!(non_pos_errors.len(), 0);

    // let code = r#"
    // module ModuleN {
    //     let arr: p8[2] = '{1, 0};
    // }
    // "#;

    // let errors = analyze(code);
    // let non_pos_errors: Vec<_> = errors
    //     .iter()
    //     .filter(|e| matches!(e, AnalyzerError::NonPositiveValue { .. }))
    //     .collect();
    // assert_eq!(non_pos_errors.len(), 1);

    // let code = r#"
    // module ModuleO {
    //     let a: p8 = 1;
    //     let b: p16 = 100;
    //     let c: p32[2] = '{1, 255};
    // }
    // "#;

    // let errors = analyze(code);
    // let non_pos_errors: Vec<_> = errors
    //     .iter()
    //     .filter(|e| matches!(e, AnalyzerError::NonPositiveValue { .. }))
    //     .collect();
    // assert_eq!(non_pos_errors.len(), 0);

    let code = r#"
    module ModuleA {
        function func(a: input p16) -> p16 {
            return a;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        function func(a: input p16, b: input p16) -> p16 {
            return a + b;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA {
        function func(a: input p8, b: input p8) -> p8 {
            return a + b;
        }
        const A: p8 = func(8'hff, 8'h01);
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::NonPositiveValue { .. }));
}

#[test]
fn invalid_wavedrom() {
    // Valid wavedrom (no test attribute) should not produce errors
    let code = r#"
    /// ```wavedrom
    /// {signal: [{name: 'clk', wave: 'p....'}]}
    /// ```
    module ModuleA (
        clk: input clock,
        rst: input reset,
    ) {}
    "#;

    let errors = analyze(code);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert!(wavedrom_errors.is_empty());

    // Invalid JSON in wavedrom block should produce error
    let code = r#"
    /// ```wavedrom
    /// {signal: [BROKEN
    /// ```
    module ModuleB (
        clk: input clock,
        rst: input reset,
    ) {}
    "#;

    let errors = analyze(code);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert_eq!(wavedrom_errors.len(), 1);

    // Invalid wave character should produce error
    let code = r#"
    /// ```wavedrom
    /// {signal: [{name: 'sig', wave: '01Q10'}]}
    /// ```
    module ModuleC (
        clk: input clock,
        rst: input reset,
    ) {}
    "#;

    let errors = analyze(code);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert_eq!(wavedrom_errors.len(), 1);

    // wavedrom,test with pipe separator should produce error
    let code = r#"
    /// ```wavedrom,test
    /// {signal: [
    ///   {name: 'clk', wave: 'p...|..'},
    ///   {name: 'dat', wave: '010|101'},
    ///   {name: 'out', wave: '010|101'}
    /// ]}
    /// ```
    module ModuleD (
        clk: input clock,
        rst: input reset,
        dat: input logic,
        out: output logic,
    ) {
        assign out = dat;
    }
    "#;

    let errors = analyze(code);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert_eq!(wavedrom_errors.len(), 1);

    // wavedrom,test with no matching ports should produce error
    let code = r#"
    /// ```wavedrom,test
    /// {signal: [
    ///   {name: 'clk', wave: 'p....'},
    ///   {name: 'foo', wave: '01010'},
    ///   {name: 'bar', wave: '10101'}
    /// ]}
    /// ```
    module ModuleE (
        clk: input clock,
        rst: input reset,
        dat: input logic,
        out: output logic,
    ) {
        assign out = dat;
    }
    "#;

    let errors = analyze(code);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert_eq!(wavedrom_errors.len(), 1);

    // Valid wavedrom,test should not produce errors
    let code = r#"
    /// ```wavedrom,test
    /// {signal: [
    ///   {name: 'clk', wave: 'p....'},
    ///   {name: 'dat', wave: '01010'},
    ///   {name: 'out', wave: '01010'}
    /// ]}
    /// ```
    module ModuleF (
        clk: input clock,
        rst: input reset,
        dat: input logic,
        out: output logic,
    ) {
        assign out = dat;
    }
    "#;

    let errors = analyze(code);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert!(wavedrom_errors.is_empty());

    // Plain wavedrom with pipe (decorative) should NOT produce error
    let code = r#"
    /// ```wavedrom
    /// {signal: [
    ///   {name: 'clk', wave: 'p...|..'},
    ///   {name: 'dat', wave: '010|101'}
    /// ]}
    /// ```
    module ModuleG (
        clk: input clock,
        rst: input reset,
    ) {}
    "#;

    let errors = analyze(code);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert!(wavedrom_errors.is_empty());
}

#[test]
fn doc_comment_table_leak_across_analyses() {
    // First source: a doc comment whose wavedrom block sits above ModuleA on line 9.
    let code1 = r#"/// ```wavedrom
/// { signal: [{ name: "Alfa", wave: "u" }] }
/// ```
///
///
///
///
///
module ModuleA {
}
"#;
    let _ = analyze(code1);

    // Second source: clean code, ModuleB on the same line 9. If doc_comment_table
    // is not cleared between analyses, ModuleB inherits the wavedrom doc comment
    // and a bogus InvalidWavedrom error is reported.
    let code2 = r#"package PackageB {
    enum Color: logic<2> {
        Red,
        Green,
        Blue,
    }
}

module ModuleB {
}
"#;
    let errors = analyze(code2);
    let wavedrom_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::InvalidWavedrom { .. }))
        .collect();
    assert!(
        wavedrom_errors.is_empty(),
        "doc_comment_table leaked across analyses: {:?}",
        wavedrom_errors
    );
}

#[test]
fn unevaluable_value_for_range() {
    // non-const variable used as generate for range bound
    let code = r#"
    module ModuleA {
        let a: logic<10> = 0;
        for i in 0..a :label {
            assign a[i] = i + 2;
        }
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

    // const variable used as generate for range bound should be fine
    let code = r#"
    module ModuleA {
        var a: logic<10>;
        const N: u32 = 10;
        for i in 0..N :label {
            assign a[i] = i + 2;
        }
    }"#;

    let errors = analyze(code);
    let unevaluable_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::UnevaluableValue { .. }))
        .collect();
    assert!(unevaluable_errors.is_empty());
}

#[test]
fn unevaluable_value_system_function_arg() {
    let code = r#"
    module ModuleA (
        d: input  logic<8> ,
        q: output logic<32>,
    ) {
        always_comb {
            q = $clog2(d);
        }
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

    let code = r#"
    module ModuleA (
        d: input  logic<8>,
        q: output logic   ,
    ) {
        always_comb {
            q = $onehot(d);
        }
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

    let code = r#"
    module ModuleA {
        const A: u32 = $clog2(8);
        const B: u32 = $onehot(8);
    }"#;

    let errors = analyze(code);
    let unevaluable_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::UnevaluableValue { .. }))
        .collect();
    assert!(unevaluable_errors.is_empty());
}

#[test]
fn unevaluable_value_parameter_value() {
    // non-const variable used as parameter override
    let code = r#"
    module SubA #(
        param C: u32 = 1,
    ) {}

    module ModuleA {
        let c: u32 = 3;
        inst u_sub: SubA #( C: c );
    }"#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnevaluableValue { .. }));

    // const variable used as parameter override should be fine
    let code = r#"
    module SubA #(
        param C: u32 = 1,
    ) {}

    module ModuleA {
        const C: u32 = 3;
        inst u_sub: SubA #( C: C );
    }"#;

    let errors = analyze(code);
    let unevaluable_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::UnevaluableValue { .. }))
        .collect();
    assert!(unevaluable_errors.is_empty());

    let code = r#"
    module ModuleA::<a: u32, b: u32> {
        gen   A_B: u32 = a + b;
        const C  : u32 = A_B;
    }
    module ModuleB {
        inst u: ModuleA::<1, 2>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    module ModuleA::<W: u32> {
        gen   T0: type = logic<W>;
        const T1: type = T0;
    }
    module ModuleB {
        inst u: ModuleA::<1>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn mismatch_function_arity_system_function() {
    let code = r#"
    module ModuleA {
        let _a: u32 = $bits(1, 2);
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = $size(1, 2);
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = $clog2(1, 2);
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = $onehot(1, 2);
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));

    let code = r#"
    module ModuleA {
        let _a: u32 = $readmemh("file.hex");
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(
        errors[0],
        AnalyzerError::MismatchFunctionArity { .. }
    ));
}

#[test]
fn invalid_select_after_range() {
    let code = r#"
    module ModuleA {
        var a: logic<8>;
        let _b: logic = a[3:0][0];
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidSelect { .. }));
}

#[test]
fn multiple_default_array_literal() {
    let code = r#"
    module ModuleA {
        let _a: u32[4] = '{default: 0, default: 1};
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(errors[0], AnalyzerError::MultipleDefault { .. }));
}

#[test]
fn mismatch_dimension_array() {
    let code = r#"
    module ModuleA {
        let _a: u32[2] = '{1, 2, 3};
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(errors[0], AnalyzerError::MismatchType { .. }));
}

#[test]
fn missing_member_struct_constructor() {
    let code = r#"
    module ModuleA {
        struct StructA {
            x: logic,
            y: logic,
        }

        let _a: StructA = StructA'{x: 1};
    }
    "#;

    let errors = analyze_with_ir(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));
}

#[test]
fn local_var_in_always_ff_uses_var_naming() {
    let code = r#"
    module ModuleA (
        i_clk: input  clock,
        i_rst: input  reset,
        o_x  : output logic,
    ) {
        always_ff {
            if_reset {
                o_x = 1'b0;
            } else {
                var tmp: logic;
                tmp = 1'b1;
                o_x = tmp;
            }
        }
    }
    "#;

    let mut lint = Lint::default();
    lint.naming.prefix_reg = Some("r_".to_string());
    lint.naming.prefix_var = Some("v_".to_string());

    let errors = analyze_with_lint(code, lint);

    let invalid: Vec<_> = errors
        .iter()
        .filter_map(|e| match e {
            AnalyzerError::InvalidIdentifier {
                identifier, rule, ..
            } => Some((identifier.as_str(), rule.as_str())),
            _ => None,
        })
        .collect();

    assert!(
        invalid
            .iter()
            .any(|(id, rule)| *id == "tmp" && rule.contains("v_")),
        "expected `tmp` to be flagged by the var rule (`v_`), got: {invalid:?}"
    );
    assert!(
        !invalid
            .iter()
            .any(|(id, rule)| *id == "tmp" && rule.contains("r_")),
        "`tmp` should not be flagged by the reg rule (`r_`), got: {invalid:?}"
    );
    assert!(
        invalid
            .iter()
            .any(|(id, rule)| *id == "o_x" && rule.contains("r_")),
        "expected `o_x` to keep the reg rule (`r_`), got: {invalid:?}"
    );
}

#[test]
fn local_var_in_always_comb_uses_var_naming() {
    let code = r#"
    module ModuleA (
        i_a: input  logic,
        o_x: output logic,
    ) {
        always_comb {
            var tmp: logic;
            tmp = i_a;
            o_x = tmp;
        }
    }
    "#;

    let mut lint = Lint::default();
    lint.naming.prefix_wire = Some("w_".to_string());
    lint.naming.prefix_var = Some("v_".to_string());

    let errors = analyze_with_lint(code, lint);

    let invalid: Vec<_> = errors
        .iter()
        .filter_map(|e| match e {
            AnalyzerError::InvalidIdentifier {
                identifier, rule, ..
            } => Some((identifier.as_str(), rule.as_str())),
            _ => None,
        })
        .collect();

    assert!(
        invalid
            .iter()
            .any(|(id, rule)| *id == "tmp" && rule.contains("v_")),
        "expected `tmp` to be flagged by the var rule (`v_`), got: {invalid:?}"
    );
    assert!(
        !invalid
            .iter()
            .any(|(id, rule)| *id == "tmp" && rule.contains("w_")),
        "`tmp` should not be flagged by the wire rule (`w_`), got: {invalid:?}"
    );
}

#[test]
fn regression_gray_enum_encoding() {
    // Correct Gray-coded explicit values (0,1,3,2) must be accepted.
    let code = r#"
    module M {
        #[enum_encoding(gray)]
        enum E: logic<4> {
            aa = 0,
            bb = 1,
            cc = 3,
            dd = 2,
        }
        var x: E;
        assign x = E::aa;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidEnumVariant { .. })),
        "correct Gray values 0,1,3,2 should not be rejected: {errors:?}"
    );

    // A non-Gray explicit value (dd = 6 after cc = 3) must be rejected.
    let code = r#"
    module M {
        #[enum_encoding(gray)]
        enum E: logic<4> {
            aa = 0,
            bb = 1,
            cc = 3,
            dd = 6,
        }
        var x: E;
        assign x = E::aa;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidEnumVariant { .. })),
        "non-Gray value dd=6 should be rejected: {errors:?}"
    );
}

#[test]
fn regression_modport_inout_expand() {
    // An inout modport member can't be expanded into top-level ports
    // (it would be emitted as `output var`, dropping bidirectionality).
    let code = r#"
    interface InterfaceA {
        var a: logic;
        var b: logic;
        var c: logic;
        modport mp {
            a: input,
            b: output,
            c: inout,
        }
    }

    #[expand(modport)]
    module ModuleA (
        port: modport InterfaceA::mp,
    ) {
        assign port.b = port.a;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnexpandableModport { .. })),
        "inout modport member under #[expand(modport)] should be rejected: {errors:?}"
    );
}

#[test]
fn regression_generic_function_statement_call() {
    // A generic function called as a discarded-result statement must have
    // its generic argument inferred (no false generic_inference_failed).
    let code = r#"
    module Top {
        function f::<A: u32> (
            a: input logic<A>,
        ) {
            let _t: logic<A> = a;
        }
        var aa: logic<5>;
        always_comb {
            f(aa);
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::GenericInferenceFailed { .. })),
        "generic statement-call should infer its argument: {errors:?}"
    );
}

#[test]
fn regression_ambiguous_wildcard_import() {
    // The same name wildcard-imported from two packages and used unqualified
    // must be reported as ambiguous instead of silently bound to one.
    let code = r#"
    package PkgA {
        const X: u32 = 3;
    }
    package PkgB {
        const X: u32 = 7;
    }
    module Top {
        import PkgA::*;
        import PkgB::*;
        var a: logic<32>;
        always_comb {
            a = X;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::AmbiguousIdentifier { .. })),
        "ambiguous wildcard import should be reported: {errors:?}"
    );

    // A single wildcard import of the same name must still resolve cleanly.
    let code = r#"
    package PkgA {
        const X: u32 = 3;
    }
    module Top {
        import PkgA::*;
        var a: logic<32>;
        always_comb {
            a = X;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::AmbiguousIdentifier { .. })),
        "single wildcard import should not be ambiguous: {errors:?}"
    );
}

#[test]
fn regression_ifdef_duplicate_decls_not_ambiguous() {
    // Two declarations of the same identifier guarded by mutually-exclusive
    // `#[ifdef]`/`#[ifndef]` (or ifdef/elsif/else) attributes cannot both be
    // active, so wildcard-importing the package must not flag ambiguity.
    let code = r#"
    package PkgA {
        #[ifdef(DEFINE_X)]
        const X: u32 = 1;
        #[ifndef(DEFINE_X)]
        const X: u32 = 2;
    }
    module Top {
        import PkgA::*;
        var a: logic<32>;
        always_comb {
            a = X;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::AmbiguousIdentifier { .. })),
        "ifdef/ifndef alternatives in one package must not be ambiguous: {errors:?}"
    );

    let code = r#"
    package PkgA {
        #[ifdef(DEFINE_Y)]
        const X: u32 = 1;
        #[elsif(DEFINE_Z)]
        const X: u32 = 2;
        #[else]
        const X: u32 = 3;
    }
    module Top {
        import PkgA::*;
        var a: logic<32>;
        always_comb {
            a = X;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::AmbiguousIdentifier { .. })),
        "ifdef/elsif/else alternatives in one package must not be ambiguous: {errors:?}"
    );
}

#[test]
fn regression_ambiguous_wildcard_import_cross_depth() {
    // The same name wildcard-imported from namespaces of different depths (a
    // package const at depth 2 vs an enum member at depth 3) collides just like
    // the same-depth case and must be reported ambiguous, not silently bound to
    // the deeper candidate.
    let code = r#"
    package PkgA {
        const X: u32 = 7;
    }
    package PkgB {
        enum MyEnum: logic<2> {
            X = 1,
        }
    }
    module Top {
        import PkgA::*;
        import PkgB::MyEnum::*;
        var a: logic<32>;
        always_comb {
            a = X;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::AmbiguousIdentifier { .. })),
        "cross-depth ambiguous wildcard import should be reported: {errors:?}"
    );

    // Distinct names across the two depths must still resolve cleanly.
    let code = r#"
    package PkgA {
        const Y: u32 = 7;
    }
    package PkgB {
        enum MyEnum: logic<2> {
            X = 1,
        }
    }
    module Top {
        import PkgA::*;
        import PkgB::MyEnum::*;
        var a: logic<32>;
        always_comb {
            a = X + Y;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::AmbiguousIdentifier { .. })),
        "distinct cross-depth wildcard imports must not be ambiguous: {errors:?}"
    );
}

#[test]
fn regression_pow_overflow_no_panic() {
    // Constant ** that overflows the host integer must not panic in debug
    // builds (wrapping + width masking, like Op::Mul).
    let code = r#"
    module Top (
        o0: output logic<32>,
    ) {
        always_comb {
            o0 = 10 ** 20;
        }
    }
    "#;
    let _ = analyze(code);

    let code = r#"
    module Top (
        o0: output logic<32>,
    ) {
        always_comb {
            o0 = (-7) ** 23;
        }
    }
    "#;
    let _ = analyze(code);
}

#[test]
fn regression_baseless_literal_overflow_no_panic() {
    // A base-less decimal literal larger than u64::MAX must not panic.
    let code = r#"
    module Top {
        const HUGE: u64 = 99999999999999999999;
    }
    "#;
    let _ = analyze(code);
}

#[test]
fn regression_allbit_width_overflow_no_panic() {
    // An all-bit literal whose width prefix overflows usize must not panic.
    let code = r#"
    module Top {
        let _a: logic = 999999999999999999999999999999'0;
    }
    "#;
    let _ = analyze(code);
}

#[test]
fn regression_rev_for_non_additive_step_rejected() {
    // A reverse loop with a non-additive step cannot be inverted to descend
    // toward the lower bound, so it must be rejected rather than emitted as a
    // non-terminating SystemVerilog loop.
    let code = r#"
    module Top (
        o: output logic<32>,
    ) {
        var sum: logic<32>;
        always_comb {
            sum = 0;
            for i in rev 1..100 step *= 2 {
                sum += i;
            }
            o = sum;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
        "rev for-loop with a non-additive step should be rejected: {errors:?}"
    );

    // A reverse loop with an additive step is fine.
    let code = r#"
    module Top (
        o: output logic<32>,
    ) {
        var sum: logic<32>;
        always_comb {
            sum = 0;
            for i in rev 0..10 step += 2 {
                sum += i;
            }
            o = sum;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
        "rev for-loop with an additive step should be accepted: {errors:?}"
    );

    // A forward loop with a non-additive step is fine.
    let code = r#"
    module Top (
        o: output logic<32>,
    ) {
        var sum: logic<32>;
        always_comb {
            sum = 0;
            for i in 1..100 step *= 2 {
                sum += i;
            }
            o = sum;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
        "forward for-loop with a non-additive step should be accepted: {errors:?}"
    );
}

#[test]
fn no_panic_on_oversized_based_literal() {
    // Regression: a based literal whose value needs >64 bits while its declared
    // width is <=64 previously panicked in ValueBigUint::to_value_u64
    // (Option::unwrap on None). It must now produce a diagnostic, not crash.
    let code = r#"
    module Top {
        let a: logic = 1'h1ffffffffffffffff;
    }
    "#;
    let _ = analyze(code);
}

#[test]
fn sized_xz_literal_narrower_than_digits_accepted() {
    // Regression: 2'hx was rejected as too_large_number (the x digit's
    // 4-bit mask exceeded the declared width) while the identical 2'bxx
    // passed.  x/z fill digits truncate legally; only lost 1-bits count.
    for lit in ["2'hx", "2'bxx", "2'hz", "2'h0x"] {
        let code = format!(
            r#"
            module Top {{
                var a: logic<2>;
                assign a = {lit};
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::TooLargeNumber { .. })),
            "{lit}: {errors:?}"
        );
    }

    // Lost 1-bits still error.
    for lit in ["2'h7", "2'b1111111x"] {
        let code = format!(
            r#"
            module Top {{
                var a: logic<2>;
                assign a = {lit};
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::TooLargeNumber { .. })),
            "{lit}: {errors:?}"
        );
    }
}

#[test]
fn no_panic_on_oversized_literal_in_expression() {
    // Regression: a based literal whose digits need >64 bits with a declared
    // width <=64 stayed a BigUint while its operand partner expanded to U64,
    // hitting `unreachable!()` in Op::eval_value_binary / Value::concat during
    // pass2 (reachable via `veryl dump --ir` and the language server, which
    // run pass2 regardless of pass1 errors).  Only too_large_number may fire.
    for code in [
        r#"
        module Top {
            const A: logic<8> = 8'hffff_ffff_ffff_ffff_ff + 8'h01;
            var x: logic<8>;
            assign x = A;
        }
        "#,
        r#"
        module Top {
            const A: logic<2> = {1'h1ffffffffffffffff, 1'b0};
            var x: logic<2>;
            assign x = A;
        }
        "#,
    ] {
        let errors = analyze(code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::TooLargeNumber { .. })),
            "{errors:?}"
        );
    }
}

#[test]
fn no_panic_on_out_of_range_bit_select() {
    // Regression: a const/expression bit-select with index >= 64 on a <=64-bit
    // value previously panicked in ValueU64::select (u64 shift overflow) in
    // debug/test builds. It must report the out-of-range diagnostic, not crash.
    let code = r#"
    module Top {
        var a: logic<8>;
        var b: logic;
        always_comb {
            b = a[64];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidSelect { .. }))
    );

    let code = r#"
    module Top {
        const A: logic<8> = 8'hFF;
        const B: logic    = A[100];
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidSelect { .. }))
    );
}

#[test]
fn no_panic_on_huge_biguint_left_shift() {
    // Regression: the BigUint left-shift arms shifted by the raw constant
    // amount before masking, so `128'd1 << 64'h4000...` made num-bigint
    // try a ~5.8e17-byte allocation and abort.
    for code in [
        "module Top { const A: logic<128> = 128'd1 << 64'h4000000000000000; let _x: logic<128> = A; }",
        "module Top { const A: logic<128> = 128'd1 <<< 64'h4000000000000000; let _x: logic<128> = A; }",
    ] {
        let _ = analyze(code);
    }
}

#[test]
fn no_panic_on_large_shift_amount() {
    // Regression: a const shift by an amount >= the operand width previously
    // panicked in debug/test builds (native u64 shift overflow and a
    // `width - y` usize underflow in the arithmetic-shift sign-extension mask).
    for code in [
        "module Top { const A: bit<8> = 8'hff >> 100; }",
        "module Top { const A: bit<8> = 8'hff << 80; }",
        "module Top { const A: i8 = -1; const B: i8 = A >>> 9; }",
        "module Top { const A: i8 = 1; const B: i8 = A <<< 80; }",
        "module Top { const A: signed logic<70> = -1; const B: signed logic<70> = A >>> 71; }",
    ] {
        let _ = analyze(code);
    }
}

#[test]
fn no_panic_on_zero_step_for_loop() {
    // Regression: a for-loop with `step += 0` never advances the induction
    // variable, so compile-time unrolling looped forever and OOM-aborted.
    let code = r#"
    module Top (
        o: output logic<32>,
    ) {
        always_comb {
            var acc: logic<32>;
            acc = 0;
            for i in 0..10 step += 0 {
                acc += i;
            }
            o = acc;
        }
    }
    "#;
    let _ = analyze(code);
}

#[test]
fn for_zero_step_rejected() {
    // `step += 0` never advances the induction variable. The unroll path bails in
    // eval_iter, but a dynamic-range loop reaches build_for_statement and would
    // emit an infinite `for (; i < n; i += 0)`. Both forms must be rejected.
    for code in [
        // dynamic range (runtime bound -> not unrolled)
        r#"
        module Top (
            n: input  logic<32>,
            o: output logic<32>,
        ) {
            always_comb {
                var acc: logic<32>;
                acc = 0;
                for i in 0..n step += 0 {
                    acc += i;
                }
                o = acc;
            }
        }
        "#,
        // static range
        r#"
        module Top (
            o: output logic<32>,
        ) {
            always_comb {
                var acc: logic<32>;
                acc = 0;
                for i in 0..10 step += 0 {
                    acc += i;
                }
                o = acc;
            }
        }
        "#,
    ] {
        let errors = analyze(code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
            "{errors:?}"
        );
    }
}

#[test]
fn no_panic_on_inclusive_range_to_usize_max() {
    // Regression: `for i in 0..=N` with N == usize::MAX computed `end + 1`,
    // overflowing (panic in dev builds; in release the bound wrapped to 0 and
    // the loop body was silently dropped from the IR while the emitted SV
    // kept the full loop).  The unroller must decline instead.
    for code in [
        r#"
        module Top (
            o: output logic<32>,
        ) {
            const N: u64 = 64'hffff_ffff_ffff_ffff;
            always_comb {
                var acc: logic<32>;
                acc = 0;
                for i in 0..=N {
                    acc += i as 32;
                }
                o = acc;
            }
        }
        "#,
        r#"
        module Top (
            o: output logic<32>,
        ) {
            const N: u64 = 64'hffff_ffff_ffff_ffff;
            always_comb {
                var acc: logic<32>;
                acc = 0;
                for i in 1..=N step *= 2 {
                    acc += i as 32;
                }
                o = acc;
            }
        }
        "#,
    ] {
        let _ = analyze(code);
    }
}

#[test]
fn for_degenerate_step_rejected() {
    // Regression: degenerate non-additive steps previously panicked the
    // compiler (`/= 0`, `%= 0` divide-by-zero; `-=` subtract overflow) or
    // silently unrolled to one iteration while the emitted SV loop was
    // infinite (`*= 1`, `<<= 0`, `*= 2` from 0, a stalling `|=`).  All of
    // them must produce a diagnostic instead.
    for step in [
        "/= 0", "%= 0", "-= 1", "/= 2", "%= 3", "&= 1", ">>= 1", "*= 1", "*= 0", "<<= 0", "|= 0",
        "^= 0",
    ] {
        let code = format!(
            r#"
            module Top (
                o: output logic<32>,
            ) {{
                always_comb {{
                    var acc: logic<32>;
                    acc = 0;
                    for i in 1..10 step {step} {{
                        acc += i;
                    }}
                    o = acc;
                }}
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
            "step {step}: {errors:?}"
        );
    }

    // Value-dependent stalls with const bounds: `*= 2` parked at 0, `|= 2`
    // stuck at 3 below the bound, `^= 1` oscillating between 3 and 2.
    for (start, step) in [("0", "*= 2"), ("1", "|= 2"), ("3", "^= 1")] {
        let code = format!(
            r#"
            module Top (
                o: output logic<32>,
            ) {{
                always_comb {{
                    var acc: logic<32>;
                    acc = 0;
                    for i in {start}..10 step {step} {{
                        acc += i;
                    }}
                    o = acc;
                }}
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
            "start {start} step {step}: {errors:?}"
        );
    }

    // Advancing steps stay accepted, including ones that stall only after
    // crossing the end of the range.
    for (start, step) in [("1", "*= 2"), ("1", "<<= 1"), ("1", "|= 12"), ("2", "^= 8")] {
        let code = format!(
            r#"
            module Top (
                o: output logic<32>,
            ) {{
                always_comb {{
                    var acc: logic<32>;
                    acc = 0;
                    for i in {start}..10 step {step} {{
                        acc += i;
                    }}
                    o = acc;
                }}
            }}
            "#
        );
        let errors = analyze(&code);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
            "start {start} step {step}: {errors:?}"
        );
    }
}

#[test]
fn no_panic_on_msb_after_unpacked_array_select() {
    // Regression: `msb` applied after an unpacked-array select indexed past the
    // packed-width Shape (the select dimension counts the unpacked array
    // dimensions too), panicking with an out-of-bounds index.
    let code = r#"
    module M (
        o: output logic,
    ) {
        var a: logic<8> [4];
        always_comb {
            a = '{0, 0, 0, 0};
            o = a[0][msb];
        }
    }
    "#;
    let _ = analyze(code);

    let code = r#"
    package P {
        const A: logic<8> [4] = '{0, 0, 0, 0};
    }
    module M (
        o: output logic,
    ) {
        assign o = P::A[0][msb];
    }
    "#;
    let _ = analyze(code);
}

#[test]
fn no_stack_overflow_on_cyclic_alias() {
    // Regression: a cyclic alias chain recursed forever while expanding the
    // alias target (ReferenceTable::generic_symbol_path / unalias /
    // TypeDag::resolve_symbol_path), overflowing the stack and aborting.
    for code in [
        // unreferenced cyclic aliases
        "alias package P1 = P2; alias package P2 = P1; module Top ( o: output logic<32> ) { always_comb { o = 0; } }",
        "alias package P1 = P2; alias package P2 = P3; alias package P3 = P1; module Top ( o: output logic<32> ) { always_comb { o = 0; } }",
        "alias module A = B; alias module B = A; module Top ( o: output logic<32> ) { always_comb { o = 0; } }",
        "alias interface A = B; alias interface B = A; module Top ( o: output logic<32> ) { always_comb { o = 0; } }",
        // cyclic aliases that are actually referenced/instantiated (must not
        // stack-overflow via resolve_inst_type / trace_type_path either)
        "alias module A = B; alias module B = A; module top ( o: output logic ) { inst u: A ( o ); assign o = 1'b0; }",
        "alias interface IA = IB; alias interface IB = IA; module top { inst u: IA; }",
        "alias package P1 = P2; alias package P2 = P1; module top ( o: output logic<8> ) { var x: P1::T; assign o = 0; assign x = 0; }",
    ] {
        let _ = analyze(code);
    }
}

#[test]
fn no_stack_overflow_on_cyclic_typedef() {
    // Regression: a cyclic typedef chain recursed forever while evaluating its
    // IR type (eval_type / Type::to_ir_type), overflowing the stack and
    // aborting (reachable via `veryl dump`, which runs pass2 best-effort even
    // after type_dag reports the cycle). Every case must report
    // CyclicTypeDependency rather than crash.
    for code in [
        "module M { type A = B; type B = A; var x: A; always_comb { let _y: logic = x; } }",
        "module M { type A = B; type B = C; type C = A; var x: A; always_comb { let _y: logic = x; } }",
        // a chain that loops back to a middle link, not the start
        "module M { type A = B; type B = C; type C = B; var x: A; always_comb { let _y: logic = x; } }",
    ] {
        let errors = analyze(code);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::CyclicTypeDependency { .. })),
            "expected CyclicTypeDependency for: {code}"
        );
    }
}

#[test]
fn no_panic_on_for_step_shift_overflow() {
    // Regression: a for-loop whose step shifts by >= the usize bit width
    // (`step <<= 64`) overflowed the native shift in Op::eval and panicked the
    // compiler during compile-time range evaluation.
    let code = r#"
    module Top (
        o: output logic<32>,
    ) {
        always_comb {
            var acc: logic<32>;
            acc = 0;
            for i in 1..100 step <<= 64 {
                acc += i;
            }
            o = acc;
        }
    }
    "#;
    let _ = analyze(code);
}

#[test]
fn wide_pow_is_masked_to_width() {
    // Regression: a BigUint `**` result was not masked to the declared width,
    // and the exponent was truncated to u32. Both must hold now: `3 ** 50`
    // folds to (3**50 mod 2**8) and a huge exponent folds via modpow rather
    // than wrapping the exponent. Here we just exercise the paths (correctness
    // of the folded value is checked end-to-end against the built binary).
    for code in [
        "module Top { const A: logic<8> = 3 ** 50; }",
        "module Top { const A: signed logic<8> = (-3) ** 7; }",
        "module Top { const A: logic<65> = 65'd2 ** 4294967297; }",
        "module Top { const A: logic<70> = 70'd3 ** 100; }",
    ] {
        let errors = analyze(code);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::TooLargeNumber { .. })),
            "masked pow should fit its width: {errors:?}"
        );
    }
}

#[test]
fn no_panic_on_resolving_symbol_path_including_alias() {
    let code = r#"
    package a_pkg::<W: u32> {
        type T = logic<W>;
    }
    package b_pkg::<W: u32> {
        alias package a = a_pkg::<W>;
    }
    package c_pkg {
        alias package b = b_pkg::<32>;
    }
    package d_pkg {
        import c_pkg::b::a::T;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn no_stack_overflow_on_importing_item_named_as_its_package() {
    // Importing an item whose name matches its parent package name used to
    // overflow the stack in `GenericSymbolPath::resolve_imported`.

    // Function: the head binds to the package, the bare name calls the function.
    let code = r#"
    package abcd {
        function abcd () -> logic {
            return 1;
        }
    }
    package xyz {
        import abcd::abcd;
        const C: logic = abcd();
    }
    "#;
    assert!(analyze(code).is_empty());

    // Struct: also a leaf with respect to `::` paths.
    let code = r#"
    package p {
        struct p {
            a: logic,
        }
    }
    package q {
        import p::p;
    }
    "#;
    assert!(analyze(code).is_empty());

    // Enum: can be a `::` prefix, so the name is genuinely ambiguous (package vs
    // imported enum); it currently errors, but must not overflow the stack.
    let code = r#"
    package e {
        enum e {
            A,
        }
    }
    package f {
        import e::e;
    }
    "#;
    let _ = analyze(code);

    // Wildcard import of a package with a same-named member must be accepted.
    let code = r#"
    package abcd {
        function abcd () -> logic {
            return 1;
        }
    }
    package wild {
        import abcd::*;
        const C: logic = abcd();
    }
    "#;
    assert!(analyze(code).is_empty());

    let code = r#"
    package p {
        struct p {
            a: logic,
        }
    }
    module top {
        import p::*;
        var v: p;
        assign v.a = 1;
    }
    "#;
    assert!(analyze(code).is_empty());
}

#[test]
fn cast_width_unevaluable_const_is_not_invalid_operand() {
    // A cast width that is a const unevaluable during analysis (here, reached
    // through an alias to a generic package instance) is not an invalid operand.
    let code = r#"
    package real_bus::<W: u32> {
        struct config_t {
            data_width: u32,
        }
        const BUS_CONFIG: config_t = config_t'{ data_width: W };
    }
    package real_dma::<W: u32> {
        alias package BUS_PKG = real_bus::<W>;
    }
    package top_pkg {
        alias package DMA_PKG = real_dma::<64>;
    }
    module ModuleA (
        o: output logic<8>,
    ) {
        const DATA_WIDTH      : u32 = top_pkg::DMA_PKG::BUS_PKG::BUS_CONFIG.data_width;
        const DATA_POS_WIDTH  : u32 = $clog2(DATA_WIDTH / 8);
        const ENTRY_BYTE_WIDTH: u32 = 4;
        function f () -> logic<DATA_POS_WIDTH> {
            var start_pos: logic<DATA_POS_WIDTH>;
            var end_pos  : logic<DATA_POS_WIDTH>;
            start_pos = 0;
            end_pos   = 0;
            for i in 0..4 {
                end_pos = start_pos + (ENTRY_BYTE_WIDTH * (2 ** i) - 1) as DATA_POS_WIDTH;
            }
            return {start_pos, end_pos};
        }
        always_comb {
            o = f();
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn cast_to_proto_typedef_is_not_unevaluable_reset_value() {
    // `0 as PKG::T` where T is a typedef of a proto-bounded generic package
    // parameter resolves, in the generic body, to the proto's bodyless type
    // (an unknown type). That is still a type cast, so it must stay on the
    // type-cast path; routing it to the value path turns the cast target into a
    // non-evaluable value and wrongly reports an unevaluable reset value.
    let code = r#"
    pub proto package proto_pkg {
        type maddr_t;
    }
    pub package impl_pkg::<W: u32> for proto_pkg {
        type maddr_t = logic<W>;
    }
    pub module Adapter::<PKG: proto_pkg> (
        i_clk : input  clock    ,
        i_rst : input  reset    ,
        o_addr: output logic<8> ,
    ) {
        var maddr: PKG::maddr_t;
        always_ff {
            if_reset {
                maddr = 0 as PKG::maddr_t;
            } else {
                maddr = maddr;
            }
        }
        assign o_addr = maddr as 8;
    }
    pub module Top (
        i_clk : input  clock    ,
        i_rst : input  reset    ,
        o_addr: output logic<8> ,
    ) {
        inst u: Adapter::<impl_pkg::<8>> (
            i_clk,
            i_rst,
            o_addr,
        );
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty(), "{errors:?}");
}

#[track_caller]
fn analyze_pass1_only(code: &str) {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let _ = analyzer.analyze_pass1("prj", &parser.veryl);
}

#[test]
fn scope_tree_matches_namespace() {
    use crate::scope;
    use crate::symbol::SymbolKind;

    let code = r#"
    package Pkg {
        const W: u32 = 8;
        struct Foo {
            a: logic,
            b: logic,
        }
        enum State: logic<2> {
            Idle,
            Run,
        }
        function add (x: input logic, y: input logic) -> logic {
            return x + y;
        }
    }

    interface If {
        var v: logic;
        modport mp {
            v: input,
        }
    }

    module Top {
        import Pkg::*;
        var x: logic<W>;
        inst u: If;
        always_comb {
            var t: logic;
            t = x;
        }
        for i in 0..2 :g {
            var y: logic;
        }
    }
    "#;

    analyze_pass1_only(code);

    let symbols = symbol_table::get_all();
    assert!(!symbols.is_empty());

    // Every symbol's scope must reconstruct exactly its namespace path.
    for symbol in &symbols {
        let path = scope::name_path(symbol.scope);
        let expected: Vec<_> = symbol.namespace.paths.iter().copied().collect();
        assert_eq!(
            path, expected,
            "scope path mismatch for symbol `{}` (id {:?}): scope={:?} namespace={}",
            symbol.token.text, symbol.id, symbol.scope, symbol.namespace
        );
    }

    // Locals integrity: every name bound in a scope must point back to a
    // symbol whose own scope is that scope and whose canonical name matches.
    for i in 0..scope::count() {
        let scope = scope::get(scope::ScopeId(i as u32)).unwrap();
        for (name, ids) in &scope.locals {
            for id in ids {
                let symbol = symbol_table::get(*id).unwrap();
                assert_eq!(
                    symbol.scope, scope.id,
                    "local `{}` (id {:?}) is bound in {:?} but its scope is {:?}",
                    symbol.token.text, id, scope.id, symbol.scope
                );
                assert_eq!(
                    veryl_parser::resource_table::canonical_str_id(symbol.token.text),
                    *name,
                    "local name key mismatch for `{}`",
                    symbol.token.text
                );
            }
        }
    }

    // Each scope-owning declaration must own the inner scope, and that scope's
    // path must equal the owner's inner namespace. Builtin symbols ($sv/$tb)
    // are registered without an owner, so restrict this to source declarations.
    for symbol in &symbols {
        if !matches!(
            symbol.token.source,
            veryl_parser::veryl_token::TokenSource::File { .. }
        ) {
            continue;
        }
        let owns = matches!(
            symbol.kind,
            SymbolKind::Module(_)
                | SymbolKind::Interface(_)
                | SymbolKind::Package(_)
                | SymbolKind::Function(_)
                | SymbolKind::Struct(_)
                | SymbolKind::Union(_)
                | SymbolKind::Enum(_)
                | SymbolKind::Modport(_)
                | SymbolKind::Block
        );
        if !owns {
            continue;
        }
        let owned = scope::intern_child(symbol.scope, symbol.token.text, scope::ScopeKind::Unknown);
        let owned_scope = scope::get(owned).unwrap();
        assert_eq!(
            owned_scope.owner,
            Some(symbol.id),
            "scope owned by `{}` has owner {:?}",
            symbol.token.text,
            owned_scope.owner
        );
        let owned_path = scope::name_path(owned);
        let expected: Vec<_> = symbol.inner_namespace().paths.iter().copied().collect();
        assert_eq!(
            owned_path, expected,
            "owned scope path mismatch for `{}`",
            symbol.token.text
        );
    }

    // The enter/exit hooks that track `current` (used to stamp references with
    // their scope) must be balanced: after the walk `current` is back at the
    // project scope, which is also where every top-level module is declared.
    let top = symbols
        .iter()
        .find(|s| matches!(s.kind, SymbolKind::Module(_)))
        .unwrap();
    assert_eq!(scope::current(), top.scope);
}

#[test]
fn project_properties() {
    let code = r#"
    module ModuleA {
        const A: i64   = PROP_A;
        const B: bbool = PROP_B;
    }
    "#;

    let mut properties = HashMap::new();
    properties.insert("PROP_A".to_string(), ProjectProperty::Int(32));
    properties.insert("PROP_B".to_string(), ProjectProperty::Bool(true));

    let errors = analyze_with_project_properties(code, properties);
    assert!(errors.is_empty());
}

#[track_caller]
fn analyze_as_dependency(code: &str) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"dep.veryl").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    context.set_project_name("dep");
    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1("dep", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
    dbg!(&errors);
    errors
}

#[test]
fn dependency_test_module_skipped() {
    // A dependency's testbench referencing its own component by bare name
    // resolves only in the dependency's context; the consumer must skip it.
    let code = r#"
    module ModuleA {}

    #[test(test_dep)]
    module test_dep {
        inst c: $comp::edge_checker;

        initial {
            $finish();
        }
    }
    "#;

    let errors = analyze_as_dependency(code);
    assert!(errors.is_empty());

    // The skipped testbench is not collected as a runnable test.
    assert!(symbol_table::get_tests("dep").is_empty());
    assert!(symbol_table::get_tests("prj").is_empty());

    // The dependency's non-test module is still analyzed.
    assert!(
        symbol_table::get_all()
            .iter()
            .any(|s| s.token.to_string() == "ModuleA")
    );

    // The same source in the project itself is analyzed as before.
    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::UnknownMember { .. }));
}

/// Analyze `code` with `components` registered as `$comp::<name>` but no
/// interface manifest seeded, so only the manifest-independent interface
/// checks fire.
#[track_caller]
fn analyze_with_components(code: &str, components: &[&str]) -> Vec<AnalyzerError> {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();
    crate::component_manifest_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    crate::tb_component::insert_external_components(components);
    let mut context = Context::default();
    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
    dbg!(&errors);
    errors
}

fn has_interface_mismatch(
    errors: &[AnalyzerError],
    expected: crate::analyzer_error::ComponentInterfaceMismatchKind,
) -> bool {
    errors.iter().any(|e| {
        matches!(
            e,
            AnalyzerError::ComponentInterfaceMismatch { kind, .. } if *kind == expected
        )
    })
}

#[test]
fn component_generic_args_need_manifest() {
    use crate::analyzer_error::ComponentInterfaceMismatchKind;

    let code = r#"
    #[test(t)]
    module t {
        var m: $comp::widget::<8>;
        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_with_components(code, &["widget"]);
    assert!(has_interface_mismatch(
        &errors,
        ComponentInterfaceMismatchKind::GenericParamsNeedManifest
    ));
}

#[test]
fn component_inst_form_rejects_generic_args() {
    use crate::analyzer_error::ComponentInterfaceMismatchKind;

    let code = r#"
    #[test(t)]
    module t {
        inst m: $comp::widget::<8> ();
        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_with_components(code, &["widget"]);
    assert!(has_interface_mismatch(
        &errors,
        ComponentInterfaceMismatchKind::InstFormUsesParen
    ));
}

#[test]
fn invalid_mixin_interface() {
    let code = r#"
    module ModuleA {}
    interface InterfaceB {
        mixin ModuleA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let code = r#"
    proto interface InterfaceA {}
    interface InterfaceB {
        mixin InterfaceA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let code = r#"
    interface InterfaceA {
        mixin InterfaceA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let code = r#"
    interface InterfaceA #(param A: u32 = 0) {}
    interface InterfaceB {
        mixin InterfaceA;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let code = r#"
    interface InterfaceA {}
    interface InterfaceB {
        mixin InterfaceA;
    }
    interface InterfaceC {
        mixin InterfaceB;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    interface InterfaceB {
        var b: logic;
        modport mp {
            b: input,
        }
    }
    interface InterfaceC {
        mixin InterfaceA;
        mixin InterfaceB;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let code = r#"
    interface InterfaceA {
        var a: logic;
        modport mp {
            a: input,
        }
    }
    interface InterfaceB {
        mixin InterfaceA;
        var b: logic;
        modport mp {
            b: input,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));

    let code = r#"
    interface InterfaceA::<W: u32> {
        var a: logic<W>;
        modport mp_a {
            a: input,
        }
    }
    interface InterfaceB::<W: u32> {
        var b: logic<W>;
        modport mp_b {
            b: input,
        }
    }
    interface InterfaceC::<W: u32> {
        mixin InterfaceA::<W>;
        mixin InterfaceB::<W>;
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    interface InterfaceA::<W: u32> {
        var a: logic<W>;
        modport mp_a {
            a: input,
        }
    }
    alias interface InterfaceA32 = InterfaceA::<32>;
    interface InterfaceB {
        mixin InterfaceA32;
        var b: logic<32>;
        modport mp_ab {
            a: input,
            b: input,
        }
    }
    "#;

    let errors = analyze(code);
    assert!(errors.is_empty());

    let code = r#"
    proto interface ProtoInterfaceA {}
    interface InterfaceA::<IF: ProtoInterfaceA> {
        mixin IF;
    }
    "#;

    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::InvalidMixin { .. }));
}

#[test]
fn pow_with_wide_exponent() {
    // `**` converted its exponent with Value::to_usize, which is None for
    // every BigUint — so a >64-bit exponent operand const-folded to X and
    // widths derived from it silently lost their downstream checks.
    let code = r#"
    module ModuleA {
        const Q: u32 = 2 ** 65'd10;
        var y: logic<Q>;
        var b: logic;
        assign y = 0;
        assign b = y[5000];
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidSelect { .. })),
        "{errors:?}"
    );
}

#[test]
fn comb_loop_through_disjoint_bits() {
    // A combinational cycle through two disjoint bits of the same variable
    // was never detected: the same-(VarId, idx) guard required the read and
    // write masks to overlap, dropping every cross-bit edge (Verilator
    // flags the emitted SV with UNOPTFLAT).
    let code = r#"
    module ModuleA (
        o_y: output logic<4>,
    ) {
        assign o_y[1] = o_y[2];
        assign o_y[2] = o_y[1];
        assign o_y[0] = 1'b0;
        assign o_y[3] = 1'b0;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "{errors:?}"
    );

    // An acyclic cross-bit chain stays accepted.
    let code = r#"
    module ModuleA (
        i  : input  logic,
        o_y: output logic<4>,
    ) {
        assign o_y[0] = i;
        assign o_y[1] = o_y[0];
        assign o_y[2] = o_y[1];
        assign o_y[3] = o_y[2];
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_interface_instance() {
    // An unannotated interface instance defaulted to ClockDomain::None,
    // which is compatible with everything — so its members laundered any
    // crossing. It now defaults to Implicit like unannotated vars.
    let code = r#"
    interface InterfaceA {
        var d: logic<8>;
        modport mp {
            d: input,
        }
    }
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_a    : input  'a logic<8>,
        o_b    : output 'b logic<8>,
    ) {
        inst bus0: InterfaceA;
        assign bus0.d = i_a;
        assign o_b = bus0.d;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain traffic through an unannotated interface stays accepted.
    let code = r#"
    interface InterfaceA {
        var d: logic<8>;
        modport mp {
            d: input,
        }
    }
    module ModuleA (
        i_clk_a: input  'a clock,
        i_a    : input  'a logic<8>,
        o_a    : output 'a logic<8>,
    ) {
        inst bus0: InterfaceA;
        assign bus0.d = i_a;
        assign o_a = bus0.d;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_lhs_select() {
    // A write index/select from another domain is the same mux CDC as the
    // data-dependent read, which is already checked on the RHS.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        idx_a  : input  'a logic<3>,
        i_b    : input  'b logic,
        o_b    : output 'b logic<8>,
    ) {
        assign o_b[idx_a] = i_b;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain write select stays accepted.
    let code = r#"
    module ModuleA (
        i_clk_b: input  'b clock,
        idx_b  : input  'b logic<3>,
        i_b    : input  'b logic,
        o_b    : output 'b logic<8>,
    ) {
        assign o_b[idx_b] = i_b;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_const_array_index() {
    // Reading a const lookup table with a foreign-domain index is the same
    // mux CDC as a variable read, but the comptime.evaluated guard skipped
    // the index check for consts/params.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        idx_a  : input  'a logic<2>,
        o_b    : output 'b logic<8>,
    ) {
        const TABLE: logic<8> [4] = '{8'h11, 8'h22, 8'h33, 8'h44};
        assign o_b = TABLE[idx_a];
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain const table reads stay accepted.
    let code = r#"
    module ModuleA (
        i_clk_b: input  'b clock,
        idx_b  : input  'b logic<2>,
        o_b    : output 'b logic<8>,
    ) {
        const TABLE: logic<8> [4] = '{8'h11, 8'h22, 8'h33, 8'h44};
        assign o_b = TABLE[idx_b];
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_concatenation_lhs() {
    // A concatenation LHS built its AssignStatement directly, skipping
    // eval_assign_statement's clock-domain checks entirely.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_a    : input  'a logic<8>,
        o_b    : output 'b logic<4>,
        o_b2   : output 'b logic<4>,
    ) {
        assign {o_b, o_b2} = i_a;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Statement form inside always_comb.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_a    : input  'a logic<8>,
        o_b    : output 'b logic<4>,
        o_b2   : output 'b logic<4>,
    ) {
        always_comb {
            {o_b, o_b2} = i_a;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // dst-vs-clock check in always_ff.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_b    : input  'b logic<8>,
        o_b    : output 'b logic<4>,
        o_b2   : output 'b logic<4>,
    ) {
        always_ff (i_clk_a) {
            {o_b, o_b2} = i_b;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain concatenation LHS stays accepted.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_a    : input  'a logic<8>,
        o_a    : output 'a logic<4>,
        o_a2   : output 'a logic<4>,
    ) {
        assign {o_a, o_a2} = i_a;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn clock_domain_statement_condition() {
    // An if/case condition gates the register write like a mux select. The
    // expression form (`r = if i_a { i_b } else { r };`) is reported, but
    // the statement form dropped the condition's clock domain entirely.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_a    : input  'a logic,
        i_b    : input  'b logic<8>,
        o_b    : output 'b logic<8>,
    ) {
        var r: 'b logic<8>;
        always_ff (i_clk_b) {
            if i_a {
                r = i_b;
            }
        }
        assign o_b = r;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Case-statement form.
    let code = r#"
    module ModuleA (
        i_clk_a: input  'a clock,
        i_clk_b: input  'b clock,
        i_a    : input  'a logic<2>,
        i_b    : input  'b logic<8>,
        o_b    : output 'b logic<8>,
    ) {
        var r: 'b logic<8>;
        always_ff (i_clk_b) {
            case i_a {
                2'd0: r = i_b;
                default: {}
            }
        }
        assign o_b = r;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );

    // Same-domain conditions stay accepted.
    let code = r#"
    module ModuleA (
        i_clk_b: input  'b clock,
        i_en   : input  'b logic,
        i_b    : input  'b logic<8>,
        o_b    : output 'b logic<8>,
    ) {
        var r: 'b logic<8>;
        always_ff (i_clk_b) {
            if i_en {
                r = i_b;
            }
        }
        assign o_b = r;
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchClockDomain { .. })),
        "{errors:?}"
    );
}

#[test]
fn baseless_literal_in_concatenation() {
    // A base-less literal is 32-bit in Veryl but emitted verbatim, so the
    // SV output contains an unsized concatenation operand — illegal per
    // LRM 11.4.12 (iverilog rejects it).
    let code = r#"
    module ModuleA {
        let a: logic<4> = 4'd1;
        var y: logic<8>;
        assign y = {a, 123};
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidUnsizedLiteral { .. })),
        "{errors:?}"
    );

    // Sized operands and base-less replication counts stay accepted.
    let code = r#"
    module ModuleA {
        let a: logic<4> = 4'd1;
        var y: logic<12>;
        assign y = {a, 8'd123};
        var z: logic<8>;
        assign z = {a repeat 2};
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidUnsizedLiteral { .. })),
        "{errors:?}"
    );
}

#[test]
fn literal_width_prefix_overflow() {
    // A width prefix beyond u32 was silently truncated modulo 2^32
    // (4294967297'h1 became IR width 1) while the emitter passes the
    // literal through verbatim.
    let code = r#"
    module ModuleA {
        const A: logic<8> = 4294967297'h1;
        var y: logic<8>;
        always_comb {
            y = A + 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TooLargeNumber { .. })),
        "{errors:?}"
    );

    // An all-bit literal with an overflowing prefix is rejected too;
    // a parse-overflow prefix (beyond usize) as well.
    let code = r#"
    module ModuleA {
        const A: logic<8> = 99999999999999999999'1;
        var y: logic<8>;
        always_comb {
            y = A + 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TooLargeNumber { .. })),
        "{errors:?}"
    );

    // Sane widths stay accepted.
    let code = r#"
    module ModuleA {
        const A: logic<8> = 8'h12;
        var y: logic<8>;
        always_comb {
            y = A + 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::TooLargeNumber { .. })),
        "{errors:?}"
    );
}

#[test]
fn loop_variable_select_width() {
    // A for-loop induction variable is const during IR unrolling, so a
    // part-select width depending on it passed the check — but the emitted
    // SV keeps a runtime `for`, where such a width is illegal (Verilator:
    // "Expecting expression to be constant").
    let code = r#"
    module ModuleA (
        o_w: output logic<32>,
    ) {
        always_comb {
            o_w = 0;
            for i in 0..4 {
                o_w[i * 8 +: i + 1] = 8'hbb;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );

    // Colon-range bounds depending on the loop variable are illegal too.
    let code = r#"
    module ModuleB (
        o_w: output logic<32>,
    ) {
        always_comb {
            o_w = 0;
            for i in 0..4 {
                o_w[i * 8 + 7 : i * 8] = 8'hbb;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );

    // A const width with a loop-variable base offset is legal SV `+:`.
    let code = r#"
    module ModuleC (
        o_w: output logic<32>,
    ) {
        always_comb {
            o_w = 0;
            for i in 0..4 {
                o_w[i * 8 +: 8] = 8'hbb;
            }
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::NonConstantSelectWidth { .. })),
        "{errors:?}"
    );
}

#[test]
fn negative_for_step() {
    // A negative additive step wraps through to_usize into a huge unsigned
    // step: the analyzer unrolled one iteration while the emitted SV
    // decrements an int for ~2^31 iterations. Reject it like step += 0.
    let code = r#"
    module ModuleA (
        o: output logic<32>,
    ) {
        always_comb {
            var s: logic<32>;
            s = 0;
            for i in 0..10 step += -1 {
                s += 1;
            }
            o = s;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
        "{errors:?}"
    );

    // A positive step stays accepted.
    let code = r#"
    module ModuleA (
        o: output logic<32>,
    ) {
        always_comb {
            var s: logic<32>;
            s = 0;
            for i in 0..10 step += 2 {
                s += 1;
            }
            o = s;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidForStep { .. })),
        "{errors:?}"
    );
}

#[test]
fn negative_for_range() {
    use crate::analyzer_error::InvalidForRangeKind;
    // A negative const range bound wraps through to_usize into a huge
    // unsigned ForBound: the analyzer silently elaborated 0 iterations
    // while the emitted SV iterates normally.
    let code = r#"
    module ModuleA (
        o: output logic<32>,
    ) {
        always_comb {
            var s: logic<32>;
            s = 0;
            for i in -2..2 {
                s += 1;
            }
            o = s;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            AnalyzerError::InvalidForRange {
                kind: InvalidForRangeKind::NegativeBound,
                ..
            }
        )),
        "{errors:?}"
    );

    // Generate-for variant.
    let code = r#"
    module ModuleB (
        o: output logic<4>,
    ) {
        for i in -2..2 :g {
            assign o[i + 2] = 1;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            AnalyzerError::InvalidForRange {
                kind: InvalidForRangeKind::NegativeBound,
                ..
            }
        )),
        "{errors:?}"
    );

    // Non-negative bounds stay accepted.
    let code = r#"
    module ModuleC (
        o: output logic<32>,
    ) {
        always_comb {
            var s: logic<32>;
            s = 0;
            for i in 0..2 {
                s += 1;
            }
            o = s;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors.iter().any(|e| matches!(
            e,
            AnalyzerError::InvalidForRange {
                kind: InvalidForRangeKind::NegativeBound,
                ..
            }
        )),
        "{errors:?}"
    );
}

#[test]
fn comptime_for_over_size_limit() {
    // A const function whose for loop exceeds evaluate_size_limit had its
    // body silently skipped, folding the constant to the pre-loop value
    // (N = 0) with no diagnostic, while the emitted SV computes 2000000.
    let code = r#"
    package PkgA {
        function count () -> u32 {
            var s: u32;
            s = 0;
            for i in 0..2000000 {
                s += 1;
            }
            return s;
        }
        const N: u32 = count();
    }
    module ModuleA (
        o: output logic<32>,
    ) {
        assign o = PkgA::N;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::ExceedLimit { .. })),
        "{errors:?}"
    );

    // A loop within the limit still folds silently.
    let code = r#"
    package PkgB {
        function count () -> u32 {
            var s: u32;
            s = 0;
            for i in 0..10 {
                s += 1;
            }
            return s;
        }
        const N: u32 = count();
    }
    module ModuleB (
        o: output logic<32>,
    ) {
        assign o = PkgB::N;
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::ExceedLimit { .. })),
        "{errors:?}"
    );
}

#[test]
fn out_of_range_select_assign() {
    // A const-function assignment through an out-of-range bit/part-select
    // reached ValueU64::assign with end >= 64, overflowing the shift and
    // panicking in debug builds *before* the invalid_select diagnostic could
    // fire. Dropping the out-of-range write lets analysis finish and report it.
    let code = r#"
    module ModuleA {
        function f () -> logic<8> {
            var v: logic<8>;
            v = 8'd0;
            v[70] = 1'b1;
            return v;
        }
        const B: logic<8> = f();
        var y: logic<8>;
        always_comb {
            y = B;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidSelect { .. })),
        "{errors:?}"
    );

    // Part-select variant.
    let code = r#"
    module ModuleA {
        function f () -> logic<8> {
            var v: logic<8>;
            v = 8'd0;
            v[71:64] = 8'hff;
            return v;
        }
        const B: logic<8> = f();
        var y: logic<8>;
        always_comb {
            y = B;
        }
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidSelect { .. })),
        "{errors:?}"
    );
}

#[test]
fn tb_random_element_type() {
    // `$tb::random::<T>` rejects a non-integer / 4-state element type with a
    // MismatchType error.
    let build = |ty: &str| {
        format!(
            r#"
    #[test(t)]
    module t {{
        var r: $tb::random::<{ty}>;
        var x: {ty};
        initial {{
            x = r.get();
            $finish();
        }}
    }}
    "#
        )
    };

    for ty in ["f32", "f64", "lbool"] {
        let errors = analyze(&build(ty));
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, AnalyzerError::MismatchType { .. })),
            "`$tb::random::<{ty}>` should be a type mismatch, got: {errors:?}"
        );
    }

    // A valid fixed integer element type raises no type mismatch.
    let errors = analyze(&build("u32"));
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchType { .. })),
        "`$tb::random::<u32>` should be accepted, got: {errors:?}"
    );

    // The element type must be at most 64 bits. A wider type is reached via a
    // `gen` type binding (`bit<N>` cannot be written as a generic argument
    // directly); 64 is the accepted boundary, 65 is rejected.
    let with_gen = |bits: u32| {
        format!(
            r#"
    #[test(t)]
    module t {{
        gen w: type = bit<{bits}>;
        var r: $tb::random::<w>;
        var x: w;
        initial {{
            x = r.get();
            $finish();
        }}
    }}
    "#
        )
    };
    let errors = analyze(&with_gen(64));
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchType { .. })),
        "`bit<64>` should be accepted, got: {errors:?}"
    );
    let errors = analyze(&with_gen(65));
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchType { .. })),
        "`bit<65>` should be a type mismatch, got: {errors:?}"
    );
}

#[test]
fn tb_random_analyze() {
    // Valid `$tb::random::<T>` usage for several element types, with its
    // methods in statement, assignment and typed-let (embedded expression)
    // positions, must pass the analyzer (the generic return type `T` resolves
    // in every position). Unused-variable lints are incidental to the shape of
    // the snippet and are ignored.
    let code = r#"
    #[test(test_random)]
    module test_random {
        var r: $tb::random::<u32>;
        var s: $tb::random::<i16>;
        var b: $tb::random::<bbool>;
        var x: u32;
        var y: i16;
        var sd: u64;
        initial {
            r.seed(42);
            x = r.get();
            x = r.get_range(0, 100);
            sd = r.get_seed();
            y = s.get();
            let z: u32 = r.get() + 1;
            $finish();
        }
    }
    "#;
    let errors: Vec<_> = analyze(code)
        .into_iter()
        .filter(|e| !matches!(e, AnalyzerError::UnusedVariable { .. }))
        .collect();
    assert!(
        errors.is_empty(),
        "expected no analyzer errors, got: {errors:?}"
    );
}

#[test]
fn maybe_driver_cross_process_conflict() {
    // A non-const index/select write has an empty definite mask, so the
    // per-bit overlap check missed it: two processes writing dynamic slices of
    // one variable passed silently, though SV rejects it (VCS ICPD, Verilator
    // MULTIDRIVEN / BLKANDNBLK) for every process-kind pair.
    let has_conflict = |code: &str| {
        analyze(code)
            .iter()
            .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. }))
    };

    // always_ff + always_comb (mixed).
    assert!(has_conflict(
        r#"
    module ModuleA (
        clk: input  clock,
        i_i: input  logic<2>,
        i_j: input  logic<2>,
        o_w: output logic<32>,
    ) {
        always_ff (clk) {
            o_w[i_i * 8+:8] = 8'haa;
        }
        always_comb {
            o_w[i_j * 8+:8] = 8'hbb;
        }
    }
    "#
    ));

    // Two always_comb (same kind).
    assert!(has_conflict(
        r#"
    module ModuleA (
        i_i: input  logic<2>,
        i_j: input  logic<2>,
        o_w: output logic<32>,
    ) {
        always_comb {
            o_w[i_i * 8+:8] = 8'haa;
        }
        always_comb {
            o_w[i_j * 8+:8] = 8'hbb;
        }
    }
    "#
    ));

    // Two always_ff (same kind).
    assert!(has_conflict(
        r#"
    module ModuleA (
        clk: input  clock,
        i_i: input  logic<2>,
        i_j: input  logic<2>,
        o_w: output logic<32>,
    ) {
        always_ff (clk) {
            o_w[i_i * 8+:8] = 8'haa;
        }
        always_ff (clk) {
            o_w[i_j * 8+:8] = 8'hbb;
        }
    }
    "#
    ));

    // A single process writing dynamic slices stays accepted.
    assert!(!has_conflict(
        r#"
    module ModuleA (
        i_i: input  logic<2>,
        o_w: output logic<32>,
    ) {
        always_comb {
            o_w = 0;
            o_w[i_i * 8+:8] = 8'haa;
        }
    }
    "#
    ));
}

#[test]
fn sv_connect_is_not_a_driving_process() {
    // An `$sv::` connection may be a read; it must not count as a second
    // driving process against a dynamic write in a real process.
    let errors = analyze(
        r#"
    module ModuleA (
        clk  : input clock,
        rst  : input reset,
        i_bin: input logic<3>,
    ) {
        var r_hist: logic<8, 32>;

        always_ff (clk, rst) {
            if_reset {
                for b in 0..8 {
                    r_hist[b] = 0;
                }
            } else {
                r_hist[i_bin] = r_hist[i_bin] + 1;
            }
        }

        inst u: $sv::Ext (
            i_x: r_hist,
        );
    }
    "#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MultipleAssignment { .. })),
        "{errors:?}"
    );
}

#[test]
fn partially_driven_output_port() {
    // The partially-driven dead-bits exemption also swallowed output
    // ports, whose bits are read externally by definition — `o[7:4]`
    // stayed X in the emitted SV with zero diagnostics.
    let code = r#"
    module ModuleA (
        o: output logic<8>,
    ) {
        assign o[3:0] = 4'h5;
    }
    "#;

    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnassignVariable { .. })),
        "{errors:?}"
    );

    // Fully driven outputs stay accepted.
    let code = r#"
    module ModuleA (
        o: output logic<8>,
    ) {
        assign o[3:0] = 4'h5;
        assign o[7:4] = 4'ha;
    }
    "#;

    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnassignVariable { .. })),
        "{errors:?}"
    );
}
