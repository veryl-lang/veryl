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
fn ir_branch() {
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
            if 1 {
              var5 = 00000004;
            }
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
            if 1 {
              var7 = 00000004;
            }
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
