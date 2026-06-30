use crate::Parser;
use miette::Diagnostic;

#[track_caller]
fn success(code: &str) {
    let code = format!("module A {{ {} }}", code);
    let parser = Parser::parse(&code, &"");
    dbg!(code);
    assert!(parser.is_ok());
}

#[track_caller]
fn failure(code: &str) {
    let code = format!("module A {{ {} }}", code);
    let parser = Parser::parse(&code, &"");
    dbg!(code);
    assert!(parser.is_err());
}

#[track_caller]
fn help_message(code: &str) -> String {
    let parser = Parser::parse(code, &"");
    let err = parser.err().unwrap();
    err.help().unwrap().to_string()
}

#[test]
fn comment() {
    success("// aaaaa \n");
    success("/* aaaaa */");
    success("/* aa \n a \n aa */");
}

#[test]
fn line_comment_at_eof_without_newline() {
    // A file whose last line is a `// ...` comment with no trailing newline must
    // parse: the lexer's line-comment regex needs a terminating newline, which the
    // parser appends internally before lexing.
    let code = "module A {\n}\n// trailing comment, no newline";
    assert!(!code.ends_with('\n'));
    let parser = Parser::parse(code, &"");
    assert!(parser.is_ok(), "{:?}", parser.err());
}

#[test]
fn string_literal_no_unicode_escape() {
    // JSON-style `\uXXXX` escapes are intentionally unsupported: SystemVerilog
    // has no equivalent, so they are rejected at lex time rather than emitted
    // verbatim (which SV renders as the literal text `u0041`, not the char).
    failure(r#"initial { $display("x\u0041y"); }"#);
    // Two-char escapes and plain text still lex.
    success(r#"initial { $display("two\nchar"); }"#);
    success(r#"initial { $display("plain"); }"#);
}

#[test]
fn string_literal_rejects_non_sv_escapes() {
    // `\b`, `\r`, and `\/` are JSON escapes with no IEEE 1800 equivalent (like
    // `\u` above), so they are rejected at lex time rather than emitted as bytes
    // SV tools would decode differently from the simulator.
    failure(r#"initial { $display("a\bc"); }"#);
    failure(r#"initial { $display("a\rc"); }"#);
    failure(r#"initial { $display("a\/c"); }"#);
    // The SV-valid escapes still lex.
    success(r#"initial { $display("a\nb\tc\fd\"e\\f"); }"#);
}

#[test]
fn max_parsing_depth() {
    use crate::ParserError;
    use parol_runtime::ParserError as Parol;

    let nested_paren = |n: usize| {
        format!(
            "module A {{ assign o = {}1{}; }}",
            "(".repeat(n),
            ")".repeat(n)
        )
    };
    let nested_if = |n: usize| {
        format!(
            "module A {{ always_comb {{ {}b = 1;{} }} }}",
            "if a {".repeat(n),
            "}".repeat(n)
        )
    };
    let wide_case = |n: usize| {
        let arms: String = (0..n).map(|i| format!("{i}: b = 1;")).collect();
        format!("module A {{ always_comb {{ case a {{ {arms} default: b = 1; }} }} }}")
    };

    // Nesting past the limit is rejected instead of overflowing the stack.
    for code in [nested_paren(4000), nested_if(4000)] {
        let err = Parser::parse(&code, &"").unwrap_err();
        assert!(
            matches!(
                err,
                ParserError::ParserError(Parol::MaxParsingDepthExceeded { .. })
            ),
            "{err:?}"
        );
    }

    // Realistic nesting still parses (128 was the old analyzer-side limit).
    assert!(Parser::parse(&nested_paren(128), &"").is_ok());
    assert!(Parser::parse(&nested_if(128), &"").is_ok());

    // Flat lists are push productions: parol_runtime 5.0.0 excludes them from
    // the depth count, so a case with far more arms than MAX_PARSING_DEPTH still
    // parses instead of being wrongly rejected as too deep.
    assert!(Parser::parse(&wide_case(3000), &"").is_ok());
}

#[test]
fn number() {
    // integer
    success("let a: u32 = 0123456789;");
    success("let a: u32 = 0_1_23456789;");
    success("let a: u32 = _0_1_23456789;"); // identifier
    failure("let a: u32 = 0_1__23456789;");

    // binary
    success("let a: u32 = 32'b01xzXZ;");
    success("let a: u32 = 32'b01_xz_XZ;");
    failure("let a: u32 = 32'b01__xz_XZ;");

    // octal
    success("let a: u32 = 32'o01234567xzXZ;");
    success("let a: u32 = 32'o01234567_xz_XZ;");
    failure("let a: u32 = 32'o01234567__xz_XZ;");

    // decimal
    success("let a: u32 = 32'd0123456789xzXZ;");
    success("let a: u32 = 32'd0123456789_xz_XZ;");
    failure("let a: u32 = 32'd0123456789__xz_XZ;");

    // hex
    success("let a: u32 = 32'h0123456789abcdefABCDEFxzXZ;");
    success("let a: u32 = 32'h0123456789abcdefABCDEF_xz_XZ;");
    failure("let a: u32 = 32'h0123456789abcdefABCDEF__xz_XZ;");

    // all0, all1
    success("let a: u32 = '0;");
    success("let a: u32 = '1;");
    failure("let a: u32 = '2;");

    // floating point
    success("let a: u32 = 0.1;");
    success("let a: u32 = 0_1_23.4_5_67;");
    failure("let a: u32 = 0_1__23.4_5_67;");

    // exponent
    success("let a: u32 = 0.1e10;");
    success("let a: u32 = 0.1e+10;");
    success("let a: u32 = 0.1e-10;");
    success("let a: u32 = 0.1E+10;");
    success("let a: u32 = 0.1E-10;");
    failure("let a: u32 = 0.1e++10;");
    failure("let a: u32 = 0.1e10.0;");
}

#[test]
fn identifier() {
    success("var a: u32;");
    success("var _abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_: u32;");
    failure("var 0a: u32;");
}

#[test]
fn expression() {
    success("let a: u32 = 1 && 1 || 1 & 1 ^ 1 ~^ 1 ^~ 1 | 1;");
    success("let a: u32 = 1 <: 1 <= 1 >: 1 >= 1 == 1 != 1 ==? 1 !=? 1;");
    success("let a: u32 = 1 << 1 >> 1 <<< 1 >>> 1;");
    success("let a: u32 = 1 ** 1 * 1 / 1 % 1 + 1 - 1;");
    success("let a: u32 = +-!~&|^~&~|~^^~1;");
    success("let a: u32 = ( (1 && 1) || 1) & (1 ^ 1 ~^ 1) ^~ 1 | 1;");
    failure("let a: u32 = ( (1 && 1) || 1 & (1 ^ 1 ~^ 1) ^~ 1 | 1;");
}

#[test]
fn function_call() {
    success("let a: u32 = a();");
    success("let a: u32 = $a();");
    success("let a: u32 = a.a.a();");
    success("let a: u32 = a::a::a();");
    success("let a: u32 = a::a::a.a.a();");
    success("let a: u32 = a(1, 1, 1);");
    success("let a: u32 = a(1, 1, 1,);");
    failure("let a: u32 = a(1 1, 1,);");
}

#[test]
fn range() {
    success("let a: u32 = a[1];");
    success("let a: u32 = a[1:0];");
    success("let a: u32 = a[1+:1];");
    success("let a: u32 = a[1-:1];");
    success("let a: u32 = a[1 step 1];");
}

#[test]
fn r#type() {
    success("var a: logic;");
    success("var a: bit;");
    success("var a: u32;");
    success("var a: u64;");
    success("var a: i32;");
    success("var a: i64;");
    success("var a: f32;");
    success("var a: f64;");
    success("var a: a::a;");

    success("var a: logic<10, 10>;");
    success("var a: bit<10, 10>;");
    success("var a: u32[10, 10];");
    success("var a: u64[10, 10];");
    success("var a: i32[10, 10];");
    success("var a: i64[10, 10];");
    success("var a: f32[10, 10];");
    success("var a: f64[10, 10];");
    success("var a: a::a<10, 10>;");
}

#[test]
fn assignment_statement() {
    success("always_comb { a = 1; }");
    success("always_comb { a.a.a = 1; }");
    success("always_comb { a += 1; }");
    success("always_comb { a -= 1; }");
    success("always_comb { a *= 1; }");
    success("always_comb { a /= 1; }");
    success("always_comb { a %= 1; }");
    success("always_comb { a &= 1; }");
    success("always_comb { a |= 1; }");
    success("always_comb { a ^= 1; }");
    success("always_comb { a <<= 1; }");
    success("always_comb { a >>= 1; }");
    success("always_comb { a <<<= 1; }");
    success("always_comb { a >>>= 1; }");
}

#[test]
fn embed() {
    let code = r#"
    let a: logic = {{{1'b0}}};
    embed (inline) sv {{{
        initial begin
            $display("a = %0d", \{ a \});
        end
    }}}
    "#;
    success(code);
}

#[test]
fn generic_arg() {
    let code = r#"
    inst u: ModuleA::<Pkg::C[0]>;
    "#;
    failure(code);
}

#[test]
fn parse_error_help() {
    let code = r#"
    module ModuleA {
        let a: logic = 1 < 0;
    }
    "#;

    assert_eq!(
        &help_message(code),
        "If you mean \"less than operator\", please use '<:'"
    );

    let code = r#"
    module ModuleA {
        let a: logic = 1 > 0;
    }
    "#;

    assert_eq!(
        &help_message(code),
        "If you mean \"greater than operator\", please use '>:'"
    );

    let code = r#"
    module ModuleA {
        for i in 0..10 {
        }
    }
    "#;

    assert_eq!(
        &help_message(code),
        "The first arm of generate-if declaration needs label (e.g. 'if x :label {')"
    );

    let code = r#"
    module ModuleA {
        always_comb {
            case a {
                1: {e, f} = g;
            }
        }
    }
    "#;

    assert_eq!(
        &help_message(code),
        "single case statement with bit concatenation at the left-hand side is not allowed,\nplease surround it by '{}' (e.g. 'x: { {a, b} = 1; }')"
    );

    let code = r#"
    module ModuleA {
        if x {
        }
    }
    "#;

    assert_eq!(
        &help_message(code),
        "The first arm of generate-if declaration needs label (e.g. 'if x :label {')"
    );

    let code = r#"
    module ModuleA {
        var if: logic;
    }
    "#;

    assert_eq!(
        &help_message(code),
        "'if' is a reserved keyword and cannot be used as an identifier"
    );

    // `else` is valid here; the hint must address `case`, not `else`.
    let code = r#"
    module ModuleA {
        always_ff {
            if_reset {
                v = 0;
            } else case sel {
                0: v = 10;
            }
        }
    }
    "#;

    assert_eq!(
        &help_message(code),
        "'else' must be followed by a block ('{ ... }') or 'if'"
    );
}

#[test]
fn parse_error_location_points_at_divergence() {
    use crate::ParserError;

    // Regression for misleading error location: parol reports the syntax error at its
    // LA(1) (`else`, which is valid here), but the real problem is `case` at LA(2).
    let code = r#"module ModuleA {
    always_ff {
        if_reset {
            v = 0;
        } else case sel {
            0: v = 10;
        }
    }
}"#;
    let err = Parser::parse(code, &"").err().unwrap();
    let ParserError::SyntaxError(se) = err else {
        panic!("expected a syntax error");
    };

    // The reported message and the highlighted location must both be `case`.
    assert_eq!(se.to_string(), "Unexpected token: 'case'");
    let case_offset = code.find("case").unwrap();
    assert_eq!(se.error_location.offset(), case_offset);
}

#[test]
fn parse_error_shows_source_text_for_error_token() {
    use crate::ParserError;

    // `[` is not lexable inside a generic argument, so it becomes an `Error` token.
    // The message must show the offending source text (`[`), not the literal "error".
    let code = "module ModuleA { inst u: ModuleA::<Pkg::C[0]>; }";
    let err = Parser::parse(code, &"").err().unwrap();
    let ParserError::SyntaxError(se) = err else {
        panic!("expected a syntax error");
    };

    assert_eq!(se.to_string(), "Unexpected token: '['");
    assert_eq!(se.error_location.offset(), code.find('[').unwrap());
}
