use crate::conv::Context;
use crate::ir::Ir;
use crate::{Analyzer, attribute_table, symbol_table};
use similar::{ChangeTag, TextDiff};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn check_ir(code: &str, exp: &str) {
    symbol_table::clear();
    attribute_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1(&"prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2());

    dbg!(&errors);

    let ir = ir.to_string();
    let diff = TextDiff::from_lines(ir.as_str(), exp);
    for change in diff.iter_all_changes() {
        if matches!(change.tag(), ChangeTag::Insert | ChangeTag::Delete) {
            let text = &format!("{}{}", change.tag().to_string(), change);
            dbg!(text);
        }
    }

    println!("ir\n{}exp\n{}", ir, exp);

    assert!(ir.as_str() == exp);
}

#[test]
fn basic() {
    let code = r#"
    module ModuleA (
        clk: input  clock    ,
        rst: input  reset    ,
        a  : output logic    ,
        b  : output logic<32>,
    ) {
        let c: logic = a;
        var d: logic<32>;
        var e: logic;
        var f: logic;
        always_ff {
            if_reset {
                a = 0;
                b = 0;
            } else {
                a = ~a;
                b = b + 1;
            }
        }
        always_comb {
            d = b * 3;
            if f {
                e = 0;
            } else {
                e = 1;
            }
        }
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(clk): clock = 1'hx;
  input var1(rst): reset = 1'hx;
  output var2(a): logic = 1'hx;
  output var3(b): logic<32> = 32'hxxxxxxxx;
  let var4(c): logic = 1'hx;
  var var5(d): logic<32> = 32'hxxxxxxxx;
  var var6(e): logic = 1'hx;
  var var7(f): logic = 1'hx;

  comb {
    var4 = var2;
  }
  ff (var0, var1) {
    if_reset {
      var2 = 32'sh00000000;
      var3 = 32'sh00000000;
    } else {
      var2 = (~ var2);
      var3 = (var3 + 32'sh00000001);
    }
  }
  comb {
    var5 = (var3 * 32'sh00000003);
    if var7 {
      var6 = 32'sh00000000;
    } else {
      var6 = 32'sh00000001;
    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn branch() {
    let code = r#"
    module ModuleA (
        clk: input  clock    ,
        rst: input  reset    ,
        a  : output logic<32>,
        b  : output logic<32>,
        c  : input  logic<32>,
        d  : output logic<32>,
        e  : input  logic<32>,
        f  : output logic<32>,
    ) {
        var g: logic;
        var h: logic;
        var i: logic;
        always_ff {
            if_reset {
                a = 0;
            } else if g {
                a = 1;
            } else if h {
                a = 2;
            } else {
                a = 3;
            }
        }
        always_comb {
            if g {
                b = 0;
            } else if h {
                b = 1;
            } else if i {
                b = 2;
            } else {
                b = 3;
            }
            case c {
                0: d = 0;
                1: d = 1;
                2: d = 2;
                3: d = 3;
                default: d = 4;
            }
            switch {
                e == 0: f = 0;
                e >= 1: f = 1;
                e >: 2: f = 2;
                e <= 3: f = 3;
                default: f = 4;
            }
        }
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(clk): clock = 1'hx;
  input var1(rst): reset = 1'hx;
  output var2(a): logic<32> = 32'hxxxxxxxx;
  output var3(b): logic<32> = 32'hxxxxxxxx;
  input var4(c): logic<32> = 32'hxxxxxxxx;
  output var5(d): logic<32> = 32'hxxxxxxxx;
  input var6(e): logic<32> = 32'hxxxxxxxx;
  output var7(f): logic<32> = 32'hxxxxxxxx;
  var var8(g): logic = 1'hx;
  var var9(h): logic = 1'hx;
  var var10(i): logic = 1'hx;

  ff (var0, var1) {
    if_reset {
      var2 = 32'sh00000000;
    } else {
      if var8 {
        var2 = 32'sh00000001;
      } else {
        if var9 {
          var2 = 32'sh00000002;
        } else {
          var2 = 32'sh00000003;
        }
      }
    }
  }
  comb {
    if var8 {
      var3 = 32'sh00000000;
    } else {
      if var9 {
        var3 = 32'sh00000001;
      } else {
        if var10 {
          var3 = 32'sh00000002;
        } else {
          var3 = 32'sh00000003;
        }
      }
    }
    if (var4 ==? 32'sh00000000) {
      var5 = 32'sh00000000;
    } else {
      if (var4 ==? 32'sh00000001) {
        var5 = 32'sh00000001;
      } else {
        if (var4 ==? 32'sh00000002) {
          var5 = 32'sh00000002;
        } else {
          if (var4 ==? 32'sh00000003) {
            var5 = 32'sh00000003;
          } else {
            var5 = 32'sh00000004;
          }
        }
      }
    }
    if (var6 == 32'sh00000000) {
      var7 = 32'sh00000000;
    } else {
      if (var6 >= 32'sh00000001) {
        var7 = 32'sh00000001;
      } else {
        if (var6 >: 32'sh00000002) {
          var7 = 32'sh00000002;
        } else {
          if (var6 <= 32'sh00000003) {
            var7 = 32'sh00000003;
          } else {
            var7 = 32'sh00000004;
          }
        }
      }
    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn generate_if() {
    let code = r#"
    module ModuleA #(
        param N: u32 = 1,
    ) {
        if N == 0 :g {
            let a: logic = 1;
        } else {
            let b: logic = 2;
        }
    }
    module ModuleB #(
        param N: u32 = 0,
    ) {
        if N == 0 :g {
            let a: logic = 1;
        } else {
            let b: logic = 2;
        }
    }
    "#;

    let exp = r#"module ModuleA {
  param var0(N): bit<32> = 32'sh00000001;
  let var1(g.b): logic = 1'hx;

  comb {
    var1 = 32'sh00000002;
  }
}
module ModuleB {
  param var0(N): bit<32> = 32'sh00000000;
  let var1(g.a): logic = 1'hx;

  comb {
    var1 = 32'sh00000001;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn generate_for() {
    let code = r#"
    module ModuleA #(
        param N: u32 = 4,
    ) {
        for i in 0..N :g {
            let a: logic = i;
        }
    }
    "#;

    let exp = r#"module ModuleA {
  param var0(N): bit<32> = 32'sh00000004;
  const var1(g[0].i): bit<32> = 32'h00000000;
  let var2(g[0].a): logic = 1'hx;
  const var3(g[1].i): bit<32> = 32'h00000001;
  let var4(g[1].a): logic = 1'hx;
  const var5(g[2].i): bit<32> = 32'h00000002;
  let var6(g[2].a): logic = 1'hx;
  const var7(g[3].i): bit<32> = 32'h00000003;
  let var8(g[3].a): logic = 1'hx;

  comb {
    var2 = 32'h00000000;
  }
  comb {
    var4 = 32'h00000001;
  }
  comb {
    var6 = 32'h00000002;
  }
  comb {
    var8 = 32'h00000003;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn inst() {
    let code = r#"
    module ModuleA (
        i_clk: input  clock,
        i_rst: input  reset,
        i_dat: input  logic,
        o_dat: output logic,
    ) {
    }
    module ModuleB (
        i_clk: input  clock,
        i_rst: input  reset,
        i_dat: input  logic,
        o_dat: output logic,
    ) {
        inst u: ModuleA (
            i_clk,
            i_rst,
            i_dat,
            o_dat,
        );
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(i_clk): clock = 1'hx;
  input var1(i_rst): reset = 1'hx;
  input var2(i_dat): logic = 1'hx;
  output var3(o_dat): logic = 1'hx;

}
module ModuleB {
  input var0(i_clk): clock = 1'hx;
  input var1(i_rst): reset = 1'hx;
  input var2(i_dat): logic = 1'hx;
  output var3(o_dat): logic = 1'hx;

  inst u (
    var0 <- var0;
    var1 <- var1;
    var2 <- var2;
    var3 -> var3;
  ) {
    module ModuleA {
      input var0(i_clk): clock = 1'hx;
      input var1(i_rst): reset = 1'hx;
      input var2(i_dat): logic = 1'hx;
      output var3(o_dat): logic = 1'hx;

    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn system_function() {
    let code = r#"
    module ModuleA {
        let a0: logic = $clog2(0);
        let a1: logic = $clog2(1);
        let a2: logic = $clog2(2);
        let a3: logic = $clog2(3);
        let a4: logic = $clog2(4);
        let a5: logic = $clog2(5);
        const b0: u32 = $clog2(0);
        const b1: u32 = $clog2(1);
        const b2: u32 = $clog2(2);
        const b3: u32 = $clog2(3);
        const b4: u32 = $clog2(4);
        const b5: u32 = $clog2(5);
    }
    "#;

    let exp = r#"module ModuleA {
  let var0(a0): logic = 1'hx;
  let var1(a1): logic = 1'hx;
  let var2(a2): logic = 1'hx;
  let var3(a3): logic = 1'hx;
  let var4(a4): logic = 1'hx;
  let var5(a5): logic = 1'hx;
  const var6(b0): bit<32> = 32'h00000000;
  const var7(b1): bit<32> = 32'h00000000;
  const var8(b2): bit<32> = 32'h00000001;
  const var9(b3): bit<32> = 32'h00000002;
  const var10(b4): bit<32> = 32'h00000002;
  const var11(b5): bit<32> = 32'h00000003;

  comb {
    var0 = 32'h00000000;
  }
  comb {
    var1 = 32'h00000000;
  }
  comb {
    var2 = 32'h00000001;
  }
  comb {
    var3 = 32'h00000002;
  }
  comb {
    var4 = 32'h00000002;
  }
  comb {
    var5 = 32'h00000003;
  }
}
"#;

    check_ir(code, exp);

    let code = r#"
module ModuleA {
    const A: bit<65> = 65'h00000000000000000;
    const B: bit<65> = 65'h00000000000000001;
    const C: bit<65> = $signed(A + B);

    const D: bit<64> = 64'h0000000000000000;
    const E: bit<65> = 65'h00000000000000001;
    const F: bit<65> = $signed(D + E);

    const G: bit<64> = 64'h0000000000000000;
    const H: bit<64> = 64'h0000000000000001;
    const I: bit<65> = $signed(D + E);

    const J: bit<65> = 65'h00000000000000000;
    const K: bit<65> = 65'h00000000000000001;
    const L: bit<64> = $signed(A + B);
}
"#;

    let exp = r#"module ModuleA {
  const var0(A): bit<65> = 65'h00000000000000000;
  const var1(B): bit<65> = 65'h00000000000000001;
  const var2(C): bit<65> = 65'h00000000000000001;
  const var3(D): bit<64> = 64'h0000000000000000;
  const var4(E): bit<65> = 65'h00000000000000001;
  const var5(F): bit<65> = 65'h00000000000000001;
  const var6(G): bit<64> = 64'h0000000000000000;
  const var7(H): bit<64> = 64'h0000000000000001;
  const var8(I): bit<65> = 65'h00000000000000001;
  const var9(J): bit<65> = 65'h00000000000000000;
  const var10(K): bit<65> = 65'h00000000000000001;
  const var11(L): bit<64> = 64'h0000000000000001;

}
"#;

    check_ir(code, exp);
}

#[test]
fn testbench_initial_for_not_unrolled() {
    let code = r#"
    #[test(tb_sample)]
    module tb_sample {
        var a: logic<32> [4];
        initial {
            for i: u32 in 0..4 {
                a[i] = i;
            }
        }
    }
    "#;

    let exp = r#"module tb_sample {
  var var0[0](a): logic<32> = 32'hxxxxxxxx;
  var var0[1](a): logic<32> = 32'hxxxxxxxx;
  var var0[2](a): logic<32> = 32'hxxxxxxxx;
  var var0[3](a): logic<32> = 32'hxxxxxxxx;
  const var1(i): bit<32> = 32'hxxxxxxxx;

  initial {
    for i in 0..4 {
      var0[var1] = var1;
    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn comb_for() {
    let code = r#"
    module ModuleA {
        var a: logic<4>;

        always_comb {
            for i: u32 in 0..4 {
                a[i] = i + 1;
            }
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(a): logic<4> = 4'hx;
  const var1([0].i): bit<32> = 32'h00000000;
  const var2([1].i): bit<32> = 32'h00000001;
  const var3([2].i): bit<32> = 32'h00000002;
  const var4([3].i): bit<32> = 32'h00000003;

  comb {
    var0[32'h00000000] = 32'h00000001;
    var0[32'h00000001] = 32'h00000002;
    var0[32'h00000002] = 32'h00000003;
    var0[32'h00000003] = 32'h00000004;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn const_function_with_static_for() {
    let code = r#"
    module ModuleA {
        function sum() -> u32 {
            var acc: u32;
            acc = 0;
            for i: u32 in 0..5 {
                acc += i;
            }
            return acc;
        }
        const A: u32 = sum();
    }
    "#;

    symbol_table::clear();
    attribute_table::clear();
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    analyzer.analyze_pass1(&"prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir));
    Analyzer::analyze_post_pass2();
    let ir = ir.to_string();
    // sum() = 0+1+2+3+4 = 10 = 0xa
    assert!(
        ir.contains("0000000a"),
        "const A should be evaluated to 10 (0xa):\n{}",
        ir,
    );

    let code = r#"
    module ModuleA {
        function sum(n: input u32) -> u32 {
            var acc: u32;
            acc = 0;
            for i: u32 in 0..n {
                acc += i;
            }
            return acc;
        }
        const B: u32 = sum(5);
    }
    "#;

    symbol_table::clear();
    attribute_table::clear();
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    analyzer.analyze_pass1(&"prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir));
    Analyzer::analyze_post_pass2();
    let ir = ir.to_string();
    // sum(5) = 0+1+2+3+4 = 10 = 0xa
    assert!(
        ir.contains("0000000a"),
        "const B should be evaluated to 10 (0xa):\n{}",
        ir,
    );
}

#[test]
fn msb_lsb() {
    let code = r#"
    module ModuleA {
        var a: logic<4>;
        let b: logic = a[msb];
        let c: logic = a[lsb];
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(a): logic<4> = 4'hx;
  let var1(b): logic = 1'hx;
  let var2(c): logic = 1'hx;

  comb {
    var1 = var0[32'h00000003];
  }
  comb {
    var2 = var0[32'h00000000];
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn r#struct() {
    let code = r#"
    package PackageA {
        struct StructA {
            x: logic,
            y: logic,
            z: StructB,
        }
        struct StructB {
            x: logic,
            y: logic,
        }
    }
    module ModuleA (
        x: input PackageA::StructA = 2'b0101,
    ) {
        var a: PackageA::StructA;
        let b: PackageA::StructA = 1;

        assign a.x = 1;
        assign a.y = 1;
        assign a.z = 1;
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(x): struct {x: logic<1>, y: logic<1>, z: struct {x: logic<1>, y: logic<1>}} = 2'h5;
  var var1(a): struct {x: logic<1>, y: logic<1>, z: struct {x: logic<1>, y: logic<1>}} = 4'hx;
  let var2(b): struct {x: logic<1>, y: logic<1>, z: struct {x: logic<1>, y: logic<1>}} = 4'hx;

  comb {
    var2 = 32'sh00000001;
  }
  comb {
    var1[32'h00000003] = 32'sh00000001;
  }
  comb {
    var1[32'h00000002] = 32'sh00000001;
  }
  comb {
    var1[32'h00000001:32'h00000000] = 32'sh00000001;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn interface() {
    let code = r#"
    interface InterfaceA {
        var x: logic;
        var y: logic;
        function FuncB (
        ) -> logic {
            return x;
        }
    }
    module ModuleA {
        inst u0: InterfaceA;
        inst u1: InterfaceA;
        var a: logic;
        var b: logic;

        always_comb {
            a = u0.FuncB();
            b = u1.FuncB();
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(u0.x): logic = 1'hx;
  var var1(u0.y): logic = 1'hx;
  var var3(u1.x): logic = 1'hx;
  var var4(u1.y): logic = 1'hx;
  var var6(a): logic = 1'hx;
  var var7(b): logic = 1'hx;
  var var9(u0.FuncB.return): logic = 1'hx;
  var var11(u1.FuncB.return): logic = 1'hx;
  func var8(u0.FuncB) -> var9 {
    var9 = var0;
  }
  func var10(u1.FuncB) -> var11 {
    var11 = var3;
  }

  comb {
    var6 = var8();
    var7 = var10();
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn array() {
    let code = r#"
    package PackageA {
        struct StructA {
            x: logic,
            y: logic,
        }
    }
    module ModuleA {
        var a: logic<2>[3];
        var b: PackageA::StructA<2>[3];
        var c: logic;
        var d: logic;

        always_comb {
            c = a[2][1];
            d = b[2][1].x[0];
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0[0](a): logic<2> = 2'hx;
  var var0[1](a): logic<2> = 2'hx;
  var var0[2](a): logic<2> = 2'hx;
  var var1[0](b): struct {x: logic<1>, y: logic<1>}<2> = 4'hx;
  var var1[1](b): struct {x: logic<1>, y: logic<1>}<2> = 4'hx;
  var var1[2](b): struct {x: logic<1>, y: logic<1>}<2> = 4'hx;
  var var2(c): logic = 1'hx;
  var var3(d): logic = 1'hx;

  comb {
    var2 = var0[32'sh00000002][32'sh00000001];
    var3 = var1[32'sh00000002][32'h00000003];
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn function() {
    let code = r#"
    module ModuleA {
        struct StructA {
            x: logic,
            y: logic,
        }

        function FuncA (
            a: input logic,
            b: input logic,
        ) -> logic {
            var c: logic;
            c = a | b;
            return a & c;
        }

        function FuncB (
            a: input  logic,
            b: output logic,
        ) {
            b = a;
        }

        function FuncC (
            a: input StructA,
        ) -> StructA {
            return a;
        }

        var a: logic;
        var b: logic;
        var c: logic;
        var d: logic;
        var e: logic;
        var f: StructA;
        var g: StructA;

        always_comb {
            c = FuncA(a, b);
            FuncB(d, e);
            g = FuncC(f);
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var1(FuncA.return): logic = 1'hx;
  input var2(FuncA.a): logic = 1'hx;
  input var3(FuncA.b): logic = 1'hx;
  var var4(FuncA.c): logic = 1'hx;
  input var6(FuncB.a): logic = 1'hx;
  output var7(FuncB.b): logic = 1'hx;
  var var9(FuncC.return): logic<2> = 2'hx;
  input var10(FuncC.a): struct {x: logic<1>, y: logic<1>} = 2'hx;
  var var11(a): logic = 1'hx;
  var var12(b): logic = 1'hx;
  var var13(c): logic = 1'hx;
  var var14(d): logic = 1'hx;
  var var15(e): logic = 1'hx;
  var var16(f): struct {x: logic<1>, y: logic<1>} = 2'hx;
  var var17(g): struct {x: logic<1>, y: logic<1>} = 2'hx;
  func var0(FuncA) -> var1 {
    var4 = (var2 | var3);
    var1 = (var2 & var4);
  }
  func var5(FuncB) {
    var7 = var6;
  }
  func var8(FuncC) -> var9 {
    var9 = var10;
  }

  comb {
    var13 = var0(a: var11, b: var12);
    var5(a: var14, b: var15);
    var17 = var8(a: var16);
  }
}
"#;

    check_ir(code, exp);

    let code = r#"
    package Pkg {
        const A: u32 = 8;
        function func_a(a: input u32) -> u32 {
            return a + A;
        }
        function func_b(b: input u32) -> u32 {
            return func_a(b);
        }
    }
    module ModuleA {
        const A: u32 = Pkg::func_b(8);
    }
    "#;

    let exp = r#"module ModuleA {
  var var1(Pkg.func_b.return): bit<32> = 32'h00000010;
  input var2(Pkg.func_b.b): bit<32> = 32'sh00000008;
  var var4(Pkg.func_a.return): bit<32> = 32'h00000010;
  input var5(Pkg.func_a.a): bit<32> = 32'sh00000008;
  const var6(A): bit<32> = 32'h00000010;
  func var0(Pkg.func_b) -> var1 {
    var1 = var3(a: var2);
  }
  func var3(Pkg.func_a) -> var4 {
    var4 = (var5 + 32'sh00000008);
  }

}
"#;

    check_ir(code, exp);

    let code = r#"
    package PkgA {
        const A: u32 = 8;
        function func_a() -> u32 {
            return A;
        }
    }
    package PkgB {
        function func_b() -> u32 {
            return PkgA::func_a();
        }
    }
    module ModuleA {
        const A: u32 = PkgB::func_b();
    }
    "#;

    let exp = r#"module ModuleA {
  var var1(PkgB.func_b.return): bit<32> = 32'h00000008;
  var var3(PkgB.PkgA.func_a.return): bit<32> = 32'h00000008;
  const var4(A): bit<32> = 32'h00000008;
  func var0(PkgB.func_b) -> var1 {
    var1 = 32'h00000008;
  }
  func var2(PkgB.PkgA.func_a) -> var3 {
    var3 = 32'sh00000008;
  }

}
"#;

    check_ir(code, exp);

    let code = r#"
    module ModuleA {
        const A: u32 = 8;
        const B: u32 = func();
        function func() -> u32 {
            return A;
        }
    }
    "#;

    let exp = r#"module ModuleA {
  const var0(A): bit<32> = 32'sh00000008;
  var var2(func.return): bit<32> = 32'h00000008;
  const var3(B): bit<32> = 32'h00000008;
  func var1(func) -> var2 {
    var2 = var0;
  }

}
"#;

    check_ir(code, exp);

    let code = r#"
    module ModuleA (
        a: input logic<64>,
        b: input logic    ,
    ) {
        function func(
            a: input logic<64>,
            b: input logic    ,
        ) -> logic<65> {
            var ab: logic<65>;
            ab[0+:64] = a;
            ab[64]    = b;
            return ab;
        }
        let _ab: logic<65> = func(a, b);
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(a): logic<64> = 64'hxxxxxxxxxxxxxxxx;
  input var1(b): logic = 1'hx;
  var var3(func.return): logic<65> = 65'hxxxxxxxxxxxxxxxxx;
  input var4(func.a): logic<64> = 64'hxxxxxxxxxxxxxxxx;
  input var5(func.b): logic = 1'hx;
  var var6(func.ab): logic<65> = 65'hxxxxxxxxxxxxxxxxx;
  let var7(_ab): logic<65> = 65'hxxxxxxxxxxxxxxxxx;
  func var2(func) -> var3 {
    var6[32'sh00000000+:32'sh00000040] = var4;
    var6[32'sh00000040] = var5;
    var3 = var6;
  }

  comb {
    var7 = var2(a: var0, b: var1);
  }
}
"#;

    check_ir(code, exp);

    let code = r#"
    function func_ab::<W: u32>(
        a: input logic<W>,
        b: input logic<W>,
    ) -> logic<W> {
        return a + b;
    }
    function func_abc::<W: u32> (
        a: input logic<W>,
        b: input logic<W>,
        c: input logic<W>,
    ) -> logic<W> {
        return func_ab::<W>(a, b) + c;
    }
    module ModuleA #(
        param W: u32 = 8,
    )(
        a: input  logic<W>,
        b: input  logic<W>,
        c: input  logic<W>,
        d: output logic<W>,
    ) {
        assign d = func_abc::<W>(a, b, c);
    }
    "#;

    let exp = r#"module ModuleA {
  param var0(W): bit<32> = 32'sh00000008;
  input var1(a): logic<8> = 8'hxx;
  input var2(b): logic<8> = 8'hxx;
  input var3(c): logic<8> = 8'hxx;
  output var4(d): logic<8> = 8'hxx;
  var var6(func_abc::<W>.return): logic<8> = 8'hxx;
  input var7(func_abc::<W>.a): logic<8> = 8'hxx;
  input var8(func_abc::<W>.b): logic<8> = 8'hxx;
  input var9(func_abc::<W>.c): logic<8> = 8'hxx;
  var var11(func_ab::<W>.return): logic<8> = 8'hxx;
  input var12(func_ab::<W>.a): logic<8> = 8'hxx;
  input var13(func_ab::<W>.b): logic<8> = 8'hxx;
  func var5(func_abc::<W>) -> var6 {
    var6 = (var10(a: var7, b: var8) + var9);
  }
  func var10(func_ab::<W>) -> var11 {
    var11 = (var12 + var13);
  }

  comb {
    var4 = var5(c: var3, a: var1, b: var2);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn array_literal() {
    let code = r#"
    module ModuleA {
        let a: logic[2] = '{0, 1};
        var b: logic[2, 3];
        var c: logic<2, 3, 4>;

        always_comb {
            b = '{'{1, 2, 3}, '{4, 5, 6}};
            c = '{'{1, 2, 3}, 10};
        }

        const X: u32[3, 4] = '{10, 11, 12};
        const Y: bit<3, 4> = '{10, 11, 12};
    }
    "#;

    let exp = r#"module ModuleA {
  let var0[0](a): logic = 1'hx;
  let var0[1](a): logic = 1'hx;
  var var1[0](b): logic = 1'hx;
  var var1[1](b): logic = 1'hx;
  var var1[2](b): logic = 1'hx;
  var var1[3](b): logic = 1'hx;
  var var1[4](b): logic = 1'hx;
  var var1[5](b): logic = 1'hx;
  var var2(c): logic<2, 3, 4> = 24'hxxxxxx;
  const var3[0](X): bit<32> = 32'sh0000000a;
  const var3[1](X): bit<32> = 32'sh0000000b;
  const var3[2](X): bit<32> = 32'sh0000000c;
  const var4(Y): bit<3, 4> = 12'habc;

  comb {
    var0[32'h00000000] = 32'sh00000000;
    var0[32'h00000001] = 32'sh00000001;
  }
  comb {
    var1[32'h00000000][32'h00000000] = 32'sh00000001;
    var1[32'h00000000][32'h00000001] = 32'sh00000002;
    var1[32'h00000000][32'h00000002] = 32'sh00000003;
    var1[32'h00000001][32'h00000000] = 32'sh00000004;
    var1[32'h00000001][32'h00000001] = 32'sh00000005;
    var1[32'h00000001][32'h00000002] = 32'sh00000006;
    var2[32'h00000000][32'h00000000] = 32'sh00000001;
    var2[32'h00000000][32'h00000001] = 32'sh00000002;
    var2[32'h00000000][32'h00000002] = 32'sh00000003;
    var2[32'h00000001] = 32'sh0000000a;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn connect() {
    let code = r#"
    package PackageA {
        struct StructA {
            z: logic,
        }
    }
    interface InterfaceA {
        var x: logic;
        var y: PackageA::StructA;
        modport master {
            x: output,
            y: input,
        }
        modport slave {
            ..converse(master)
        }
    }
    module ModuleA {
        inst u0: InterfaceA;
        inst u1: InterfaceA;
        inst u2: InterfaceA;
        inst u3: InterfaceA;

        always_comb {
            u0.master <> u1.slave;
        }

        connect u2.master <> u3.slave;
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(u0.x): logic = 1'hx;
  var var1(u0.y): struct {z: logic<1>} = 1'hx;
  var var5(u1.x): logic = 1'hx;
  var var6(u1.y): struct {z: logic<1>} = 1'hx;
  var var10(u2.x): logic = 1'hx;
  var var11(u2.y): struct {z: logic<1>} = 1'hx;
  var var15(u3.x): logic = 1'hx;
  var var16(u3.y): struct {z: logic<1>} = 1'hx;

  comb {
    var0 = var5;
    var6 = var1;
  }
  comb {
    var10 = var15;
    var16 = var11;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn assignment_operator() {
    let code = r#"
    module ModuleA {
        var a: logic;

        always_comb {
            a += 0;
            a -= 0;
            a *= 0;
            a /= 0;
            a %= 0;
            a &= 0;
            a |= 0;
            a ^= 0;
            a <<= 0;
            a >>= 0;
            a <<<= 0;
            a >>>= 0;
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(a): logic = 1'hx;

  comb {
    var0 = (var0 + 32'sh00000000);
    var0 = (var0 - 32'sh00000000);
    var0 = (var0 * 32'sh00000000);
    var0 = (var0 / 32'sh00000000);
    var0 = (var0 % 32'sh00000000);
    var0 = (var0 & 32'sh00000000);
    var0 = (var0 | 32'sh00000000);
    var0 = (var0 ^ 32'sh00000000);
    var0 = (var0 << 32'sh00000000);
    var0 = (var0 >> 32'sh00000000);
    var0 = (var0 <<< 32'sh00000000);
    var0 = (var0 >>> 32'sh00000000);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn generic_module() {
    let code = r#"
    module ModuleA {
        inst u: ModuleB::<ModuleC>;
    }

    module ModuleB::<T: Proto> {
        inst u: T;
    }

    proto module Proto;

    module ModuleC for Proto {
        var a: logic;
    }
    "#;

    let exp = r#"module ModuleA {

  inst u (
  ) {
    module ModuleB {

      inst u (
      ) {
        module ModuleC {
          var var0(a): logic = 1'hx;

        }
      }
    }
  }
}
module ModuleB {

  inst u (
  ) {
    module Proto {

    }
  }
}
module ModuleC {
  var var0(a): logic = 1'hx;

}
"#;

    check_ir(code, exp);
}

#[test]
fn interface_function() {
    let code = r#"
    interface InterfaceA {
        var a: logic;

        function FuncA (
            x: output logic,
        ) -> logic {
            x = a;
            return a;
        }

        modport mp {
            FuncA: import,
        }
    }
    module ModuleA (
        if_a: modport InterfaceA::mp,
    ){
        inst u: InterfaceA;
        var a: logic;
        var b: logic;
        var c: logic;
        var d: logic;

        always_comb {
            a = u.FuncA(b);
            d = if_a.FuncA(c);
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var3(u.a): logic = 1'hx;
  var var6(a): logic = 1'hx;
  var var7(b): logic = 1'hx;
  var var8(c): logic = 1'hx;
  var var9(d): logic = 1'hx;
  var var11(u.FuncA.return): logic = 1'hx;
  output var12(u.FuncA.x): logic = 1'hx;
  var var14(if_a.FuncA.return): logic = 1'hx;
  output var15(if_a.FuncA.x): logic = 1'hx;
  func var10(u.FuncA) -> var11 {
    var12 = var3;
    var11 = var3;
  }
  func var13(if_a.FuncA) -> var14 {
    var15 = var1;
    var14 = var1;
  }

  comb {
    var6 = var10(x: var7);
    var9 = var13(x: var8);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn package_function() {
    let code = r#"
    package PackageA {
        function FuncA (
            a: input logic,
            b: input logic,
        ) -> logic {
            return a & b;
        }
    }
    module ModuleA {
        let a: logic = 1;
        let b: logic = 1;
        var c: logic;

        always_comb {
            c = PackageA::FuncA(a, b);
        }
    }
    "#;

    let exp = r#"module ModuleA {
  let var0(a): logic = 1'hx;
  let var1(b): logic = 1'hx;
  var var2(c): logic = 1'hx;
  var var4(PackageA.FuncA.return): logic = 1'hx;
  input var5(PackageA.FuncA.a): logic = 1'hx;
  input var6(PackageA.FuncA.b): logic = 1'hx;
  func var3(PackageA.FuncA) -> var4 {
    var4 = (var5 & var6);
  }

  comb {
    var0 = 32'sh00000001;
  }
  comb {
    var1 = 32'sh00000001;
  }
  comb {
    var2 = var3(a: var0, b: var1);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn interface_array() {
    let code = r#"
    interface InterfaceA {
        var a: logic;
        var b: logic;

        function FuncA (
        ) -> logic {
            return a;
        }

        modport mp {
            a: input,
            b: output,
            FuncA: import,
        }
    }
    module ModuleA (
        if_a: modport InterfaceA::mp[2],
    ){
        inst u: InterfaceA[2];
        var a: logic<4>;

        always_comb {
            u[0].a = if_a[0].a;
            u[1].a = if_a[1].a;
            if_a[0].b = u[0].b;
            if_a[1].b = u[1].b;
            a[0] = u[0].FuncA();
            a[1] = u[1].FuncA();
            a[2] = if_a[0].FuncA();
            a[3] = if_a[1].FuncA();
        }
    }
    "#;

    let exp = r#"module ModuleA {
  input var0[0](if_a.a): logic = 1'hx;
  input var0[1](if_a.a): logic = 1'hx;
  output var1[0](if_a.b): logic = 1'hx;
  output var1[1](if_a.b): logic = 1'hx;
  var var4[0](u.a): logic = 1'hx;
  var var4[1](u.a): logic = 1'hx;
  var var5[0](u.b): logic = 1'hx;
  var var5[1](u.b): logic = 1'hx;
  var var8(a): logic<4> = 4'hx;
  var var10[0](u.FuncA.return): logic = 1'hx;
  var var10[1](u.FuncA.return): logic = 1'hx;
  var var12[0](if_a.FuncA.return): logic = 1'hx;
  var var12[1](if_a.FuncA.return): logic = 1'hx;
  func var9[0](u.FuncA) -> var10 {
    var10[32'h00000000] = var4[32'h00000000];
  }
  func var9[1](u.FuncA) -> var10 {
    var10[32'h00000001] = var4[32'h00000001];
  }
  func var11[0](if_a.FuncA) -> var12 {
    var12[32'h00000000] = var0[32'h00000000];
  }
  func var11[1](if_a.FuncA) -> var12 {
    var12[32'h00000001] = var0[32'h00000001];
  }

  comb {
    var4[32'sh00000000] = var0[32'sh00000000];
    var4[32'sh00000001] = var0[32'sh00000001];
    var1[32'sh00000000] = var5[32'sh00000000];
    var1[32'sh00000001] = var5[32'sh00000001];
    var8[32'sh00000000] = var9[0]();
    var8[32'sh00000001] = var9[1]();
    var8[32'sh00000002] = var11[0]();
    var8[32'sh00000003] = var11[1]();
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn enum_test() {
    let code = r#"
    module ModuleA {
        enum EnumA {
            X,
            Y,
            Z,
            W,
        }

        enum EnumB {
            X = 7,
            Y = 2,
            Z = 1,
            W = 0,
        }

        var a: EnumA;
        var b: EnumA;
        var c: EnumB;
        var d: EnumB;

        always_comb {
            a = EnumA::X;
            b = EnumA::Y;
            c = EnumB::X;
            d = EnumB::Y;
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(a): enum {logic<2>} = 2'hx;
  var var1(b): enum {logic<2>} = 2'hx;
  var var2(c): enum {logic<3>} = 3'hx;
  var var3(d): enum {logic<3>} = 3'hx;

  comb {
    var0 = 2'h0;
    var1 = 2'h1;
    var2 = 32'sh00000007;
    var3 = 32'sh00000002;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn generic_function() {
    let code = r#"
    module ModuleA {
        function FuncA::<N: u32> (
            x: input logic<N>,
        ) -> logic<N> {
            return x + 1;
        }

        let a: logic<10> = 1;
        var b: logic<10>;

        always_comb {
            b = FuncA::<10>(a);
        }
    }
    "#;

    let exp = r#"module ModuleA {
  let var2(a): logic<10> = 10'hxxx;
  var var3(b): logic<10> = 10'hxxx;
  var var5(FuncA::<10>.return): logic<10> = 10'hxxx;
  input var6(FuncA::<10>.x): logic<10> = 10'hxxx;
  func var4(FuncA::<10>) -> var5 {
    var5 = (var6 + 32'sh00000001);
  }

  comb {
    var2 = 32'sh00000001;
  }
  comb {
    var3 = var4(x: var2);
  }
}
"#;

    check_ir(code, exp);

    let code = r#"
    module ModuleA {
        function func_a::<W: u32>(a: input bit<W>) -> bit<W> {
            return a;
        }
        function func_b(a: input bit<32>, b: input bit<32>) -> bit<32> {
            return func_a::<32>(a) + b;
        }
        function func_c(a: input bit<32>, c: input bit<32>) -> bit<32> {
            return func_a::<32>(a) + c;
        }
        function func_d(a: input bit<16>, d: input bit<16>) -> bit<16> {
            return func_a::<16>(a) + d;
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var3(func_b.return): bit<32> = 32'hxxxxxxxx;
  input var4(func_b.a): bit<32> = 32'hxxxxxxxx;
  input var5(func_b.b): bit<32> = 32'hxxxxxxxx;
  var var7(func_a::<32>.return): bit<32> = 32'hxxxxxxxx;
  input var8(func_a::<32>.a): bit<32> = 32'hxxxxxxxx;
  var var10(func_c.return): bit<32> = 32'hxxxxxxxx;
  input var11(func_c.a): bit<32> = 32'hxxxxxxxx;
  input var12(func_c.c): bit<32> = 32'hxxxxxxxx;
  var var14(func_d.return): bit<16> = 16'hxxxx;
  input var15(func_d.a): bit<16> = 16'hxxxx;
  input var16(func_d.d): bit<16> = 16'hxxxx;
  var var18(func_a::<16>.return): bit<16> = 16'hxxxx;
  input var19(func_a::<16>.a): bit<16> = 16'hxxxx;
  func var2(func_b) -> var3 {
    var3 = (var6(a: var4) + var5);
  }
  func var6(func_a::<32>) -> var7 {
    var7 = var8;
  }
  func var9(func_c) -> var10 {
    var10 = (var6(a: var11) + var12);
  }
  func var13(func_d) -> var14 {
    var14 = (var17(a: var15) + var16);
  }
  func var17(func_a::<16>) -> var18 {
    var18 = var19;
  }

}
"#;

    check_ir(code, exp);
}

#[test]
fn complex_function_arg() {
    let code = r#"
    package PackageA {
        struct StructA {
            x: logic,
            y: logic,
        }
    }
    interface InterfaceA {
        var a: logic;
        var b: PackageA::StructA;

        modport mp {
            a: input,
            b: output,
        }
    }
    module ModuleA {
        function FuncA (
            x: modport InterfaceA::mp,
        ) -> logic {
            x.b = 0;
            return x.a;
        }

        inst u: InterfaceA;
        var a: logic;

        always_comb {
            a = FuncA(u);
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var1(FuncA.return): logic = 1'hx;
  input var2(FuncA.x.a): logic = 1'hx;
  output var3(FuncA.x.b): struct {x: logic<1>, y: logic<1>} = 2'h0;
  var var6(u.a): logic = 1'hx;
  var var7(u.b): struct {x: logic<1>, y: logic<1>} = 2'hx;
  var var10(a): logic = 1'hx;
  func var0(FuncA) -> var1 {
    var3 = 32'sh00000000;
    var1 = var2;
  }

  comb {
    var10 = var0(x.a: var6, x.b: var7);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn struct_constructor() {
    let code = r#"
    package PackageA {
        struct StructA {
            x: logic<32>,
            y: logic<32>,
        }
        struct StructB {
            v: StructA,
            w: StructA,
        }
    }
    module ModuleA {
        var a: PackageA::StructB;
        var b: PackageA::StructB;

        always_comb {
            a = PackageA::StructB'{
                v: PackageA::StructA'{ x: 1, y: 2 },
                w: PackageA::StructA'{ x: 3, y: 4 },
            };
            b = PackageA::StructB'{
                v: PackageA::StructA'{ x: 1, y: 64'hffffffffffff },
                ..default('0)
            };
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(a): struct {v: struct {x: logic<32>, y: logic<32>}, w: struct {x: logic<32>, y: logic<32>}} = 128'hxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx;
  var var1(b): struct {v: struct {x: logic<32>, y: logic<32>}, w: struct {x: logic<32>, y: logic<32>}} = 128'hxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx;

  comb {
    var0 = 128'h00000001000000020000000300000004;
    var1 = 128'h00000001ffffffff0000000000000000;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn struct_port() {
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

        var a: StructA [2];
        var c: StructA    ;
        var d: StructA <2>;

        assign c    = 0;
        assign a[1] = c;
        assign d[1] = d[0];

        inst u0: ModuleA #(
            A: StructA,
        ) (
            o: a[0],
        );

        inst u1: ModuleA #(
            A: StructA,
        ) (
            o: d[0],
        );
    }
    "#;

    let exp = r#"module ModuleA {
  output var1(o): logic = 1'hx;

  comb {
    var1 = '0;
  }
}
module ModuleB {
  var var0[0](a): struct {x: logic<1>, y: logic<1>} = 2'hx;
  var var0[1](a): struct {x: logic<1>, y: logic<1>} = 2'hx;
  var var1(c): struct {x: logic<1>, y: logic<1>} = 2'hx;
  var var2(d): struct {x: logic<1>, y: logic<1>}<2> = 4'hx;

  comb {
    var1 = 32'sh00000000;
  }
  comb {
    var0[32'sh00000001] = var1;
  }
  comb {
    var2[32'h00000003:32'h00000002] = var2[32'h00000001:32'h00000000];
  }
  inst u0 (
    var1 -> var0[32'sh00000000];
  ) {
    module ModuleA {
      output var1(o): struct {x: logic<1>, y: logic<1>} = 2'hx;

      comb {
        var1 = '0;
      }
    }
  }
  inst u1 (
    var1 -> var2[32'h00000001:32'h00000000];
  ) {
    module ModuleA {
      output var1(o): struct {x: logic<1>, y: logic<1>} = 2'hx;

      comb {
        var1 = '0;
      }
    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn string() {
    let code = r#"
    module ModuleA {
        const S: string = "X";
        const T: string = "Y";

        if S == "X" :g1 {
            var _a: logic;
        } else {
            var _b: logic;
        }

        if S != "X" :g2 {
            var _c: logic;
        } else {
            var _d: logic;
        }

        if S == T :g3 {
            var _e: logic;
        } else {
            var _f: logic;
        }

        if S != T :g4 {
            var _g: logic;
        } else {
            var _h: logic;
        }

        if S == "X" || T == "Y" :g5 {
            var _i: logic;
        } else {
            var _j: logic;
        }
    }
    "#;

    let exp = r#"module ModuleA {
  const var0(S): string = 8'h58;
  const var1(T): string = 8'h59;
  var var2(g1._a): logic = 1'hx;
  var var3(g2._d): logic = 1'hx;
  var var4(g3._f): logic = 1'hx;
  var var5(g4._g): logic = 1'hx;
  var var6(g5._i): logic = 1'hx;

}
"#;

    check_ir(code, exp);
}

#[test]
fn union() {
    let code = r#"
    module ModuleA {
        union UnionA {
            x: StructA<2>,
            y: StructB,
            z: logic<2, 4>,
        }

        struct StructA {
            v: logic,
            w: logic<3>,
        }

        struct StructB {
            s: logic<6>,
            t: logic<2>,
        }

        var a: UnionA;
        var b: UnionA<2>;
        var c: UnionA<2>[2];

        always_comb {
            a.x[0].w         = a.x[0].w        ;
            a.x[0].v         = a.x[0].v        ;
            a.x[1].w         = a.x[1].w        ;
            a.x[1].v         = a.x[1].v        ;
            a.y.t            = a.y.t           ;
            a.y.s            = a.y.s           ;
            a.z              = a.z             ;
            a.x[0].w[0]      = a.x[0].w[0]     ;
            a.x[0].v[0]      = a.x[0].v[0]     ;
            a.x[1].w[0]      = a.x[1].w[0]     ;
            a.x[1].v[0]      = a.x[1].v[0]     ;
            a.y.t   [0]      = a.y.t   [0]     ;
            a.y.s   [0]      = a.y.s   [0]     ;
            a.z     [0]      = a.z     [0]     ;
            a.z     [0][0]   = a.z     [0][0]  ;
            a.x[0].w[1:0]    = a.x[0].w[1:0]   ;
            a.x[0].v[0:0]    = a.x[0].v[0:0]   ;
            a.x[1].w[1:0]    = a.x[1].w[1:0]   ;
            a.x[1].v[0:0]    = a.x[1].v[0:0]   ;
            a.y.t   [1:0]    = a.y.t   [1:0]   ;
            a.y.s   [1:0]    = a.y.s   [1:0]   ;
            a.z     [1:0]    = a.z     [1:0]   ;
            a.z     [0][1:0] = a.z     [0][1:0];
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(a): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>} = 8'hxx;
  var var1(b): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>}<2> = 16'hxxxx;
  var var2[0](c): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>}<2> = 16'hxxxx;
  var var2[1](c): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>}<2> = 16'hxxxx;

  comb {
    var0[32'h00000002:32'h00000000] = var0[32'h00000002:32'h00000000];
    var0[32'h00000003] = var0[32'h00000003];
    var0[32'h00000006:32'h00000004] = var0[32'h00000006:32'h00000004];
    var0[32'h00000007] = var0[32'h00000007];
    var0[32'h00000001:32'h00000000] = var0[32'h00000001:32'h00000000];
    var0[32'h00000007:32'h00000002] = var0[32'h00000007:32'h00000002];
    var0[32'h00000007:32'h00000000] = var0[32'h00000007:32'h00000000];
    var0[32'h00000000] = var0[32'h00000000];
    var0[32'h00000003] = var0[32'h00000003];
    var0[32'h00000004] = var0[32'h00000004];
    var0[32'h00000007] = var0[32'h00000007];
    var0[32'h00000000] = var0[32'h00000000];
    var0[32'h00000002] = var0[32'h00000002];
    var0[32'h00000003:32'h00000000] = var0[32'h00000003:32'h00000000];
    var0[32'h00000000] = var0[32'h00000000];
    var0[32'h00000001:32'h00000000] = var0[32'h00000001:32'h00000000];
    var0[32'h00000003] = var0[32'h00000003];
    var0[32'h00000005:32'h00000004] = var0[32'h00000005:32'h00000004];
    var0[32'h00000007] = var0[32'h00000007];
    var0[32'h00000001:32'h00000000] = var0[32'h00000001:32'h00000000];
    var0[32'h00000003:32'h00000002] = var0[32'h00000003:32'h00000002];
    var0[32'h00000007:32'h00000000] = var0[32'h00000007:32'h00000000];
    var0[32'h00000001:32'h00000000] = var0[32'h00000001:32'h00000000];
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn generics_resolve() {
    let code = r#"
    pub proto package ProtoPkg {
        const WIDTH: u32;
        type Type = logic<WIDTH>;
    }

    pub package Pkg::<X: u32> for ProtoPkg {
        const WIDTH: u32 = X;
        type Type = logic<WIDTH>;
    }

    module ModuleA::<PKG: ProtoPkg> (
        i_clk   : input clock,
        i_rst   : input reset,
    ) {
        import PKG::*;

        struct StructA {
            a: logic,
            b: Type ,
        }

        var a: logic  ;
        var b: StructA;

        always_ff {
            if_reset {
                b = '0;
            } else {
                b.a = a[msb];
                b.b = 0;
            }
        }
    }

    module ModuleB {
        inst u: ModuleA::<Pkg::<32>> (
            i_clk: '0,
            i_rst: '0,
        );
    }
"#;

    let exp = r#"module ModuleA {
  input var0(i_clk): clock = 1'hx;
  input var1(i_rst): reset = 1'hx;
  var var2(a): logic = 1'hx;


  ff (var0, var1) {
    if_reset {
      var3 = '0;
    }
  }
}
module ModuleB {

  inst u (
    var0 <- '0;
    var1 <- '0;
  ) {
    module ModuleA {
      input var0(i_clk): clock = 1'hx;
      input var1(i_rst): reset = 1'hx;
      var var2(a): logic = 1'hx;
      var var3(b): struct {a: logic<1>, b: logic<32>} = 33'hxxxxxxxxx;

      ff (var0, var1) {
        if_reset {
          var3 = '0;
        } else {
          var3[32'h00000020] = var2[32'h00000000];
          var3[32'h0000001f:32'h00000000] = 32'sh00000000;
        }
      }
    }
  }
}
"#;

    check_ir(code, exp);

    let code = r#"
    function func_a::<A: u32> -> u32 {
        return A;
    }
    package Pkg::<A: u32> {
        const B: u32 = A;
        function func_b() -> u32 {
            return func_a::<B>();
        }
    }
    module ModuleA {
        const A: u32 = Pkg::<1>::func_b();
    }
    "#;

    let exp = r#"module ModuleA {
  var var1(Pkg.func_b::<1>.return): bit<32> = 32'h00000001;
  var var3(Pkg.func_a::<__Pkg__1 B>.return): bit<32> = 32'h00000001;
  const var4(A): bit<32> = 32'h00000001;
  func var0(Pkg.func_b::<1>) -> var1 {
    var1 = 32'h00000001;
  }
  func var2(Pkg.func_a::<__Pkg__1 B>) -> var3 {
    var3 = 32'sh00000001;
  }

}
"#;

    check_ir(code, exp);

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

    let exp = r#"module ModuleB {


  comb {
    var0 = '0;
  }
}
module ModuleC {

  inst u (
  ) {
    module ModuleB {
      let var0(_a): logic<2> = 2'hx;

      comb {
        var0 = '0;
      }
    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn generics_resolve2() {
    let code = r#"
    proto package ProtoPkg {
        const TYPE: type;
    }

    package PackageA::<X: type = lbool,> for ProtoPkg {
        const TYPE: type = X;
    }

    module ModuleA::<PKG: ProtoPkg> () {
        import PKG::*;
        let a : TYPE  = 1;
        let _b: logic = a;
    }

    module ModuleB {
        inst u0: ModuleA::<PackageA::<>>;
        inst u1: ModuleA::<PackageA::<bbool>>;
    }
    "#;

    let exp = r#"module ModuleA {
  let var0(a): unknown = 1'hx;
  let var1(_b): logic = 1'hx;

  comb {
    var0 = 32'sh00000001;
  }
  comb {
    var1 = var0;
  }
}
module ModuleB {

  inst u0 (
  ) {
    module ModuleA {
      let var0(a): logic = 1'hx;
      let var1(_b): logic = 1'hx;

      comb {
        var0 = 32'sh00000001;
      }
      comb {
        var1 = var0;
      }
    }
  }
  inst u1 (
  ) {
    module ModuleA {
      let var0(a): bit = 1'hx;
      let var1(_b): logic = 1'hx;

      comb {
        var0 = 32'sh00000001;
      }
      comb {
        var1 = var0;
      }
    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn type_param_preserves_width() {
    let code = r#"
    module Inner::<T: type> () {
        var a: T;
        assign a = '0;
    }

    module Outer #(
        param T: type = logic<8>,
    ) () {
        inst u: Inner::<T>;
    }

    module Top {
        inst u: Outer;
    }
    "#;

    let exp = r#"module Inner {
  var var0(a): unknown = 1'hx;

  comb {
    var0 = '0;
  }
}
module Outer {

  inst u (
  ) {
    module Inner {
      var var0(a): logic<8> = 8'hxx;

      comb {
        var0 = '0;
      }
    }
  }
}
module Top {

  inst u (
  ) {
    module Outer {

      inst u (
      ) {
        module Inner {
          var var0(a): logic<8> = 8'hxx;

          comb {
            var0 = '0;
          }
        }
      }
    }
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn concat() {
    let code = r#"
  module Top (
    o: output logic<16>,
    o2: output logic<16>,
) {
    assign o = {8'hff + 8'h1};
    assign o2 = {8'hf0, 8'hff + 8'h1};
}
    "#;
    let exp = r#"module Top {
  output var0(o): logic<16> = 16'hxxxx;
  output var1(o2): logic<16> = 16'hxxxx;

  comb {
    var0 = 8'h00;
  }
  comb {
    var1 = 16'hf000;
  }
}
"#;
    check_ir(code, exp);
}

#[test]
fn proto_function() {
    let code = r#"
    proto package Element {
        type data;
        function gt(a: input data, b: input data) -> logic;
    }
    package IntElement for Element {
        type data = logic<8>;
        function gt(a: input data, b: input data) -> logic {
            return a >: b;
        }
    }
    module ModuleA::<E: Element> (
        a: input  E::data,
        b: input  E::data,
        r: output logic,
    ) {
        always_comb {
            r = E::gt(a, b);
        }
    }
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        r: output logic,
    ) {
        inst inner: ModuleA::<IntElement> (a, b, r);
    }
    "#;
    let exp = r#"module ModuleA {
  input var0(a): unknown = 1'hx;
  input var1(b): unknown = 1'hx;
  output var2(r): logic = 1'hx;

  comb {
    var2 = unknown;
  }
}
module Top {
  input var0(a): logic<8> = 8'hxx;
  input var1(b): logic<8> = 8'hxx;
  output var2(r): logic = 1'hx;

  inst inner (
    var0 <- var0;
    var1 <- var1;
    var2 -> var2;
  ) {
    module ModuleA {
      input var0(a): logic<8> = 8'hxx;
      input var1(b): logic<8> = 8'hxx;
      output var2(r): logic = 1'hx;
      var var4(E.gt.return): logic = 1'hx;
      input var5(E.gt.a): logic<8> = 8'hxx;
      input var6(E.gt.b): logic<8> = 8'hxx;
      func var3(E.gt) -> var4 {
        var4 = (var5 >: var6);
      }

      comb {
        var2 = var3(a: var0, b: var1);
      }
    }
  }
}
"#;
    check_ir(code, exp);
}

#[test]
fn binary_operation_with_large_width_variable() {
    let code = r#"
module Top (
  a: input  logic<65>,
  b: output logic    ,
) {
  always_comb {
    b = a == '0;
  }
}
"#;

    let exp = r#"module Top {
  input var0(a): logic<65> = 65'hxxxxxxxxxxxxxxxxx;
  output var1(b): logic = 1'hx;

  comb {
    var1 = (var0 == '0);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn assignment_operator_with_array_index() {
    let code = r#"
    module Top #(
        param N: u32 = 4,
    ) (
        a: input  logic [N],
        b: output logic<32> [N],
    ) {
        var score: logic<32> [N];
        always_comb {
            for i: u32 in 0..N {
                score[i] = 0;
                for j: u32 in 0..N {
                    score[i] += a[j];
                }
            }
            for i: u32 in 0..N {
                b[i] = score[i];
            }
        }
    }
    "#;

    let exp = r#"module Top {
  param var0(N): bit<32> = 32'sh00000004;
  input var1[0](a): logic = 1'hx;
  input var1[1](a): logic = 1'hx;
  input var1[2](a): logic = 1'hx;
  input var1[3](a): logic = 1'hx;
  output var2[0](b): logic<32> = 32'hxxxxxxxx;
  output var2[1](b): logic<32> = 32'hxxxxxxxx;
  output var2[2](b): logic<32> = 32'hxxxxxxxx;
  output var2[3](b): logic<32> = 32'hxxxxxxxx;
  var var3[0](score): logic<32> = 32'hxxxxxxxx;
  var var3[1](score): logic<32> = 32'hxxxxxxxx;
  var var3[2](score): logic<32> = 32'hxxxxxxxx;
  var var3[3](score): logic<32> = 32'hxxxxxxxx;
  const var4([0].i): bit<32> = 32'h00000000;
  const var5([0].[0].j): bit<32> = 32'h00000000;
  const var6([0].[1].j): bit<32> = 32'h00000001;
  const var7([0].[2].j): bit<32> = 32'h00000002;
  const var8([0].[3].j): bit<32> = 32'h00000003;
  const var9([1].i): bit<32> = 32'h00000001;
  const var10([1].[0].j): bit<32> = 32'h00000000;
  const var11([1].[1].j): bit<32> = 32'h00000001;
  const var12([1].[2].j): bit<32> = 32'h00000002;
  const var13([1].[3].j): bit<32> = 32'h00000003;
  const var14([2].i): bit<32> = 32'h00000002;
  const var15([2].[0].j): bit<32> = 32'h00000000;
  const var16([2].[1].j): bit<32> = 32'h00000001;
  const var17([2].[2].j): bit<32> = 32'h00000002;
  const var18([2].[3].j): bit<32> = 32'h00000003;
  const var19([3].i): bit<32> = 32'h00000003;
  const var20([3].[0].j): bit<32> = 32'h00000000;
  const var21([3].[1].j): bit<32> = 32'h00000001;
  const var22([3].[2].j): bit<32> = 32'h00000002;
  const var23([3].[3].j): bit<32> = 32'h00000003;
  const var24([0].i): bit<32> = 32'h00000000;
  const var25([1].i): bit<32> = 32'h00000001;
  const var26([2].i): bit<32> = 32'h00000002;
  const var27([3].i): bit<32> = 32'h00000003;

  comb {
    var3[32'h00000000] = 32'sh00000000;
    var3[32'h00000000] = (var3[32'h00000000] + var1[32'h00000000]);
    var3[32'h00000000] = (var3[32'h00000000] + var1[32'h00000001]);
    var3[32'h00000000] = (var3[32'h00000000] + var1[32'h00000002]);
    var3[32'h00000000] = (var3[32'h00000000] + var1[32'h00000003]);
    var3[32'h00000001] = 32'sh00000000;
    var3[32'h00000001] = (var3[32'h00000001] + var1[32'h00000000]);
    var3[32'h00000001] = (var3[32'h00000001] + var1[32'h00000001]);
    var3[32'h00000001] = (var3[32'h00000001] + var1[32'h00000002]);
    var3[32'h00000001] = (var3[32'h00000001] + var1[32'h00000003]);
    var3[32'h00000002] = 32'sh00000000;
    var3[32'h00000002] = (var3[32'h00000002] + var1[32'h00000000]);
    var3[32'h00000002] = (var3[32'h00000002] + var1[32'h00000001]);
    var3[32'h00000002] = (var3[32'h00000002] + var1[32'h00000002]);
    var3[32'h00000002] = (var3[32'h00000002] + var1[32'h00000003]);
    var3[32'h00000003] = 32'sh00000000;
    var3[32'h00000003] = (var3[32'h00000003] + var1[32'h00000000]);
    var3[32'h00000003] = (var3[32'h00000003] + var1[32'h00000001]);
    var3[32'h00000003] = (var3[32'h00000003] + var1[32'h00000002]);
    var3[32'h00000003] = (var3[32'h00000003] + var1[32'h00000003]);
    var2[32'h00000000] = var3[32'h00000000];
    var2[32'h00000001] = var3[32'h00000001];
    var2[32'h00000002] = var3[32'h00000002];
    var2[32'h00000003] = var3[32'h00000003];
  }
}
"#;

    check_ir(code, exp);

    let code = r#"
    module ModuleA {
        var a: logic<2*4>[2];

        for i in 0..8 :g {
            always_comb {
                a[i[2]][2*i[1:0]+:2] = '0;
            }
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0[0](a): logic<8> = 8'hxx;
  var var0[1](a): logic<8> = 8'hxx;
  const var1(g[0].i): bit<32> = 32'h00000000;
  const var2(g[1].i): bit<32> = 32'h00000001;
  const var3(g[2].i): bit<32> = 32'h00000002;
  const var4(g[3].i): bit<32> = 32'h00000003;
  const var5(g[4].i): bit<32> = 32'h00000004;
  const var6(g[5].i): bit<32> = 32'h00000005;
  const var7(g[6].i): bit<32> = 32'h00000006;
  const var8(g[7].i): bit<32> = 32'h00000007;

  comb {
    var0[1'h0][32'h00000000+:32'sh00000002] = '0;
  }
  comb {
    var0[1'h0][32'h00000002+:32'sh00000002] = '0;
  }
  comb {
    var0[1'h0][32'h00000004+:32'sh00000002] = '0;
  }
  comb {
    var0[1'h0][32'h00000006+:32'sh00000002] = '0;
  }
  comb {
    var0[1'h1][32'h00000000+:32'sh00000002] = '0;
  }
  comb {
    var0[1'h1][32'h00000002+:32'sh00000002] = '0;
  }
  comb {
    var0[1'h1][32'h00000004+:32'sh00000002] = '0;
  }
  comb {
    var0[1'h1][32'h00000006+:32'sh00000002] = '0;
  }
}
"#;

    check_ir(code, exp);

    let code = r#"
    module ModuleA {
        var a: logic<2*4>[2];
        always_comb {
            for i: u32 in 0..8 {
                a[i[2]][2*i[1:0]+:2] = '0;
            }
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0[0](a): logic<8> = 8'hxx;
  var var0[1](a): logic<8> = 8'hxx;
  const var1([0].i): bit<32> = 32'h00000000;
  const var2([1].i): bit<32> = 32'h00000001;
  const var3([2].i): bit<32> = 32'h00000002;
  const var4([3].i): bit<32> = 32'h00000003;
  const var5([4].i): bit<32> = 32'h00000004;
  const var6([5].i): bit<32> = 32'h00000005;
  const var7([6].i): bit<32> = 32'h00000006;
  const var8([7].i): bit<32> = 32'h00000007;

  comb {
    var0[1'h0][32'h00000000+:32'sh00000002] = '0;
    var0[1'h0][32'h00000002+:32'sh00000002] = '0;
    var0[1'h0][32'h00000004+:32'sh00000002] = '0;
    var0[1'h0][32'h00000006+:32'sh00000002] = '0;
    var0[1'h1][32'h00000000+:32'sh00000002] = '0;
    var0[1'h1][32'h00000002+:32'sh00000002] = '0;
    var0[1'h1][32'h00000004+:32'sh00000002] = '0;
    var0[1'h1][32'h00000006+:32'sh00000002] = '0;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn operator_precedence() {
    let code = r#"
    module ModuleA (
        a: input  logic<32>,
        b: input  logic<32>,
        c: input  logic<32>,
        d: output logic<32>,
        e: output logic<32>,
        f: output logic<32>,
        g: output logic<32>,
        h: output logic,
        i: output logic,
        j: output logic<32>,
        k: output logic<32>,
    ) {
        // mul binds tighter than add: a + b * c => a + (b * c)
        always_comb { d = a + b * c; }
        // add binds tighter than shift: a << b + c => a << (b + c)
        always_comb { e = a << b + c; }
        // add binds tighter than compare: a + b <: c => (a + b) <: c
        always_comb { f = a + b <: c; }
        // power binds tightest: a * b ** c => a * (b ** c)
        always_comb { g = a * b ** c; }
        // logical: a || b && c => a || (b && c)
        always_comb { h = a || b && c; }
        // bitwise: a | b & c => a | (b & c)
        always_comb { i = a | b & c; }
        // left-associativity: a - b - c => (a - b) - c
        always_comb { j = a - b - c; }
        // mixed compare and equality: a + b == c => (a + b) == c
        always_comb { k = a + b == c; }
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(a): logic<32> = 32'hxxxxxxxx;
  input var1(b): logic<32> = 32'hxxxxxxxx;
  input var2(c): logic<32> = 32'hxxxxxxxx;
  output var3(d): logic<32> = 32'hxxxxxxxx;
  output var4(e): logic<32> = 32'hxxxxxxxx;
  output var5(f): logic<32> = 32'hxxxxxxxx;
  output var6(g): logic<32> = 32'hxxxxxxxx;
  output var7(h): logic = 1'hx;
  output var8(i): logic = 1'hx;
  output var9(j): logic<32> = 32'hxxxxxxxx;
  output var10(k): logic<32> = 32'hxxxxxxxx;

  comb {
    var3 = (var0 + (var1 * var2));
  }
  comb {
    var4 = (var0 << (var1 + var2));
  }
  comb {
    var5 = ((var0 + var1) <: var2);
  }
  comb {
    var6 = (var0 * (var1 ** var2));
  }
  comb {
    var7 = (var0 || (var1 && var2));
  }
  comb {
    var8 = (var0 | (var1 & var2));
  }
  comb {
    var9 = ((var0 - var1) - var2);
  }
  comb {
    var10 = ((var0 + var1) == var2);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn cast_operation() {
    let code = r#"
    module ModuleA {
        const W: u32  = 32;
        const T: type = bit<W>;

        const A: u32 = 33'h1_FFFF_FFFF as 32;
        const B: u32 = 33'h1_FFFF_FFFF as u32;
        const C: u32 = 33'h1_FFFF_FFFF as W;
        const D: u32 = 33'h1_FFFF_FFFF as T;
    }
    "#;

    let exp = r#"module ModuleA {
  const var0(W): bit<32> = 32'sh00000020;
  const var2(A): bit<32> = 32'hffffffff;
  const var3(B): bit<32> = 32'hffffffff;
  const var4(C): bit<32> = 32'hffffffff;
  const var5(D): bit<32> = 32'hffffffff;

}
"#;

    check_ir(code, exp);

    let code = r#"
    module ModuleA {
        const W: u32  = 8;
        const T: type = bit<W>;

        const A: u32 = 33'h1_FFFF_FFFF as 8;
        const B: u32 = 33'h1_FFFF_FFFF as u8;
        const C: u32 = 33'h1_FFFF_FFFF as W;
        const D: u32 = 33'h1_FFFF_FFFF as T;
    }
    "#;

    let exp = r#"module ModuleA {
  const var0(W): bit<32> = 32'sh00000008;
  const var2(A): bit<32> = 8'hff;
  const var3(B): bit<32> = 8'hff;
  const var4(C): bit<32> = 8'hff;
  const var5(D): bit<32> = 8'hff;

}
"#;

    check_ir(code, exp);

    let code = r#"
    module ModuleA {
        const W: u32  = 64;
        const T: type = bit<W>;

        const A: u64 = 32'hFFFF_FFFF as 64;
        const B: u64 = 32'hFFFF_FFFF as u64;
        const C: u64 = 32'hFFFF_FFFF as W;
        const D: u64 = 32'hFFFF_FFFF as T;
    }
    "#;

    let exp = r#"module ModuleA {
  const var0(W): bit<32> = 32'sh00000040;
  const var2(A): bit<64> = 64'h00000000ffffffff;
  const var3(B): bit<64> = 64'h00000000ffffffff;
  const var4(C): bit<64> = 64'h00000000ffffffff;
  const var5(D): bit<64> = 64'h00000000ffffffff;

}
"#;

    check_ir(code, exp);

    let code = r#"
    module top (
        i_x: input logic<32>,
    ) {
        enum Enum: logic<8> {
            A = 8'b0,
        }
        let _: logic<32> = (Enum::A as 32) + i_x;
    }
    "#;

    let exp = r#"module top {
  input var0(i_x): logic<32> = 32'hxxxxxxxx;
  let var1(_): logic<32> = 32'hxxxxxxxx;

  comb {
    var1 = ((8'h00 as 32'sh00000020) + var0);
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn array_literal_default_only() {
    // Regression: '{default: expr} must produce exactly target_len elements,
    // not target_len + 1 (the old code computed remaining as target_len - (x.len()-1)
    // which was off-by-one when x.len() == 1).
    let code = r#"
    module ModuleA {
        let a: logic[4] = '{default: 0};
    }
    "#;

    let exp = r#"module ModuleA {
  let var0[0](a): logic = 1'hx;
  let var0[1](a): logic = 1'hx;
  let var0[2](a): logic = 1'hx;
  let var0[3](a): logic = 1'hx;

  comb {
    var0[32'h00000000] = 32'sh00000000;
    var0[32'h00000001] = 32'sh00000000;
    var0[32'h00000002] = 32'sh00000000;
    var0[32'h00000003] = 32'sh00000000;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn array_literal_default_with_explicit() {
    // When mixing explicit elements with default, the explicit count must be subtracted.
    let code = r#"
    module ModuleA {
        let a: logic[4] = '{42, default: 0};
    }
    "#;

    let exp = r#"module ModuleA {
  let var0[0](a): logic = 1'hx;
  let var0[1](a): logic = 1'hx;
  let var0[2](a): logic = 1'hx;
  let var0[3](a): logic = 1'hx;

  comb {
    var0[32'h00000000] = 32'sh0000002a;
    var0[32'h00000001] = 32'sh00000000;
    var0[32'h00000002] = 32'sh00000000;
    var0[32'h00000003] = 32'sh00000000;
  }
}
"#;

    check_ir(code, exp);
}

#[test]
fn generic_function_inference() {
    let code = r#"
    module ModuleA {
        function FuncId::<T: u32> (
            x: input logic<T>,
        ) -> logic<T> {
            return x;
        }

        function FuncWide::<T: u32> (
            x: input logic<T>,
        ) -> logic<T + 1> {
            return {1'b0, x};
        }

        let _a: logic<8>  = 0;
        let _b: logic<16> = 0;

        let _r1: logic<8>  = FuncId(_a);
        let _r2: logic<16> = FuncId(_b);

        let _rw: logic<9> = FuncWide(_a);
    }
    "#;

    let exp = r#"module ModuleA {
  let var4(_a): logic<8> = 8'hxx;
  let var5(_b): logic<16> = 16'hxxxx;
  let var6(_r1): logic<8> = 8'hxx;
  var var8(FuncId::<8>.return): logic<8> = 8'hxx;
  input var9(FuncId::<8>.x): logic<8> = 8'hxx;
  let var10(_r2): logic<16> = 16'hxxxx;
  var var12(FuncId::<16>.return): logic<16> = 16'hxxxx;
  input var13(FuncId::<16>.x): logic<16> = 16'hxxxx;
  let var14(_rw): logic<9> = 9'hxxx;
  var var16(FuncWide::<8>.return): logic<9> = 9'h0xx;
  input var17(FuncWide::<8>.x): logic<8> = 8'hxx;
  func var7(FuncId::<8>) -> var8 {
    var8 = var9;
  }
  func var11(FuncId::<16>) -> var12 {
    var12 = var13;
  }
  func var15(FuncWide::<8>) -> var16 {
    var16 = {1'h0, var17};
  }

  comb {
    var4 = 32'sh00000000;
  }
  comb {
    var5 = 32'sh00000000;
  }
  comb {
    var6 = var7(x: var4);
  }
  comb {
    var10 = var11(x: var5);
  }
  comb {
    var14 = var15(x: var4);
  }
}
"#;

    check_ir(code, exp);
}
