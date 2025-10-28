use crate::Parser;

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

#[test]
fn comment() {
    success("// aaaaa \n");
    success("/* aaaaa */");
    success("/* aa \n a \n aa */");
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
