use crate::Formatter;
use veryl_analyzer::{Analyzer, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn format(metadata: &Metadata, code: &str) -> String {
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(metadata);
    let mut context = Context::default();

    analyzer.analyze_pass1(&"prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, None);

    let mut formatter = Formatter::new(metadata);
    formatter.format(&parser.veryl, code);
    formatter.as_str().to_string()
}

#[test]
fn empty_body_with_comment() {
    let code = r#"module ModuleA {
    /* */
}
"#;
    let expect = r#"module ModuleA {
    /* */
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);

    let code = r#"module ModuleA {
    /* foo */
    /* bar */
}
"#;
    let expect = r#"module ModuleA {
    /* foo */
    /* bar */
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);

    let code = r#"module ModuleA {
    /* foo */
    // bar
}
"#;
    let expect = r#"module ModuleA {
    /* foo */
    // bar
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);
}

#[test]
fn empty_list() {
    let code = r#"module ModuleA #(

) (

) {

}
module ModuleB {
  inst u: ModuleA #(

    ) (

    );

    function Func (

    ) {

    }

    always_comb {
        Func(

        );
    }
}
"#;

    let expect = r#"module ModuleA #() () {}
module ModuleB {
    inst u: ModuleA ;

    function Func () {}

    always_comb {
        Func();
    }
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn skip_formatting() {
    let code = r#"#[fmt(skip)]
module ModuleA {
    let _a: logic = 0;
}

#[fmt(skip)]
interface InterfaceA {
    var a: logic;

    modport mp {
        a: input
    }
}

#[fmt(skip)]
package PackageA {
    const A: u32 = 0;

    function FuncA(
        a: input u32,
        b: input u32
    ) -> u32 {
        return a + b;
    }
}
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.format.indent_width = 2;

    let ret = format(&metadata, &code);

    println!("ret\n{}\nexp\n{}", ret, code);
    assert_eq!(ret, code);

    let code = r#"#[fmt(skip)]
module ModuleA () {
    /* this comment line is important */
}
#[fmt(skip)]
module ModuleB () {
    // this comment line is important
}
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.format.indent_width = 2;

    let ret = format(&metadata, &code);

    println!("ret\n{}\nexp\n{}", ret, code);
    assert_eq!(ret, code);
}

#[test]
fn no_panic_if_expression_when_vertical_align_off() {
    let code = r#"module ModuleA {
    let a: logic = 1;
    let _b: logic = if a == 1 ? 1 : if a == 2 ? 0 : 1;
}
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.vertical_align = false;

    let ret = format(&metadata, code);
    assert!(!ret.is_empty());
}

#[test]
fn const_above_let_alignment() {
    let code = r#"module TopModule {
    const _c: u32 = 0;
    let _a: logic = 0;
    let _abcd: logic = 0;
}
"#;

    let expect = r#"module TopModule {
    const _c   : u32   = 0;
    let   _a   : logic = 0;
    let   _abcd: logic = 0;
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

#[test]
fn format_generic_list() {
    let metadata = Metadata::create_default("prj").unwrap();

    // Short list fits on one line regardless of how the user laid it out.
    // No alignment when packed flat — the columns would not line up.
    let code = r#"module ModuleA::<A : a_type, AA: u32,> {}
"#;
    let expect = r#"module ModuleA::<A: a_type, AA: u32> {}
"#;
    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);

    // Same content laid out multi-line in the source still collapses to
    // a single line because it fits within `max_width`.
    let code = r#"module ModuleA::<
    A: a_type,
    AA: u32
> {}
"#;
    let expect = r#"module ModuleA::<A: a_type, AA: u32> {}
"#;
    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);

    let code = r#"alias module ModuleA = ModuleB::<8, 16,>;
"#;
    let expect = r#"alias module ModuleA = ModuleB::<8, 16>;
"#;
    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);

    let code = r#"alias module ModuleB = ModuleC::<
    8,
    16
>;
"#;
    let expect = r#"alias module ModuleB = ModuleC::<8, 16>;
"#;
    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);
}

#[test]
fn format_generic_list_breaks_when_too_wide() {
    // A long generic parameter list breaks into stacked-or-flat layout
    // with column alignment.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module ModuleA::<LONG_NAME: u32, OTHER_LONG: u32, ANOTHER: u32> {}
"#;
    let expect = r#"module ModuleA::<
    LONG_NAME : u32,
    OTHER_LONG: u32,
    ANOTHER   : u32,
> {}
"#;
    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

// ----- Issue #2598: format based on line width -----------------------------

#[test]
fn max_width_breaks_binary_expression() {
    // The example from https://github.com/veryl-lang/veryl/issues/2598:
    // a sum that doesn't fit on one line should wrap with operators at
    // the head of each continuation line.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 60;

    let code = r#"module M {
    let a: logic = aaaaaaaaaaaa + bbbbbbbbbb + cccccccccccc + dddddddddd + eeeeeeeeeeee;
}
"#;
    let expect = r#"module M {
    let a: logic = aaaaaaaaaaaa + bbbbbbbbbb + cccccccccccc
        + dddddddddd + eeeeeeeeeeee;
}
"#;

    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

#[test]
fn max_width_keeps_short_binary_expression_flat() {
    // A short expression must stay on one line regardless of how many
    // operands it has.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 120;

    let code = r#"module M {
    let a: logic = b + c + d;
}
"#;
    let expect = code;

    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

#[test]
fn max_width_breaks_function_call() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 30;

    let code = r#"module M {
    let _: logic = func(aaaa, bbbb, cccc, dddd);
}
"#;
    let ret = format(&metadata, code);
    // The call should wrap with each arg on its own line.
    assert!(
        ret.contains("\n        aaaa,") && ret.contains("\n    )"),
        "expected wrapped function call in:\n{ret}"
    );
}

#[test]
fn max_width_collapses_short_call_regardless_of_source_layout() {
    // A short call that the user wrote across multiple lines collapses
    // back onto one line: layout is purely width-driven, with no
    // dependency on the source's line breaks.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 200;

    let code = r#"module M {
    let _: logic = func(
        a,
        b,
    );
}
"#;
    let expect = r#"module M {
    let _: logic = func(a, b);
}
"#;
    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

#[test]
fn max_width_breaks_nested_expression() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module M {
    let _: logic = aa + bb + cc + dd + ee;
}
"#;
    let ret = format(&metadata, code);
    // Should break before some operator to fit max_width=40.
    assert!(
        ret.contains("\n        +"),
        "expected operator wrap in:\n{ret}"
    );
}

#[test]
fn max_width_keeps_short_function_call_flat() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 120;

    let code = r#"module M {
    let _: logic = f(a, b, c);
}
"#;
    let expect = code;

    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

#[test]
fn max_width_breaks_ternary() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 30;

    let code = r#"module M {
    let _: logic = if a == 1 ? bbbb : cccc;
}
"#;
    let ret = format(&metadata, code);
    // Long ternary should break at the `?` and `:` boundaries.
    assert!(
        ret.contains("?\n") && ret.contains(":\n"),
        "expected ternary wrap in:\n{ret}"
    );
}

#[test]
fn max_width_keeps_short_ternary_flat() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 120;

    let code = r#"module M {
    let _: logic = if a ? b : c;
}
"#;
    let expect = code;

    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

#[test]
fn max_width_breaks_concatenation() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 30;

    let code = r#"module M {
    let _: logic<32> = {aaaa, bbbb, cccc, dddd};
}
"#;
    let ret = format(&metadata, code);
    // The concatenation should wrap when it doesn't fit. Items may be
    // packed (fill mode) — assert that at least one break happens.
    assert!(
        ret.contains("\n        ") && ret.contains("\n    }"),
        "expected concatenation wrap in:\n{ret}"
    );
}

#[test]
fn max_width_keeps_short_concatenation_flat() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 120;

    let code = r#"module M {
    let _: logic<32> = {a, b, c, d};
}
"#;
    let expect = code;

    let ret = format(&metadata, code);
    assert_eq!(ret, expect);
}

#[test]
fn max_width_breaks_at_continuation_indent() {
    // Continuation lines indent +1 level past the surrounding statement.
    // The let statement is at module-indent (4 spaces), so the continuation
    // sits at 8 spaces.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 30;

    let code = r#"module M {
    let aaaaaaa: logic = xxxx + yyyy + zzzz + wwww;
}
"#;
    let ret = format(&metadata, code);
    // Verify a break occurred and the continuation is indented.
    assert!(
        ret.contains("\n        +"),
        "expected continuation indent of 8 spaces in:\n{ret}"
    );
}

// ----- case / switch condition (fill mode + multi-line isolation) ----------

#[test]
fn max_width_keeps_short_case_condition_flat() {
    // A short multi-key condition stays on one line and aligns the `:`
    // with sibling cases.
    let metadata = Metadata::create_default("prj").unwrap();

    let code = r#"module M {
    var a: logic;
    let x: logic = 1;
    always_comb {
        case x {
            0      : a = 1;
            1, 2, 3: a = 1;
            default: a = 1;
        }
    }
}
"#;
    let ret = format(&metadata, code);
    assert_eq!(ret, code, "got:\n{ret}");
}

#[test]
fn max_width_breaks_case_condition_fill_mode() {
    // A many-key condition wider than `max_width` wraps in fill mode:
    // multiple keys per line, packed up to the budget.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module M {
    var a: logic;
    let x: logic = 1;
    always_comb {
        case x {
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13: a = 1;
            default                                  : a = 1;
        }
    }
}
"#;
    let ret = format(&metadata, code);
    // Fill mode wraps and packs more than one key per continuation line.
    // Lines have indent + ".., .., .., ..," — at least one continuation
    // contains two commas separating three keys.
    let has_packed_line = ret.lines().any(|line| {
        line.trim_start().starts_with(|c: char| c.is_ascii_digit())
            && line.matches(',').count() >= 2
    });
    assert!(has_packed_line, "expected fill-mode packing in:\n{ret}");
    // None of the original keys disappear.
    for n in ["1,", "13:"].iter() {
        assert!(ret.contains(n), "missing key {n} in:\n{ret}");
    }
}

#[test]
fn multi_line_case_does_not_pull_default_padding() {
    // When the user writes a multi-line key list (or fill-mode breaks
    // one), the wide aligner width must not propagate to neighboring
    // case items. `default` keeps its natural width.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module M {
    var a: logic;
    let x: logic = 1;
    always_comb {
        case x {
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16: a = 1;
            default                                              : a = 1;
        }
    }
}
"#;
    let ret = format(&metadata, code);
    // `default` should NOT be padded out to a wide column. After
    // isolation, `default:` is adjacent (max one space) instead of
    // followed by tens of spaces.
    assert!(
        ret.contains("default: a = 1;"),
        "expected `default:` without wide padding in:\n{ret}"
    );
}

#[test]
fn max_width_breaks_switch_condition_fill_mode() {
    // Same fill-mode wrap behavior for switch conditions.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 50;

    let code = r#"module M {
    var c: logic;
    let z: logic<4> = 0;
    always_comb {
        switch {
            z == 1, z == 2, z == 3, z == 4, z == 5, z == 6: c = 1;
            default                                       : c = 1;
        }
    }
}
"#;
    let ret = format(&metadata, code);
    // The 6-key list cannot fit a 50-column budget on a single line
    // (indent 12 + `z == 1, ... z == 6: c = 1;` is ~50+).
    assert!(
        ret.contains(",\n            z =="),
        "expected wrapped switch condition in:\n{ret}"
    );
}

// ----- stacked-or-flat (generic / port-list style) -------------------------

#[test]
fn generic_list_stacks_with_trailing_comma_when_broken() {
    // Stacked-or-flat: when a generic parameter list breaks, every
    // item lives on its own line *and* a trailing comma is emitted
    // (the renderer's IfBreak mechanism). This guarantees idempotent
    // diff-friendly output.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 30;

    let code = r#"module M::<A: u32, B: u32, C: u32, D: u32> {}
"#;
    let ret = format(&metadata, code);
    assert!(
        ret.contains("    A: u32,\n") && ret.contains("    D: u32,\n>"),
        "expected stacked layout with trailing comma in:\n{ret}"
    );
}

// ----- fill mode (concatenation packs multiple per line) -------------------

#[test]
fn concatenation_fill_packs_multiple_per_line() {
    // Fill mode: when a concatenation breaks, neighboring items that
    // still fit stay together rather than dropping one per line.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module M {
    let _: logic<32> = {aa, bb, cc, dd, ee, ff, gg, hh};
}
"#;
    let ret = format(&metadata, code);
    // At least one continuation line carries more than one item.
    let has_packed = ret.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with(|c: char| c.is_ascii_alphabetic()) && t.matches(',').count() >= 2
    });
    assert!(has_packed, "expected fill-mode packing in:\n{ret}");
}

// ----- idempotency ---------------------------------------------------------

#[test]
fn format_is_idempotent_for_long_case_condition() {
    // Running the formatter twice must produce the same output —
    // important because the aligner uses source positions to decide
    // grouping. A non-idempotent pass would silently rewrite files
    // each `fmt` invocation.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module M {
    var a: logic;
    let x: logic = 1;
    always_comb {
        case x {
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13: a = 1;
            default: a = 1;
        }
    }
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

#[test]
fn format_is_idempotent_for_case_expression_multi_line_keys_issue_992() {
    // Regression test for veryl-lang/veryl#992.
    let metadata = Metadata::create_default("prj").unwrap();
    let code = r#"module ModuleA {
    let a: logic = case 0 {
        A: 0,
        B,
        C: 0,
        default: 0,
    };
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

#[test]
fn format_is_idempotent_for_case_statement_user_broken_keys() {
    // case_statement counterpart of the case_expression idempotency
    // regression. User-broken multi-key keys must not produce
    // different `:` columns between passes.
    let metadata = Metadata::create_default("prj").unwrap();
    let code = r#"module M {
    var a: logic;
    let x: logic = 1;
    always_comb {
        case x {
            A: a = 1;
            B,
            C: a = 1;
            default: a = 1;
        }
    }
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

#[test]
fn format_is_idempotent_for_nested_case_expression() {
    // A case expression inside a case body must remain idempotent —
    // each nesting level must keep its alignment groups stable.
    let metadata = Metadata::create_default("prj").unwrap();
    let code = r#"module M {
    let a: logic = case 0 {
        A: case 1 {
            X: 0,
            Y,
            Z: 0,
            default: 0,
        },
        B: 1,
        default: 0,
    };
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

#[test]
fn multi_line_switch_does_not_pull_default_padding() {
    // Switch counterpart of `multi_line_case_does_not_pull_default_padding`.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module M {
    var c: logic;
    let z: logic<4> = 0;
    always_comb {
        switch {
            z == 1, z == 2, z == 3, z == 4, z == 5, z == 6, z == 7, z == 8: c = 1;
            default                                                       : c = 1;
        }
    }
}
"#;
    let ret = format(&metadata, code);
    assert!(
        ret.contains("default: c = 1;"),
        "expected `default:` without wide padding in:\n{ret}"
    );
}

#[test]
fn fmt_compact_inst_stays_flat_with_force_flat() {
    // Regression test for the `single_line` stack removal.
    let metadata = Metadata::create_default("prj").unwrap();
    let code = r#"module M {
    #[fmt(compact)]
    inst u0: Sub #(
        A: 1,
        B: 2,
    ) (
        x: 1,
        y: 2,
    );
}
"#;
    let ret = format(&metadata, code);
    assert!(
        ret.contains("inst u0: Sub #( A: 1, B: 2 ) ( x: 1, y: 2 );"),
        "expected single-line compact inst in:\n{ret}"
    );
    assert!(
        !ret.contains("B: 2,)") && !ret.contains("y: 2,)"),
        "expected no trailing comma inside compact inst in:\n{ret}"
    );
}

#[test]
fn fmt_compact_inst_block_is_idempotent() {
    // Multiple `#[fmt(compact)]` insts under a block share IDENTIFIER
    // and CLOCK_DOMAIN cross-inst alignment. Re-formatting must not
    // shift columns.
    let metadata = Metadata::create_default("prj").unwrap();
    let code = r#"module M {
    #[fmt(compact)]
    {
        inst u0: Sub  #( A: 1, B: 2 ) ( x: 1, y: _ );
        inst u00: Sub #( A: 1, B: 2 ) ( x: 1, y: _ );
    }
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

#[test]
fn format_is_idempotent_for_long_binary_expression() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 50;

    let code = r#"module M {
    let _a: logic = aaaaaa + bbbbbb + cccccc + dddddd + eeeeee + ffffff;
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

// ----- #[fmt(compact)] -----------------------------------------------------

#[test]
fn fmt_compact_forces_flat_let() {
    // `#[fmt(compact)]` on a `let` keeps the right-hand side on a
    // single line even when it would otherwise wrap at `max_width`.
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 40;

    let code = r#"module M {
    #[fmt(compact)]
    let _a: logic<128> = if a == 1 ? 128'h11 : if a == 2 ? 128'h22 : 128'h33;
}
"#;
    let ret = format(&metadata, code);
    // The RHS stays on one line, no operator/`?` at line head.
    assert!(
        !ret.contains("\n        ?") && !ret.contains("\n        :"),
        "expected compact (flat) layout in:\n{ret}"
    );
}

// ----- blank line preservation --------------------------------------------

#[test]
fn preserves_single_blank_line_between_statements() {
    let metadata = Metadata::create_default("prj").unwrap();

    let code = r#"module M {
    let a: logic = 0;

    let b: logic = 0;
}
"#;
    let ret = format(&metadata, code);
    assert_eq!(ret, code, "got:\n{ret}");
}

#[test]
fn collapses_multiple_blank_lines_to_one() {
    let metadata = Metadata::create_default("prj").unwrap();

    let code = r#"module M {
    let a: logic = 0;



    let b: logic = 0;
}
"#;
    let expect = r#"module M {
    let a: logic = 0;

    let b: logic = 0;
}
"#;
    let ret = format(&metadata, code);
    assert_eq!(ret, expect, "got:\n{ret}");
}

#[test]
fn format_is_idempotent_for_assign_lhs_alignment_near_max_width() {
    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.format.max_width = 80;

    let code = r#"module M {
    var w_tag_we: logic;
    var w_miss_tag_we: logic;
    var a: logic;
    var b: logic;
    var c: logic;
    var d: logic;
    var e: logic;
    var f: logic;
    var g: logic;

    assign w_tag_we      = a & b & c & d & e & f & g & a & b & c & d & e & f;
    assign w_miss_tag_we = a & b & c;
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

#[test]
fn format_is_idempotent_for_inst_next_to_hierarchical_assign() {
    let metadata = Metadata::create_default("prj").unwrap();

    let code = r#"interface If {
    var ready: logic;
    modport mp {
        ready: input,
    }
}
module M {
    inst dst_if: If;
    inst src_if: If;
    for i in 0..2 :g_dma {
        inst u_ctrl: $sv::Connector (slave_if: src_if, master_if: dst_if);
        assign src_if.ready = 1'b1;
    }
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}

#[test]
fn format_is_idempotent_for_inst_param_next_to_var() {
    let metadata = Metadata::create_default("prj").unwrap();

    let code = r#"module M {
    var denorm_lzc: logic<6>;
    inst u_denorm_lzd: $sv::DW_lzd #(a_width: 23) (a: 0, enc: denorm_lzc, dec: _);
}
"#;
    let first = format(&metadata, code);
    let second = format(&metadata, &first);
    assert_eq!(
        first, second,
        "format is not idempotent:\nfirst:\n{first}\nsecond:\n{second}"
    );
}
