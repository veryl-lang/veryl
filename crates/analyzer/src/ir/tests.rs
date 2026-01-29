use crate::conv::Context;
use crate::ir::Ir;
use crate::{Analyzer, AnalyzerError, attribute_table, symbol_table};
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

    let errors: Vec<_> = errors
        .into_iter()
        .filter(|x| matches!(x, AnalyzerError::UnsupportedByIr { .. }))
        .collect();
    dbg!(&errors);

    let ir = ir.to_string();
    let diff = TextDiff::from_lines(ir.as_str(), exp);
    for change in diff.iter_all_changes() {
        if matches!(change.tag(), ChangeTag::Insert | ChangeTag::Delete) {
            let text = &format!("{}{}", change.tag().to_string(), change);
            dbg!(text);
        }
    }

    assert!(ir.as_str() == exp);
    assert!(errors.is_empty());
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
  input var0(clk): clock = 'hx;
  input var1(rst): reset = 'hx;
  output var2(a): logic = 'hx;
  output var3(b): logic<32> = 'hxxxxxxxx;
  let var4(c): logic = 'hx;
  var var5(d): logic<32> = 'hxxxxxxxx;
  var var6(e): logic = 'hx;
  var var7(f): logic = 'hx;

  comb {
    var4 = var2;
  }
  ff (var0, var1) {
    if_reset {
      var2 = 00000000;
      var3 = 00000000;
    } else {
      var2 = (~ var2);
      var3 = (var3 + 00000001);
    }
  }
  comb {
    var5 = (var3 * 00000003);
    if var7 {
      var6 = 00000000;
    } else {
      var6 = 00000001;
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
  input var0(clk): clock = 'hx;
  input var1(rst): reset = 'hx;
  output var2(a): logic<32> = 'hxxxxxxxx;
  output var3(b): logic<32> = 'hxxxxxxxx;
  input var4(c): logic<32> = 'hxxxxxxxx;
  output var5(d): logic<32> = 'hxxxxxxxx;
  input var6(e): logic<32> = 'hxxxxxxxx;
  output var7(f): logic<32> = 'hxxxxxxxx;
  var var8(g): logic = 'hx;
  var var9(h): logic = 'hx;
  var var10(i): logic = 'hx;

  ff (var0, var1) {
    if_reset {
      var2 = 00000000;
    } else {
      if var8 {
        var2 = 00000001;
      } else {
        if var9 {
          var2 = 00000002;
        } else {
          var2 = 00000003;
        }
      }
    }
  }
  comb {
    if var8 {
      var3 = 00000000;
    } else {
      if var9 {
        var3 = 00000001;
      } else {
        if var10 {
          var3 = 00000002;
        } else {
          var3 = 00000003;
        }
      }
    }
    if (var4 ==? 00000000) {
      var5 = 00000000;
    } else {
      if (var4 ==? 00000001) {
        var5 = 00000001;
      } else {
        if (var4 ==? 00000002) {
          var5 = 00000002;
        } else {
          if (var4 ==? 00000003) {
            var5 = 00000003;
          } else {
            var5 = 00000004;
          }
        }
      }
    }
    if (var6 == 00000000) {
      var7 = 00000000;
    } else {
      if (var6 >= 00000001) {
        var7 = 00000001;
      } else {
        if (var6 >: 00000002) {
          var7 = 00000002;
        } else {
          if (var6 <= 00000003) {
            var7 = 00000003;
          } else {
            var7 = 00000004;
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
  param var0(N): bit<32> = 'h00000001;
  let var1(g.b): logic = 'hx;

  comb {
    var1 = 00000002;
  }
}
module ModuleB {
  param var0(N): bit<32> = 'h00000000;
  let var1(g.a): logic = 'hx;

  comb {
    var1 = 00000001;
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
  param var0(N): bit<32> = 'h00000004;
  const var1(g[0].i): bit<32> = 'h00000000;
  let var2(g[0].a): logic = 'hx;
  const var3(g[1].i): bit<32> = 'h00000001;
  let var4(g[1].a): logic = 'hx;
  const var5(g[2].i): bit<32> = 'h00000002;
  let var6(g[2].a): logic = 'hx;
  const var7(g[3].i): bit<32> = 'h00000003;
  let var8(g[3].a): logic = 'hx;

  comb {
    var2 = 00000000;
  }
  comb {
    var4 = 00000001;
  }
  comb {
    var6 = 00000002;
  }
  comb {
    var8 = 00000003;
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
  input var0(i_clk): clock = 'hx;
  input var1(i_rst): reset = 'hx;
  input var2(i_dat): logic = 'hx;
  output var3(o_dat): logic = 'hx;

}
module ModuleB {
  input var0(i_clk): clock = 'hx;
  input var1(i_rst): reset = 'hx;
  input var2(i_dat): logic = 'hx;
  output var3(o_dat): logic = 'hx;

  inst u (
    var0 <- var0;
    var1 <- var1;
    var2 <- var2;
    var3 -> var3;
  ) {
    module ModuleA {
      input var0(i_clk): clock = 'hx;
      input var1(i_rst): reset = 'hx;
      input var2(i_dat): logic = 'hx;
      output var3(o_dat): logic = 'hx;

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
  let var0(a0): logic = 'hx;
  let var1(a1): logic = 'hx;
  let var2(a2): logic = 'hx;
  let var3(a3): logic = 'hx;
  let var4(a4): logic = 'hx;
  let var5(a5): logic = 'hx;
  const var6(b0): bit<32> = 'h00000000;
  const var7(b1): bit<32> = 'h00000000;
  const var8(b2): bit<32> = 'h00000001;
  const var9(b3): bit<32> = 'h00000002;
  const var10(b4): bit<32> = 'h00000002;
  const var11(b5): bit<32> = 'h00000003;

  comb {
    var0 = 00000000;
  }
  comb {
    var1 = 00000000;
  }
  comb {
    var2 = 00000001;
  }
  comb {
    var3 = 00000002;
  }
  comb {
    var4 = 00000002;
  }
  comb {
    var5 = 00000003;
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
  var var0(a): logic<4> = 'hx;
  const var1([0].i): bit<32> = 'h00000000;
  const var2([1].i): bit<32> = 'h00000001;
  const var3([2].i): bit<32> = 'h00000002;
  const var4([3].i): bit<32> = 'h00000003;

  comb {
    var0[00000000] = 00000001;
    var0[00000001] = 00000002;
    var0[00000002] = 00000003;
    var0[00000003] = 00000004;
  }
}
"#;

    check_ir(code, exp);
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
  var var0(a): logic<4> = 'hx;
  let var1(b): logic = 'hx;
  let var2(c): logic = 'hx;

  comb {
    var1 = var0[00000003];
  }
  comb {
    var2 = var0[00000000];
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
  input var0(x): struct {x: logic<1>, y: logic<1>, z: struct {x: logic<1>, y: logic<1>}} = 'h5;
  var var1(a): struct {x: logic<1>, y: logic<1>, z: struct {x: logic<1>, y: logic<1>}} = 'hx;
  let var2(b): struct {x: logic<1>, y: logic<1>, z: struct {x: logic<1>, y: logic<1>}} = 'hx;

  comb {
    var2 = 00000001;
  }
  comb {
    var1[00000003] = 00000001;
  }
  comb {
    var1[00000002] = 00000001;
  }
  comb {
    var1[00000001:00000000] = 00000001;
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
  var var0(u0.x): logic = 'hx;
  var var1(u0.y): logic = 'hx;
  var var3(u1.x): logic = 'hx;
  var var4(u1.y): logic = 'hx;
  var var6(a): logic = 'hx;
  var var7(b): logic = 'hx;
  var var9(u0.FuncB.return): logic = 'hx;
  var var11(u1.FuncB.return): logic = 'hx;
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
  var var0[0](a): logic<2> = 'hx;
  var var0[1](a): logic<2> = 'hx;
  var var0[2](a): logic<2> = 'hx;
  var var1[0](b): struct {x: logic<1>, y: logic<1>}<2> = 'hx;
  var var1[1](b): struct {x: logic<1>, y: logic<1>}<2> = 'hx;
  var var1[2](b): struct {x: logic<1>, y: logic<1>}<2> = 'hx;
  var var2(c): logic = 'hx;
  var var3(d): logic = 'hx;

  comb {
    var2 = var0[00000002][00000001];
    var3 = var1[00000002][00000003];
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
  var var1(FuncA.return): logic = 'hx;
  input var2(FuncA.a): logic = 'hx;
  input var3(FuncA.b): logic = 'hx;
  var var4(FuncA.c): logic = 'hx;
  input var6(FuncB.a): logic = 'hx;
  output var7(FuncB.b): logic = 'hx;
  var var9(FuncC.return): logic<2> = 'hx;
  input var10(FuncC.a): struct {x: logic<1>, y: logic<1>} = 'hx;
  var var11(a): logic = 'hx;
  var var12(b): logic = 'hx;
  var var13(c): logic = 'hx;
  var var14(d): logic = 'hx;
  var var15(e): logic = 'hx;
  var var16(f): struct {x: logic<1>, y: logic<1>} = 'hx;
  var var17(g): struct {x: logic<1>, y: logic<1>} = 'hx;
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
  let var0[0](a): logic = 'hx;
  let var0[1](a): logic = 'hx;
  var var1[0](b): logic = 'hx;
  var var1[1](b): logic = 'hx;
  var var1[2](b): logic = 'hx;
  var var1[3](b): logic = 'hx;
  var var1[4](b): logic = 'hx;
  var var1[5](b): logic = 'hx;
  var var2(c): logic<2, 3, 4> = 'hxxxxxx;
  const var3[0](X): bit<32> = 'h0000000a;
  const var3[1](X): bit<32> = 'h0000000b;
  const var3[2](X): bit<32> = 'h0000000c;
  const var4(Y): bit<3, 4> = 'habc;

  comb {
    var0[00000000] = 00000000;
    var0[00000001] = 00000001;
  }
  comb {
    var1[00000000][00000000] = 00000001;
    var1[00000000][00000001] = 00000002;
    var1[00000000][00000002] = 00000003;
    var1[00000001][00000000] = 00000004;
    var1[00000001][00000001] = 00000005;
    var1[00000001][00000002] = 00000006;
    var2[00000000][00000000] = 00000001;
    var2[00000000][00000001] = 00000002;
    var2[00000000][00000002] = 00000003;
    var2[00000001] = 0000000a;
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
  var var0(u0.x): logic = 'hx;
  var var1(u0.y): struct {z: logic<1>} = 'hx;
  var var5(u1.x): logic = 'hx;
  var var6(u1.y): struct {z: logic<1>} = 'hx;
  var var10(u2.x): logic = 'hx;
  var var11(u2.y): struct {z: logic<1>} = 'hx;
  var var15(u3.x): logic = 'hx;
  var var16(u3.y): struct {z: logic<1>} = 'hx;

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
  var var0(a): logic = 'hx;

  comb {
    var0 = (var0 + 00000000);
    var0 = (var0 + (- 00000000));
    var0 = (var0 * 00000000);
    var0 = (var0 / 00000000);
    var0 = (var0 % 00000000);
    var0 = (var0 & 00000000);
    var0 = (var0 | 00000000);
    var0 = (var0 ^ 00000000);
    var0 = (var0 << 00000000);
    var0 = (var0 >> 00000000);
    var0 = (var0 <<< 00000000);
    var0 = (var0 >>> 00000000);
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
          var var0(a): logic = 'hx;

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
  var var0(a): logic = 'hx;

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
  var var3(u.a): logic = 'hx;
  var var6(a): logic = 'hx;
  var var7(b): logic = 'hx;
  var var8(c): logic = 'hx;
  var var9(d): logic = 'hx;
  var var11(u.FuncA.return): logic = 'hx;
  output var12(u.FuncA.x): logic = 'hx;
  var var14(if_a.FuncA.return): logic = 'hx;
  output var15(if_a.FuncA.x): logic = 'hx;
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
  let var0(a): logic = 'hx;
  let var1(b): logic = 'hx;
  var var2(c): logic = 'hx;
  var var4(PackageA.FuncA.return): logic = 'hx;
  input var5(PackageA.FuncA.a): logic = 'hx;
  input var6(PackageA.FuncA.b): logic = 'hx;
  func var3(PackageA.FuncA) -> var4 {
    var4 = (var5 & var6);
  }

  comb {
    var0 = 00000001;
  }
  comb {
    var1 = 00000001;
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
  input var0[0](if_a.a): logic = 'hx;
  input var0[1](if_a.a): logic = 'hx;
  output var1[0](if_a.b): logic = 'hx;
  output var1[1](if_a.b): logic = 'hx;
  var var4[0](u.a): logic = 'hx;
  var var4[1](u.a): logic = 'hx;
  var var5[0](u.b): logic = 'hx;
  var var5[1](u.b): logic = 'hx;
  var var8(a): logic<4> = 'hx;
  var var10[0](u.FuncA.return): logic = 'hx;
  var var10[1](u.FuncA.return): logic = 'hx;
  var var12[0](if_a.FuncA.return): logic = 'hx;
  var var12[1](if_a.FuncA.return): logic = 'hx;
  func var9[0](u.FuncA) -> var10 {
    var10[00000000] = var4[00000000];
  }
  func var9[1](u.FuncA) -> var10 {
    var10[00000001] = var4[00000001];
  }
  func var11[0](if_a.FuncA) -> var12 {
    var12[00000000] = var0[00000000];
  }
  func var11[1](if_a.FuncA) -> var12 {
    var12[00000001] = var0[00000001];
  }

  comb {
    var4[00000000] = var0[00000000];
    var4[00000001] = var0[00000001];
    var1[00000000] = var5[00000000];
    var1[00000001] = var5[00000001];
    var8[00000000] = var9[0]();
    var8[00000001] = var9[1]();
    var8[00000002] = var11[0]();
    var8[00000003] = var11[1]();
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
  var var0(a): enum {logic<2>} = 'hx;
  var var1(b): enum {logic<2>} = 'hx;
  var var2(c): enum {logic<3>} = 'hx;
  var var3(d): enum {logic<3>} = 'hx;

  comb {
    var0 = 0;
    var1 = 1;
    var2 = 00000007;
    var3 = 00000002;
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
  let var2(a): logic<10> = 'hxxx;
  var var3(b): logic<10> = 'hxxx;
  var var5(FuncA.return): logic<10> = 'hxxx;
  input var6(FuncA.x): logic<10> = 'hxxx;
  func var4(FuncA) -> var5 {
    var5 = (var6 + 00000001);
  }

  comb {
    var2 = 00000001;
  }
  comb {
    var3 = var4(x: var2);
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
  var var1(FuncA.return): logic = 'hx;
  input var2(FuncA.x.a): logic = 'hx;
  output var3(FuncA.x.b): struct {x: logic<1>, y: logic<1>} = 'h0;
  var var6(u.a): logic = 'hx;
  var var7(u.b): struct {x: logic<1>, y: logic<1>} = 'hx;
  var var10(a): logic = 'hx;
  func var0(FuncA) -> var1 {
    var3 = 00000000;
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
  var var0(a): struct {v: struct {x: logic<32>, y: logic<32>}, w: struct {x: logic<32>, y: logic<32>}} = 'hxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx;
  var var1(b): struct {v: struct {x: logic<32>, y: logic<32>}, w: struct {x: logic<32>, y: logic<32>}} = 'hxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx;

  comb {
    var0 = 00000001000000020000000300000004;
    var1 = 00000001ffffffff0000000000000000;
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
  output var1(o): logic = 'hx;

  comb {
    var1 = '0;
  }
}
module ModuleB {
  var var0[0](a): struct {x: logic<1>, y: logic<1>} = 'hx;
  var var0[1](a): struct {x: logic<1>, y: logic<1>} = 'hx;
  var var1(c): struct {x: logic<1>, y: logic<1>} = 'hx;
  var var2(d): struct {x: logic<1>, y: logic<1>}<2> = 'hx;

  comb {
    var1 = 00000000;
  }
  comb {
    var0[00000001] = var1;
  }
  comb {
    var2[00000003:00000002] = var2[00000001:00000000];
  }
  inst u0 (
    var1 -> var0[00000000];
  ) {
    module ModuleA {
      output var1(o): struct {x: logic<1>, y: logic<1>} = 'hx;

      comb {
        var1 = '0;
      }
    }
  }
  inst u1 (
    var1 -> var2[00000001:00000000];
  ) {
    module ModuleA {
      output var1(o): struct {x: logic<1>, y: logic<1>} = 'hx;

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
  const var0(S): string = 'h0000010b;
  const var1(T): string = 'h0000010e;
  var var2(g1._a): logic = 'hx;
  var var3(g2._d): logic = 'hx;
  var var4(g3._f): logic = 'hx;
  var var5(g4._g): logic = 'hx;
  var var6(g5._i): logic = 'hx;

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
  var var0(a): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>} = 'hxx;
  var var1(b): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>}<2> = 'hxxxx;
  var var2[0](c): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>}<2> = 'hxxxx;
  var var2[1](c): union {x: struct {v: logic<1>, w: logic<3>}<2>, y: struct {s: logic<6>, t: logic<2>}, z: logic<2, 4>}<2> = 'hxxxx;

  comb {
    var0[00000002:00000000] = var0[00000002:00000000];
    var0[00000003] = var0[00000003];
    var0[00000006:00000004] = var0[00000006:00000004];
    var0[00000007] = var0[00000007];
    var0[00000001:00000000] = var0[00000001:00000000];
    var0[00000007:00000002] = var0[00000007:00000002];
    var0[00000007:00000000] = var0[00000007:00000000];
    var0[00000000] = var0[00000000];
    var0[00000003] = var0[00000003];
    var0[00000004] = var0[00000004];
    var0[00000007] = var0[00000007];
    var0[00000000] = var0[00000000];
    var0[00000002] = var0[00000002];
    var0[00000003:00000000] = var0[00000003:00000000];
    var0[00000000] = var0[00000000];
    var0[00000001:00000000] = var0[00000001:00000000];
    var0[00000003] = var0[00000003];
    var0[00000005:00000004] = var0[00000005:00000004];
    var0[00000007] = var0[00000007];
    var0[00000001:00000000] = var0[00000001:00000000];
    var0[00000003:00000002] = var0[00000003:00000002];
    var0[00000007:00000000] = var0[00000007:00000000];
    var0[00000001:00000000] = var0[00000001:00000000];
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
  input var0(i_clk): clock = 'hx;
  input var1(i_rst): reset = 'hx;
  var var2(a): logic = 'hx;


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
      input var0(i_clk): clock = 'hx;
      input var1(i_rst): reset = 'hx;
      var var2(a): logic = 'hx;
      var var3(b): struct {a: logic<1>, b: logic<32>} = 'hxxxxxxxxx;

      ff (var0, var1) {
        if_reset {
          var3 = '0;
        } else {
          var3[00000020] = var2[00000000];
          var3[0000001f:00000000] = 00000000;
        }
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
  let var0(a): unknown = 'hx;
  let var1(_b): logic = 'hx;

  comb {
    var0 = 00000001;
  }
  comb {
    var1 = var0;
  }
}
module ModuleB {

  inst u0 (
  ) {
    module ModuleA {
      let var0(a): logic = 'hx;
      let var1(_b): logic = 'hx;

      comb {
        var0 = 00000001;
      }
      comb {
        var1 = var0;
      }
    }
  }
  inst u1 (
  ) {
    module ModuleA {
      let var0(a): bit = 'hx;
      let var1(_b): logic = 'hx;

      comb {
        var0 = 00000001;
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
