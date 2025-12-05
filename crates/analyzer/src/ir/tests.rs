use crate::ir::Ir;
use crate::{Analyzer, attribute_table, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn create_ir(code: &str) -> Ir {
    symbol_table::clear();
    attribute_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);

    let mut ir = Ir::default();

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1(&"prj", &"", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &"", &parser.veryl, Some(&mut ir)));
    let info = Analyzer::analyze_post_pass2();
    errors.append(&mut analyzer.analyze_pass3(&"prj", &"", &parser.veryl, &info));
    //dbg!(&errors);

    ir
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
            if true {
                e = 0;
            } else {
                e = 1;
            }
        }
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(clk): logic = 'hx;
  input var1(rst): logic = 'hx;
  output var2(a): logic = 'hx;
  output var3(b): logic<32> = 'hxxxxxxxx;
  var var4(c): logic = 'hx;
  var var5(d): logic<32> = 'hxxxxxxxx;
  var var6(e): logic = 'hx;

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
    if 1 {
      var6 = 00000000;
    } else {
      var6 = 00000001;
    }
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
        always_ff {
            if_reset {
                a = 0;
            } else if true {
                a = 1;
            } else if false {
                a = 2;
            } else {
                a = 3;
            }
        }
        always_comb {
            if true {
                b = 0;
            } else if true {
                b = 1;
            } else if false {
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
  input var0(clk): logic = 'hx;
  input var1(rst): logic = 'hx;
  output var2(a): logic<32> = 'hxxxxxxxx;
  output var3(b): logic<32> = 'hxxxxxxxx;
  input var4(c): logic<32> = 'hxxxxxxxx;
  output var5(d): logic<32> = 'hxxxxxxxx;
  input var6(e): logic<32> = 'hxxxxxxxx;
  output var7(f): logic<32> = 'hxxxxxxxx;

  ff (var0, var1) {
    if_reset {
      var2 = 00000000;
    } else {
      if 1 {
        var2 = 00000001;
      } else {
        if 0 {
          var2 = 00000002;
        } else {
          var2 = 00000003;
        }
      }
    }
  }
  comb {
    if 1 {
      var3 = 00000000;
    } else {
      if 1 {
        var3 = 00000001;
      } else {
        if 0 {
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

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
  param var0(N): signed logic<32> = 'h00000001;
  var var1(g.b): logic = 'hx;

  comb {
    var1 = 00000002;
  }
}
module ModuleB {
  param var0(N): signed logic<32> = 'h00000000;
  var var1(g.a): logic = 'hx;

  comb {
    var1 = 00000001;
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
  param var0(N): signed logic<32> = 'h00000004;
  const var1(g[0].i): logic<32> = 'h00000000;
  var var2(g[0].a): logic = 'hx;
  const var3(g[1].i): logic<32> = 'h00000001;
  var var4(g[1].a): logic = 'hx;
  const var5(g[2].i): logic<32> = 'h00000002;
  var var6(g[2].a): logic = 'hx;
  const var7(g[3].i): logic<32> = 'h00000003;
  var var8(g[3].a): logic = 'hx;

  comb {
    var2 = var1;
  }
  comb {
    var4 = var3;
  }
  comb {
    var6 = var5;
  }
  comb {
    var8 = var7;
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
}

#[test]
fn inst() {
    let code = r#"
    module ModuleA (
        i_clk: input clock,
        i_rst: input reset,
    ) {
    }
    module ModuleB (
        i_clk: input clock,
        i_rst: input reset,
    ) {
        inst u: ModuleA (
            i_clk,
            i_rst,
        );
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(i_clk): logic = 'hx;
  input var1(i_rst): logic = 'hx;

}
module ModuleB {
  input var0(i_clk): logic = 'hx;
  input var1(i_rst): logic = 'hx;

  inst ModuleA {
    module ModuleA {
      input var0(i_clk): logic = 'hx;
      input var1(i_rst): logic = 'hx;

    }
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
}

#[test]
fn system_function() {
    let code = r#"
    module ModuleA {
        let a: logic = $clog2(1);
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(a): logic = 'hx;

  comb {
    var0 = $clog2(00000001);
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
  const var1([0].i): logic<32> = 'h00000000;
  const var2([1].i): logic<32> = 'h00000001;
  const var3([2].i): logic<32> = 'h00000002;
  const var4([3].i): logic<32> = 'h00000003;

  comb {
    var0[var1] = (var1 + 00000001);
    var0[var2] = (var2 + 00000001);
    var0[var3] = (var3 + 00000001);
    var0[var4] = (var4 + 00000001);
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
  var var1(b): logic = 'hx;
  var var2(c): logic = 'hx;

  comb {
    var1 = var0[00000003];
  }
  comb {
    var2 = var0[00000000];
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
    }
    "#;

    let exp = r#"module ModuleA {
  input var0(x.x): logic = 'h0;
  input var1(x.y): logic = 'h1;
  input var2(x.z.x): logic = 'h0;
  input var3(x.z.y): logic = 'h1;
  var var4(a.x): logic = 'hx;
  var var5(a.y): logic = 'hx;
  var var6(a.z.x): logic = 'hx;
  var var7(a.z.y): logic = 'hx;
  var var8(b.x): logic = 'hx;
  var var9(b.y): logic = 'hx;
  var var10(b.z.x): logic = 'hx;
  var var11(b.z.y): logic = 'hx;

  comb {
    {var8, var9, var10, var11} = 00000001;
  }
  comb {
    var4 = 00000001;
  }
  comb {
    var5 = 00000001;
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
    }
    "#;

    let exp = r#"module ModuleA {
  var var0(u0.x): logic = 'hx;
  var var1(u0.y): logic = 'hx;
  var var2(u0.FuncB.return): logic = 'hx;
  var var5(u1.x): logic = 'hx;
  var var6(u1.y): logic = 'hx;
  var var7(u1.FuncB.return): logic = 'hx;
  func var3(u0.FuncB) {
    var2 = var0;
  }
  func var8(u1.FuncB) {
    var7 = var5;
  }

}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
  var var1[0](b.x): logic = 'hx;
  var var1[1](b.x): logic = 'hx;
  var var1[2](b.x): logic = 'hx;
  var var1[3](b.x): logic = 'hx;
  var var1[4](b.x): logic = 'hx;
  var var1[5](b.x): logic = 'hx;
  var var2[0](b.y): logic = 'hx;
  var var2[1](b.y): logic = 'hx;
  var var2[2](b.y): logic = 'hx;
  var var2[3](b.y): logic = 'hx;
  var var2[4](b.y): logic = 'hx;
  var var2[5](b.y): logic = 'hx;
  var var3(c): logic = 'hx;
  var var4(d): logic = 'hx;

  comb {
    var3 = var0[00000002][00000001];
    var4 = var1[00000002][00000001][00000000];
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
}

#[test]
fn function() {
    let code = r#"
    module ModuleA {
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

        var a: logic;
        var b: logic;
        var c: logic;
        var d: logic;
        var e: logic;

        always_comb {
            c = FuncA(a, b);
            FuncB(d, e);
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
  var var8(a): logic = 'hx;
  var var9(b): logic = 'hx;
  var var10(c): logic = 'hx;
  var var11(d): logic = 'hx;
  var var12(e): logic = 'hx;
  func var0(FuncA) {
    var4 = (var2 | var3);
    var1 = (var2 & var4);
  }
  func var5(FuncB) {
    var7 = var6;
  }

  comb {
    var10 = var0(a: var8, b: var9);
    var5(a: var11, b: var12);
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
}

#[test]
fn array_literal() {
    let code = r#"
    module ModuleA {
        let a: logic[2] = '{0, 1};
        var b: logic[2, 3];

        always_comb {
            b = '{'{1, 2, 3}, '{4, 5, 6}};
        }
    }
    "#;

    let exp = r#"module ModuleA {
  var var0[0](a): logic = 'hx;
  var var0[1](a): logic = 'hx;
  var var1[0](b): logic = 'hx;
  var var1[1](b): logic = 'hx;
  var var1[2](b): logic = 'hx;
  var var1[3](b): logic = 'hx;
  var var1[4](b): logic = 'hx;
  var var1[5](b): logic = 'hx;

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
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
}

#[test]
fn connect() {
    let code = r#"
    interface InterfaceA {
        var x: logic;
        modport master {
            x: output,
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
  var var4(u1.x): logic = 'hx;
  var var8(u2.x): logic = 'hx;
  var var12(u3.x): logic = 'hx;

  comb {
    var0 = var4;
  }
  comb {
    var8 = var12;
  }
}
"#;

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
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
    var0 = (var0 - 00000000);
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

    let ir = create_ir(code);
    assert_eq!(&ir.to_string(), exp);
}
