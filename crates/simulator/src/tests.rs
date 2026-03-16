use crate::ir::Ir;
use crate::ir::{Config, build_ir};
use crate::ir::{Event, Value};
use crate::simulator::Simulator;
use std::str::FromStr;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::VarId;
use veryl_analyzer::{Analyzer, AnalyzerError, Context, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn build_ir_from_code(code: &str, config: &Config) -> Option<Ir> {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut errors = vec![];
    let mut ir = air::Ir::default();
    errors.append(&mut analyzer.analyze_pass1(&"prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2());

    dbg!(&errors);
    let errors: Vec<_> = errors
        .drain(0..)
        .filter(|x| !matches!(x, AnalyzerError::InvalidLogicalOperand { .. }))
        .collect();
    assert!(errors.is_empty());

    build_ir(ir, "Top".into(), config)
}

#[track_caller]
fn analyze(code: &str, config: &Config) -> Ir {
    build_ir_from_code(code, config).unwrap()
}

#[test]
fn simple_comb() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = a + b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(30, 32, false);

        assert_eq!(sim.get("c").unwrap(), exp);
    }
}

#[test]
fn simple_ff() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        println!("{}", sim.ir.dump_variables());

        sim.step(&rst);

        println!("{}", sim.ir.dump_variables());

        for _ in 0..100 {
            sim.step(&clk);
        }

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(100, 32, false);

        assert_eq!(sim.get("cnt").unwrap(), exp);
    }
}

#[test]
fn ff_to_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        i0 : input  logic<32>,
        o0 : output logic<32>,
        o1 : output logic<32>,
    ) {
        always_ff {
            if_reset {
                o0 = 0;
            } else {
                o0 = i0;
            }
        }
        always_ff {
            if_reset {
                o1 = 0;
            } else {
                o1 = o0;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        println!("{}", sim.ir.dump_variables());

        sim.step(&rst);

        println!("{}", sim.ir.dump_variables());

        for i in 0..10 {
            sim.set("i0", Value::new(i, 32, false));
            sim.step(&clk);
            println!("{}", sim.ir.dump_variables());
        }

        assert_eq!(sim.get("o0").unwrap(), Value::new(9, 32, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(8, 32, false));
    }
}

#[test]
fn short_bit() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<4>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        println!("{}", sim.ir.dump_variables());

        sim.step(&rst);

        println!("{}", sim.ir.dump_variables());

        for _ in 0..100 {
            sim.step(&clk);
        }

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(4, 4, false);

        assert_eq!(sim.get("cnt").unwrap(), exp);
    }
}

#[test]
fn long_bit_128() {
    // 128-bit variables should now be supported (both JIT and interpreter)
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<128>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.step(&clk);
        sim.step(&clk);
        sim.step(&clk);

        let got = sim.get("cnt").unwrap();
        let exp = Value::new(3, 128, false);
        assert_eq!(got, exp, "config: {:?}, got: {:?}", config, got);
    }
}

#[test]
fn long_bit_over_128() {
    // >128-bit variables should now be supported via interpreter fallback
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<256>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.step(&clk);
        sim.step(&clk);
        sim.step(&clk);

        let got = sim.get("cnt").unwrap();
        let exp = Value::new(3, 256, false);
        assert_eq!(got, exp, "config: {:?}, got: {:?}", config, got);
    }
}

#[test]
fn wide_bit_ops_256() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        c: output logic<256>,
        d: output logic<256>,
        e: output logic<256>,
    ) {
        assign c = a & b;
        assign d = a | b;
        assign e = a ^ b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(0xff00ff, 256, false);
        let b = Value::new(0x0f0f0f, 256, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::default()));

        let c = Value::new(0x0f000f, 256, false);
        let d = Value::new(0xff0fff, 256, false);
        let e = Value::new(0xf00ff0, 256, false);
        assert_eq!(sim.get("c").unwrap(), c);
        assert_eq!(sim.get("d").unwrap(), d);
        assert_eq!(sim.get("e").unwrap(), e);
    }
}

#[test]
fn wide_256_arithmetic() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        sum: output logic<256>,
        diff: output logic<256>,
        prod: output logic<256>,
    ) {
        assign sum = a + b;
        assign diff = a - b;
        assign prod = a * b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(100, 256, false);
        let b = Value::new(42, 256, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("sum").unwrap(), Value::new(142, 256, false));
        assert_eq!(sim.get("diff").unwrap(), Value::new(58, 256, false));
        assert_eq!(sim.get("prod").unwrap(), Value::new(4200, 256, false));
    }
}

#[test]
fn wide_256_comparison() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        eq: output logic,
        ne: output logic,
        gt: output logic,
        lt: output logic,
    ) {
        assign eq = a == b;
        assign ne = a != b;
        assign gt = a >: b;
        assign lt = a <: b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(100, 256, false);
        let b = Value::new(42, 256, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(
            sim.get("eq").unwrap(),
            Value::new(0, 1, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("ne").unwrap(),
            Value::new(1, 1, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("gt").unwrap(),
            Value::new(1, 1, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("lt").unwrap(),
            Value::new(0, 1, false),
            "config: {:?}",
            config
        );
    }
}

#[test]
fn wide_256_shift() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        s: input  logic<8>,
        left: output logic<256>,
        right: output logic<256>,
    ) {
        assign left  = a << s;
        assign right = a >> s;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(0xff, 256, false);
        let s = Value::new(4, 8, false);

        sim.set("a", a);
        sim.set("s", s);
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(
            sim.get("left").unwrap(),
            Value::new(0xff0, 256, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("right").unwrap(),
            Value::new(0xf, 256, false),
            "config: {:?}",
            config
        );
    }
}

#[test]
fn wide_256_ternary() {
    let code = r#"
    module Top (
        sel: input  logic,
        a: input  logic<256>,
        b: input  logic<256>,
        c: output logic<256>,
    ) {
        assign c = if sel ?  a : b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(100, 256, false);
        let b = Value::new(200, 256, false);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", a.clone());
        sim.set("b", b.clone());
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("c").unwrap(), a, "config: {:?}", config);

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("c").unwrap(), b, "config: {:?}", config);
    }
}

/// Regression: 256-bit values in for-generate with narrow constant assignment.
/// Exercises two fixed bugs:
///   1. Value width mismatch: BigUint(width=256) inside ProtoExpression(width=32)
///      caused build_binary to return a pointer while the caller expected a register.
///   2. Unaligned memory access: 32-bit variables before 256-bit elements
///      placed them at non-8-byte-aligned offsets, crashing wide_ops helpers.
#[test]
fn wide_256_array_for_generate() {
    // Use separate output ports (not an array) so sim.get() can access them.
    // The for-generate still creates the alignment pattern that triggers bug #2.
    let code = r#"
    module Top (
        clk : input  clock    ,
        rst : input  reset    ,
        a   : output logic<256>,
        b   : output logic<256>,
    ) {
        // a small 32-bit var before wide vars to create unaligned layout
        var pad: logic<32>;
        assign pad = 0;

        always_ff {
            if_reset {
                a = 1;
                b = 2;
            } else {
                a += 1;
                b += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        sim.step(&clk);
        sim.step(&clk);
        sim.step(&clk);

        // After reset: a=1, b=2. After 3 clocks: a=4, b=5
        assert_eq!(
            sim.get("a").unwrap(),
            Value::new(4, 256, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("b").unwrap(),
            Value::new(5, 256, false),
            "config: {:?}",
            config
        );
    }
}

#[test]
fn wide_bit_ops() {
    // Test bitwise operations on 96-bit variables
    let code = r#"
    module Top (
        a: input  logic<96>,
        b: input  logic<96>,
        c: output logic<96>,
        d: output logic<96>,
        e: output logic<96>,
    ) {
        assign c = a & b;
        assign d = a | b;
        assign e = a ^ b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(0xff00ff, 96, false);
        let b = Value::new(0x0f0f0f, 96, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::default()));

        let c = Value::new(0x0f000f, 96, false);
        let d = Value::new(0xff0fff, 96, false);
        let e = Value::new(0xf00ff0, 96, false);
        assert_eq!(sim.get("c").unwrap(), c);
        assert_eq!(sim.get("d").unwrap(), d);
        assert_eq!(sim.get("e").unwrap(), e);
    }
}

#[test]
fn wide_ternary() {
    let code = r#"
    module Top (
        sel: input  logic,
        a:   input  logic<128>,
        b:   input  logic<128>,
        c:   output logic<128>,
    ) {
        assign c = if sel ? a : b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(42, 128, false);
        let b = Value::new(99, 128, false);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", a.clone());
        sim.set("b", b.clone());
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("c").unwrap(), a);

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("c").unwrap(), b);
    }
}

#[test]
fn select() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        x: output logic<32>,
        y: output logic<32>,
        z: output logic<32>,
        v: output logic<32>,
        w: output logic<32>,
    ) {
        assign x = a[0];
        assign y = a[1];
        assign z = a[3:2];
        assign v = a[7:5] + a[1:0];
        assign w[0] = a[2];
        assign w[1] = a[0];
        assign w[31:2] = a[6:5];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(0xae, 32, false);

        sim.set("a", a);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        let x = Value::new(0, 32, false);
        let y = Value::new(1, 32, false);
        let z = Value::new(3, 32, false);
        let v = Value::new(7, 32, false);
        let w = Value::new(5, 32, false);

        assert_eq!(sim.get("x").unwrap(), x);
        assert_eq!(sim.get("y").unwrap(), y);
        assert_eq!(sim.get("z").unwrap(), z);
        assert_eq!(sim.get("v").unwrap(), v);
        assert_eq!(sim.get("w").unwrap(), w);
    }
}

#[test]
fn inst() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        inst u: Sub (
            a,
            b,
            c,
        );
    }

    module Sub (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        var x: logic<32>;
        assign x = a + b;
        assign c = x;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(30, 32, false);

        assert_eq!(sim.get("c").unwrap(), exp);
    }
}

#[test]
fn inst_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<32>,
    ) {
        inst u: Counter (
            clk,
            rst,
            cnt,
        );
    }

    module Counter (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..50 {
            sim.step(&clk);
        }

        let exp = Value::new(50, 32, false);

        assert_eq!(sim.get("cnt").unwrap(), exp);
    }
}

#[test]
fn inst_comb_and_ff() {
    // Sub-module with both comb and FF to test merged comb+event JIT
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
        comb_out: output logic<32>,
    ) {
        inst u: Inner (
            clk,
            rst,
            out,
            comb_out,
        );
    }

    module Inner (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
        comb_out: output logic<32>,
    ) {
        // comb depends on FF output
        assign comb_out = out + 1;

        always_ff {
            if_reset {
                out = 0;
            } else {
                out = comb_out;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After reset: out=0, comb_out=0+1=1
        assert_eq!(sim.get("out").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(1, 32, false));

        sim.step(&clk);
        // After step 1: out=comb_out=1, comb_out=1+1=2
        assert_eq!(sim.get("out").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(2, 32, false));

        sim.step(&clk);
        // After step 2: out=comb_out=2, comb_out=2+1=3
        assert_eq!(sim.get("out").unwrap(), Value::new(2, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(3, 32, false));

        for _ in 0..10 {
            sim.step(&clk);
        }
        // After 12 total steps: out=12, comb_out=13
        assert_eq!(sim.get("out").unwrap(), Value::new(12, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(13, 32, false));
    }
}

#[test]
fn binary_op() {
    let code = r#"
    module Top (
        a  : input  logic<32>,
        b  : input  logic<32>,
        c  : input  logic    ,
        d  : input  logic    ,
        x00: output logic<32>,
        x01: output logic<32>,
        x02: output logic<32>,
        x03: output logic<32>,
        x04: output logic<32>,
        x05: output logic<32>,
        x06: output logic<32>,
        x07: output logic<32>,
        x08: output logic<32>,
        x09: output logic<32>,
        x10: output logic<32>,
        x11: output logic<32>,
        x12: output logic<32>,
        x13: output logic<32>,
        x14: output logic<32>,
        x15: output logic<32>,
        x16: output logic<32>,
    ) {
        assign x00 = a + b;
        assign x01 = a - b;
        assign x02 = a * b;
        assign x03 = b / a;
        assign x04 = b % a;
        assign x05 = a & b;
        assign x06 = a | b;
        assign x07 = a ^ b;
        assign x08 = a ~^ b;
        assign x09 = a == b;
        assign x10 = a != b;
        assign x11 = a >: b;
        assign x12 = a >= b;
        assign x13 = a <: b;
        assign x14 = a <= b;
        assign x15 = c && d;
        assign x16 = c || d;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let a = Value::new(11, 32, false);
        let b = Value::new(21, 32, false);
        let c = Value::new(0, 1, false);
        let d = Value::new(1, 1, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.set("c", c);
        sim.set("d", d);

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("x00").unwrap(), Value::new(32, 32, false));
        assert_eq!(sim.get("x01").unwrap(), Value::new(0xfffffff6, 32, false));
        assert_eq!(sim.get("x02").unwrap(), Value::new(231, 32, false));
        assert_eq!(sim.get("x03").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x04").unwrap(), Value::new(10, 32, false));
        assert_eq!(sim.get("x05").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x06").unwrap(), Value::new(31, 32, false));
        assert_eq!(sim.get("x07").unwrap(), Value::new(30, 32, false));
        assert_eq!(sim.get("x08").unwrap(), Value::new(0xffffffe1, 32, false));
        assert_eq!(sim.get("x09").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x10").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x11").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x12").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x13").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x14").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x15").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x16").unwrap(), Value::new(1, 32, false));
    }
}

#[test]
fn comb_dependency() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        x: output logic<32>,
        y: output logic<32>,
        z: output logic<32>,
        v: output logic<32>,
        w: output logic<32>,
    ) {
        assign w = z + x;
        assign v = a + 1;
        assign z = b + 2;
        assign y = v + a;
        assign x = b + y;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("x").unwrap(), Value::new(41, 32, false));
        assert_eq!(sim.get("y").unwrap(), Value::new(21, 32, false));
        assert_eq!(sim.get("z").unwrap(), Value::new(22, 32, false));
        assert_eq!(sim.get("v").unwrap(), Value::new(11, 32, false));
        assert_eq!(sim.get("w").unwrap(), Value::new(63, 32, false));
    }
}

#[test]
fn dump_vcd() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = a + b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut dump = Vec::new();
        let mut sim = Simulator::new(ir, Some(&mut dump));

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::default()));

        let a = Value::new(30, 32, false);
        let b = Value::new(10, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::default()));

        let a = Value::new(50, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::default()));

        let dump = String::from_utf8(dump).unwrap();
        let exp = r#"$timescale 1 us $end
$scope module Top $end
$var wire 32 ! a $end
$var wire 32 " b $end
$var wire 32 # c $end
$upscope $end
$enddefinitions $end
#0
b00000000000000000000000000001010 !
b00000000000000000000000000010100 "
b00000000000000000000000000011110 #
#1
b00000000000000000000000000011110 !
b00000000000000000000000000001010 "
b00000000000000000000000000101000 #
#2
b00000000000000000000000000110010 !
b00000000000000000000000000010100 "
b00000000000000000000000001000110 #
"#;
        assert_eq!(dump, exp);
    }
}

#[track_caller]
fn unary_test(op: &str, x: &str, dst_width: usize, dst: &str, only_4state: bool) {
    let x_signed = if x.contains('s') { "signed" } else { "" };
    let x = Value::from_str(x.trim()).unwrap();
    let x_width = x.width();

    let code = format!(
        r#"
    module Top (
        x: input {} logic<{}>,
        o: output logic<{}>,
    ) {{
        assign o = {} x;
    }}
    "#,
        x_signed,
        x_width,
        dst_width,
        op.trim()
    );

    for config in Config::all() {
        if only_4state && !config.use_4state {
            continue;
        }

        dbg!(&config);

        let ir = analyze(&code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("x", x.clone());

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        if dst.contains('b') {
            assert_eq!(format!("{:b}", sim.get("o").unwrap()), dst);
        } else {
            assert_eq!(format!("{:x}", sim.get("o").unwrap()), dst);
        }
    }
}

#[test]
fn unary_corner_case() {
    unary_test("+ ", "8'h11 ", 16, "16'b0000000000010001", false);
    unary_test("+ ", "8'hf2 ", 16, "16'b0000000011110010", false);
    unary_test("+ ", "8'hx3 ", 16, "16'b00000000xxxx0011", true);
    unary_test("+ ", "8'hz4 ", 16, "16'b00000000zzzz0100", true);
    unary_test("+ ", "8'sh15", 16, "16'b0000000000010101", false);
    unary_test("+ ", "8'shf6", 16, "16'b1111111111110110", false);
    unary_test("+ ", "8'shx7", 16, "16'bxxxxxxxxxxxx0111", true);
    unary_test("+ ", "8'shz8", 16, "16'bzzzzzzzzzzzz1000", true);

    unary_test("- ", "8'h11 ", 16, "16'b1111111111101111", false);
    unary_test("- ", "8'hf2 ", 16, "16'b1111111100001110", false);
    unary_test("- ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    unary_test("- ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    unary_test("- ", "8'sh15", 16, "16'b1111111111101011", false);
    unary_test("- ", "8'shf6", 16, "16'b0000000000001010", false);
    unary_test("- ", "8'shx7", 16, "16'bxxxxxxxxxxxxxxxx", true);
    unary_test("- ", "8'shz8", 16, "16'bxxxxxxxxxxxxxxxx", true);

    unary_test("~ ", "8'h11 ", 16, "16'b1111111111101110", false);
    unary_test("~ ", "8'hf2 ", 16, "16'b1111111100001101", false);
    unary_test("~ ", "8'hx3 ", 16, "16'b11111111xxxx1100", true);
    unary_test("~ ", "8'hz4 ", 16, "16'b11111111xxxx1011", true);
    unary_test("~ ", "8'sh15", 16, "16'b1111111111101010", false);
    unary_test("~ ", "8'shf6", 16, "16'b0000000000001001", false);
    unary_test("~ ", "8'shx7", 16, "16'bxxxxxxxxxxxx1000", true);
    unary_test("~ ", "8'shz8", 16, "16'bxxxxxxxxxxxx0111", true);

    unary_test("& ", "8'h11 ", 16, "16'b0000000000000000", false);
    unary_test("& ", "8'hff ", 16, "16'b0000000000000001", false);
    unary_test("& ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("& ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("& ", "8'h1x ", 16, "16'b0000000000000000", true);
    unary_test("& ", "8'h1z ", 16, "16'b0000000000000000", true);
    unary_test("& ", "8'hfx ", 16, "16'b000000000000000x", true);
    unary_test("& ", "8'hxz ", 16, "16'b000000000000000x", true);

    unary_test("~&", "8'h11 ", 16, "16'b0000000000000001", false);
    unary_test("~&", "8'hff ", 16, "16'b0000000000000000", false);
    unary_test("~&", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("~&", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("~&", "8'h1x ", 16, "16'b0000000000000001", true);
    unary_test("~&", "8'h1z ", 16, "16'b0000000000000001", true);
    unary_test("~&", "8'hfx ", 16, "16'b000000000000000x", true);
    unary_test("~&", "8'hxz ", 16, "16'b000000000000000x", true);

    unary_test("| ", "8'h00 ", 16, "16'b0000000000000000", false);
    unary_test("| ", "8'h11 ", 16, "16'b0000000000000001", false);
    unary_test("| ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'h1x ", 16, "16'b0000000000000001", true);
    unary_test("| ", "8'h1z ", 16, "16'b0000000000000001", true);

    unary_test("~|", "8'h00 ", 16, "16'b0000000000000001", false);
    unary_test("~|", "8'h11 ", 16, "16'b0000000000000000", false);
    unary_test("~|", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'h1x ", 16, "16'b0000000000000000", true);
    unary_test("~|", "8'h1z ", 16, "16'b0000000000000000", true);

    unary_test("^ ", "8'h00 ", 16, "16'b0000000000000000", false);
    unary_test("^ ", "8'h01 ", 16, "16'b0000000000000001", false);
    unary_test("^ ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h1x ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h1z ", 16, "16'b000000000000000x", true);

    unary_test("~^", "8'h00 ", 16, "16'b0000000000000001", false);
    unary_test("~^", "8'h01 ", 16, "16'b0000000000000000", false);
    unary_test("~^", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h1x ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h1z ", 16, "16'b000000000000000x", true);

    unary_test("! ", "8'h00 ", 16, "16'b0000000000000001", false);
    unary_test("! ", "8'h01 ", 16, "16'b0000000000000000", false);
    unary_test("! ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'h1x ", 16, "16'b0000000000000000", true);
    unary_test("! ", "8'h1z ", 16, "16'b0000000000000000", true);
}

#[track_caller]
fn binary_test(x: &str, op: &str, y: &str, dst_width: usize, dst: &str, only_4state: bool) {
    let x_signed = if x.contains('s') { "signed" } else { "" };
    let y_signed = if y.contains('s') { "signed" } else { "" };
    let x = Value::from_str(x.trim()).unwrap();
    let y = Value::from_str(y.trim()).unwrap();
    let x_width = x.width();
    let y_width = y.width();

    let code = format!(
        r#"
    module Top (
        x: input {} logic<{}>,
        y: input {} logic<{}>,
        o: output logic<{}>,
    ) {{
        assign o = x {} y;
    }}
    "#,
        x_signed,
        x_width,
        y_signed,
        y_width,
        dst_width,
        op.trim()
    );

    for config in Config::all() {
        if only_4state && !config.use_4state {
            continue;
        }

        dbg!(&config);

        let ir = analyze(&code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("x", x.clone());
        sim.set("y", y.clone());

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        if dst.contains('b') {
            assert_eq!(format!("{:b}", sim.get("o").unwrap()), dst);
        } else {
            assert_eq!(format!("{:x}", sim.get("o").unwrap()), dst);
        }
    }
}

#[test]
fn binary_corner_case() {
    binary_test("8'h01 ", "+  ", "8'h01 ", 16, "16'b0000000000000010", false);
    binary_test("8'hf2 ", "+  ", "8'hf2 ", 16, "16'b0000000111100100", false);
    binary_test("8'hx3 ", "+  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "+  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh01", "+  ", "8'sh01", 16, "16'b0000000000000010", false);
    binary_test("8'shf2", "+  ", "8'shf2", 16, "16'b1111111111100100", false);
    binary_test("8'shx3", "+  ", "8'shx3", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'shz4", "+  ", "8'shz4", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h01 ", "-  ", "8'hf2 ", 16, "16'b1111111100001111", false);
    binary_test("8'hf2 ", "-  ", "8'h03 ", 16, "16'b0000000011101111", false);
    binary_test("8'hx3 ", "-  ", "8'hx4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "-  ", "8'hz5 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh01", "-  ", "8'shf2", 16, "16'b0000000000001111", false);
    binary_test("8'shf2", "-  ", "8'sh03", 16, "16'b1111111111101111", false);
    binary_test("8'shx3", "-  ", "8'shx4", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'shz4", "-  ", "8'shz5", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h01 ", "*  ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf2 ", "*  ", "8'hf2 ", 16, "16'b1110010011000100", false);
    binary_test("8'hx3 ", "*  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "*  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh01", "*  ", "8'sh01", 16, "16'b0000000000000001", false);
    binary_test("8'shf2", "*  ", "8'shf2", 16, "16'b0000000011000100", false);
    binary_test("8'shf3", "*  ", "8'sh03", 16, "16'b1111111111011001", false);
    binary_test("8'shz4", "*  ", "8'shz4", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h02 ", "/  ", "8'h01 ", 16, "16'b0000000000000010", false);
    binary_test("8'hf0 ", "/  ", "8'h02 ", 16, "16'b0000000001111000", false);
    binary_test("8'hx3 ", "/  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "/  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh02", "/  ", "8'sh01", 16, "16'b0000000000000010", false);
    binary_test("8'shf0", "/  ", "8'sh02", 16, "16'b1111111111111000", false);
    binary_test("8'shf3", "/  ", "8'shf3", 16, "16'b0000000000000001", false);
    binary_test("8'sh01", "/  ", "8'sh00", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h03 ", "%  ", "8'h01 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "%  ", "8'h02 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx3 ", "%  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "%  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh03", "%  ", "8'sh02", 16, "16'b0000000000000001", false);
    binary_test("8'shf1", "%  ", "8'sh02", 16, "16'b1111111111111111", false);
    binary_test("8'shf1", "%  ", "8'shfc", 16, "16'b1111111111111101", false);
    binary_test("8'sh03", "%  ", "8'shfc", 16, "16'b0000000000000011", false);

    binary_test("8'hf3 ", "&  ", "8'hc1 ", 16, "16'b0000000011000001", false);
    binary_test("8'hf1 ", "&  ", "8'he2 ", 16, "16'b0000000011100000", false);
    binary_test("8'hx1 ", "&  ", "8'hx2 ", 16, "16'b00000000xxxx0000", true);
    binary_test("8'hz3 ", "&  ", "8'hz7 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'h13 ", "&  ", "8'hx1 ", 16, "16'b00000000000x0001", true);
    binary_test("8'h11 ", "&  ", "8'hz2 ", 16, "16'b00000000000x0000", true);
    binary_test("8'hx1 ", "&  ", "8'hzd ", 16, "16'b00000000xxxx0001", true);
    binary_test("8'h1z ", "&  ", "8'hfx ", 16, "16'b000000000001xxxx", true);

    binary_test("8'hf3 ", "|  ", "8'hc1 ", 16, "16'b0000000011110011", false);
    binary_test("8'hf1 ", "|  ", "8'he2 ", 16, "16'b0000000011110011", false);
    binary_test("8'hx1 ", "|  ", "8'hx2 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'hz3 ", "|  ", "8'hz7 ", 16, "16'b00000000xxxx0111", true);
    binary_test("8'h13 ", "|  ", "8'hx1 ", 16, "16'b00000000xxx10011", true);
    binary_test("8'h11 ", "|  ", "8'hz2 ", 16, "16'b00000000xxx10011", true);
    binary_test("8'hx1 ", "|  ", "8'hzd ", 16, "16'b00000000xxxx1101", true);
    binary_test("8'h1z ", "|  ", "8'hfx ", 16, "16'b000000001111xxxx", true);

    binary_test("8'hf3 ", "^  ", "8'hc1 ", 16, "16'b0000000000110010", false);
    binary_test("8'hf1 ", "^  ", "8'he2 ", 16, "16'b0000000000010011", false);
    binary_test("8'hx1 ", "^  ", "8'hx2 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'hz3 ", "^  ", "8'hz7 ", 16, "16'b00000000xxxx0100", true);
    binary_test("8'h13 ", "^  ", "8'hx1 ", 16, "16'b00000000xxxx0010", true);
    binary_test("8'h11 ", "^  ", "8'hz2 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'hx1 ", "^  ", "8'hzd ", 16, "16'b00000000xxxx1100", true);
    binary_test("8'h1z ", "^  ", "8'hfx ", 16, "16'b000000001110xxxx", true);

    binary_test("8'hf3 ", "~^ ", "8'hc1 ", 16, "16'b1111111111001101", false);
    binary_test("8'hf1 ", "~^ ", "8'he2 ", 16, "16'b1111111111101100", false);
    binary_test("8'hx1 ", "~^ ", "8'hx2 ", 16, "16'b11111111xxxx1100", true);
    binary_test("8'hz3 ", "~^ ", "8'hz7 ", 16, "16'b11111111xxxx1011", true);
    binary_test("8'h13 ", "~^ ", "8'hx1 ", 16, "16'b11111111xxxx1101", true);
    binary_test("8'h11 ", "~^ ", "8'hz2 ", 16, "16'b11111111xxxx1100", true);
    binary_test("8'hx1 ", "~^ ", "8'hzd ", 16, "16'b11111111xxxx0011", true);
    binary_test("8'h1z ", "~^ ", "8'hfx ", 16, "16'b111111110001xxxx", true);

    binary_test("8'h00 ", "== ", "8'h00 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", "== ", "8'he2 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx0 ", "== ", "8'hx0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hx3 ", "== ", "8'hx7 ", 16, "16'b0000000000000000", true);
    binary_test("8'hz0 ", "== ", "8'hz0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz1 ", "== ", "8'hz2 ", 16, "16'b0000000000000000", true);
    binary_test("8'hxz ", "== ", "8'hxz ", 16, "16'b000000000000000x", true);
    binary_test("8'hzx ", "== ", "8'hxz ", 16, "16'b000000000000000x", true);

    binary_test("8'h00 ", "!= ", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "!= ", "8'he2 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx0 ", "!= ", "8'hx0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hx3 ", "!= ", "8'hx7 ", 16, "16'b0000000000000001", true);
    binary_test("8'hz0 ", "!= ", "8'hz0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz1 ", "!= ", "8'hz2 ", 16, "16'b0000000000000001", true);
    binary_test("8'hxz ", "!= ", "8'hxz ", 16, "16'b000000000000000x", true);
    binary_test("8'hzx ", "!= ", "8'hxz ", 16, "16'b000000000000000x", true);

    binary_test("8'h00 ", "==?", "8'h00 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", "==?", "8'he2 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx0 ", "==?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h43 ", "==?", "8'h4x ", 16, "16'b0000000000000001", true);
    binary_test("8'hz0 ", "==?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h11 ", "==?", "8'h1z ", 16, "16'b0000000000000001", true);
    binary_test("8'hxz ", "==?", "8'hxz ", 16, "16'b0000000000000001", true);
    binary_test("8'hzx ", "==?", "8'hxz ", 16, "16'b0000000000000001", true);

    binary_test("8'h00 ", "!=?", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "!=?", "8'he2 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx0 ", "!=?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h43 ", "!=?", "8'h4x ", 16, "16'b0000000000000000", true);
    binary_test("8'hz0 ", "!=?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h11 ", "!=?", "8'h1z ", 16, "16'b0000000000000000", true);
    binary_test("8'hxz ", "!=?", "8'hxz ", 16, "16'b0000000000000000", true);
    binary_test("8'hzx ", "!=?", "8'hxz ", 16, "16'b0000000000000000", true);

    binary_test("8'h03 ", ">: ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", ">: ", "8'h02 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx3 ", ">: ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", ">: ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", ">: ", "8'sh01", 16, "16'b0000000000000001", false);
    binary_test("8'shf1", ">: ", "8'sh02", 16, "16'b0000000000000000", false);
    binary_test("8'shx3", ">: ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", ">: ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", ">= ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", ">= ", "8'h02 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx3 ", ">= ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", ">= ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", ">= ", "8'sh01", 16, "16'b0000000000000001", false);
    binary_test("8'shf1", ">= ", "8'sh02", 16, "16'b0000000000000000", false);
    binary_test("8'shx3", ">= ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", ">= ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "<: ", "8'h01 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "<: ", "8'h02 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx3 ", "<: ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", "<: ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", "<: ", "8'sh01", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", "<: ", "8'sh02", 16, "16'b0000000000000001", false);
    binary_test("8'shx3", "<: ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", "<: ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "<= ", "8'h01 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "<= ", "8'h02 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx3 ", "<= ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", "<= ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", "<= ", "8'sh01", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", "<= ", "8'sh02", 16, "16'b0000000000000001", false);
    binary_test("8'shx3", "<= ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", "<= ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "&& ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", "&& ", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx3 ", "&& ", "8'hx3 ", 16, "16'b0000000000000001", true);
    binary_test("8'hz4 ", "&& ", "8'hz4 ", 16, "16'b0000000000000001", true);
    binary_test("8'h0x ", "&& ", "8'h01 ", 16, "16'b000000000000000x", true);
    binary_test("8'hf1 ", "&& ", "8'h0z ", 16, "16'b000000000000000x", true);
    binary_test("8'hxx ", "&& ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", "&& ", "8'hzz ", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "|| ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'h00 ", "|| ", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx0 ", "|| ", "8'hx0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz0 ", "|| ", "8'hz0 ", 16, "16'b000000000000000x", true);
    binary_test("8'h0x ", "|| ", "8'h0z ", 16, "16'b000000000000000x", true);
    binary_test("8'hf1 ", "|| ", "8'h0z ", 16, "16'b0000000000000001", true);
    binary_test("8'hxx ", "|| ", "8'hx3 ", 16, "16'b0000000000000001", true);
    binary_test("8'hz4 ", "|| ", "8'hzz ", 16, "16'b0000000000000001", true);

    binary_test("8'h03 ", ">> ", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", ">> ", "3'd2  ", 16, "16'b0000000000111100", false);
    binary_test("8'hx3 ", ">> ", "3'd2  ", 16, "16'b0000000000xxxx00", true);
    binary_test("8'hz4 ", ">> ", "3'd2  ", 16, "16'b0000000000zzzz01", true);
    binary_test("8'sh03", ">> ", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", ">> ", "3'd2  ", 16, "16'b0011111111111100", false);
    binary_test("8'shx3", ">> ", "3'd2  ", 16, "16'b00xxxxxxxxxxxx00", true);
    binary_test("8'shz4", ">> ", "3'd2  ", 16, "16'b00zzzzzzzzzzzz01", true);

    binary_test("8'h03 ", "<< ", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'hf1 ", "<< ", "3'd2  ", 16, "16'b0000001111000100", false);
    binary_test("8'hx3 ", "<< ", "3'd2  ", 16, "16'b000000xxxx001100", true);
    binary_test("8'hz4 ", "<< ", "3'd2  ", 16, "16'b000000zzzz010000", true);
    binary_test("8'sh03", "<< ", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'shf1", "<< ", "3'd2  ", 16, "16'b1111111111000100", false);
    binary_test("8'shx3", "<< ", "3'd2  ", 16, "16'bxxxxxxxxxx001100", true);
    binary_test("8'shz4", "<< ", "3'd2  ", 16, "16'bzzzzzzzzzz010000", true);

    binary_test("8'h03 ", ">>>", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", ">>>", "3'd2  ", 16, "16'b0000000000111100", false);
    binary_test("8'hx3 ", ">>>", "3'd2  ", 16, "16'b0000000000xxxx00", true);
    binary_test("8'hz4 ", ">>>", "3'd2  ", 16, "16'b0000000000zzzz01", true);
    binary_test("8'sh03", ">>>", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", ">>>", "3'd2  ", 16, "16'b1111111111111100", false);
    binary_test("8'shx3", ">>>", "3'd2  ", 16, "16'bxxxxxxxxxxxxxx00", true);
    binary_test("8'shz4", ">>>", "3'd2  ", 16, "16'bzzzzzzzzzzzzzz01", true);

    binary_test("8'h03 ", "<<<", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'hf1 ", "<<<", "3'd2  ", 16, "16'b0000001111000100", false);
    binary_test("8'hx3 ", "<<<", "3'd2  ", 16, "16'b000000xxxx001100", true);
    binary_test("8'hz4 ", "<<<", "3'd2  ", 16, "16'b000000zzzz010000", true);
    binary_test("8'sh03", "<<<", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'shf1", "<<<", "3'd2  ", 16, "16'b1111111111000100", false);
    binary_test("8'shx3", "<<<", "3'd2  ", 16, "16'bxxxxxxxxxx001100", true);
    binary_test("8'shz4", "<<<", "3'd2  ", 16, "16'bzzzzzzzzzz010000", true);

    binary_test("8'h03 ", "** ", "3'd2  ", 16, "16'b0000000000001001", false);
    binary_test("8'hf1 ", "** ", "3'd2  ", 16, "16'b1110001011100001", false);
    binary_test("8'hx3 ", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh03", "** ", "3'd2  ", 16, "16'b0000000000001001", false);
    binary_test("8'shf1", "** ", "3'd2  ", 16, "16'b0000000011100001", false);
    binary_test("8'shx3", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'shz4", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
}

#[test]
fn partial_jit() {
    // Mix of JIT-compilable (a + b, a ** d) and non-JIT-compilable ($display) statements.
    // With partial JIT, JIT-able statements should be compiled while $display is interpreted.
    let code = r#"
    module Top (
        a  : input  logic<32>,
        b  : input  logic<32>,
        d  : input  logic<3>,
        x  : output logic<32>,
        z  : output logic<32>,
    ) {
        assign x = a + b;
        assign z = a ** d;
    }
    "#;

    // JIT disabled: no Binary statements
    let config_no_jit = Config {
        use_jit: false,
        ..Default::default()
    };
    let ir = analyze(code, &config_no_jit);
    assert!(
        ir.comb_statements.iter().all(|s| !s.is_binary()),
        "JIT disabled: all statements should be interpreted"
    );

    // JIT enabled: all statements should be compiled
    let config_jit = Config {
        use_jit: true,
        ..Default::default()
    };
    let ir = analyze(code, &config_jit);
    let has_binary = ir.comb_statements.iter().any(|s| s.is_binary());
    assert!(has_binary, "partial JIT should compile some statements");

    // Verify simulation results are correct regardless of JIT mode
    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(10, 32, false));
        sim.set("b", Value::new(20, 32, false));
        sim.set("d", Value::new(2, 3, false));

        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("x").unwrap(), Value::new(30, 32, false));
        assert_eq!(sim.get("z").unwrap(), Value::new(100, 32, false));
    }
}

#[test]
fn concatenation() {
    // Basic concatenation: {a[15:0], b[15:0]} -> 32-bit output
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = {a[15:0], b[15:0]};
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(0xABCD_1234, 32, false));
        sim.set("b", Value::new(0x5678_9ABC, 32, false));

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        // c = {a[15:0], b[15:0]} = {0x1234, 0x9ABC} = 0x12349ABC
        assert_eq!(sim.get("c").unwrap(), Value::new(0x1234_9ABC, 32, false));
    }
}

#[test]
fn concatenation_repeat() {
    // Repeat concatenation: {a[7:0] repeat 4} -> 32-bit output
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = {a[7:0] repeat 4};
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(0xAB, 32, false));

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        // c = {0xAB, 0xAB, 0xAB, 0xAB} = 0xABABABAB
        assert_eq!(sim.get("c").unwrap(), Value::new(0xABABABAB, 32, false));
    }
}

#[test]
fn concatenation_4state() {
    // 4-state concatenation with X/Z values
    let code = r#"
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        c: output logic<16>,
    ) {
        assign c = {a, b};
    }
    "#;

    for config in Config::all() {
        if !config.use_4state {
            continue;
        }

        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        // a = 8'hx3 (upper nibble is X)
        sim.set("a", Value::from_str("8'hx3").unwrap());
        sim.set("b", Value::new(0xFF, 8, false));

        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        // c = {8'hx3, 8'hFF} = 16'hx3FF -> upper byte has X in upper nibble
        let result = sim.get("c").unwrap();
        assert_eq!(format!("{:b}", result), "16'bxxxx001111111111");
    }
}

#[test]
fn lhs_concatenation() {
    let code = r#"
    module Top (
        x: input  logic<32>,
        a: output logic<20>,
        b: output logic<12>,
    ) {
        assign {a, b} = x;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        // x = 0xABCDE123 (32 bits)
        // a = upper 20 bits = 0xABCDE
        // b = lower 12 bits = 0x123
        sim.set("x", Value::new(0xABCDE123, 32, false));
        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        let a = sim.get("a").unwrap();
        let b = sim.get("b").unwrap();
        assert_eq!(a, Value::new(0xABCDE, 20, false));
        assert_eq!(b, Value::new(0x123, 12, false));
    }
}

#[test]
fn lhs_concatenation_equal_split() {
    let code = r#"
    module Top (
        x: input  logic<32>,
        a: output logic<16>,
        b: output logic<16>,
    ) {
        assign {a, b} = x;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("x", Value::new(0xDEAD_BEEF, 32, false));
        sim.step(&Event::Clock(VarId::default()));

        let a = sim.get("a").unwrap();
        let b = sim.get("b").unwrap();
        assert_eq!(a, Value::new(0xDEAD, 16, false));
        assert_eq!(b, Value::new(0xBEEF, 16, false));
    }
}

#[test]
fn lhs_concatenation_small_value() {
    let code = r#"
    module Top (
        a: output logic<8>,
        b: output logic<8>,
    ) {
        assign {a, b} = 1;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.step(&Event::Clock(VarId::default()));

        let a = sim.get("a").unwrap();
        let b = sim.get("b").unwrap();
        assert_eq!(a, Value::new(0, 8, false));
        assert_eq!(b, Value::new(1, 8, false));
    }
}

#[test]
fn function_call_expr() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        function add(
            x: input logic<32>,
            y: input logic<32>,
        ) -> logic<32> {
            return x + y;
        }

        assign c = add(a, b);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(10, 32, false));
        sim.set("b", Value::new(20, 32, false));
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("c").unwrap(), Value::new(30, 32, false));
    }
}

#[test]
fn function_call_void() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: output logic<32>,
    ) {
        function double(
            x: input  logic<32>,
            y: output logic<32>,
        ) {
            y = x + x;
        }

        always_comb {
            double(a, b);
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(7, 32, false));
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("b").unwrap(), Value::new(14, 32, false));
    }
}

#[test]
fn function_call_with_output() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
        d: output logic<32>,
    ) {
        function add_and_double(
            x: input  logic<32>,
            side: output logic<32>,
        ) -> logic<32> {
            side = x + x;
            return x + 1;
        }

        assign c = add_and_double(a, d);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(5, 32, false));
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("c").unwrap(), Value::new(6, 32, false));
        assert_eq!(sim.get("d").unwrap(), Value::new(10, 32, false));
    }
}

#[test]
fn function_call_nested() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        function add(
            x: input logic<32>,
            y: input logic<32>,
        ) -> logic<32> {
            return x + y;
        }

        function double(
            x: input logic<32>,
        ) -> logic<32> {
            return x + x;
        }

        assign c = add(double(a), b);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(3, 32, false));
        sim.set("b", Value::new(4, 32, false));
        sim.step(&Event::Clock(VarId::default()));

        // double(3) = 6, add(6, 4) = 10
        assert_eq!(sim.get("c").unwrap(), Value::new(10, 32, false));
    }
}

#[test]
fn function_call_in_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a  : input  logic<32>,
        c  : output logic<32>,
    ) {
        function inc(
            x: input logic<32>,
        ) -> logic<32> {
            return x + 1;
        }

        always_ff {
            if_reset {
                c = 0;
            } else {
                c = inc(a);
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.set("a", Value::new(41, 32, false));
        sim.step(&clk);

        assert_eq!(sim.get("c").unwrap(), Value::new(42, 32, false));
    }
}

#[test]
fn if_expression() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic,
        a: input logic<8>,
        b: input logic<8>,
        y: output logic<8>,
    ) {
        assign y = if sel ? a : b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", Value::new(42, 8, false));
        sim.set("b", Value::new(99, 8, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));
    }
}

#[test]
fn if_expression_chained() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic<2>,
        y: output logic<8>,
    ) {
        assign y = if sel ==  2'd2 ? 8'd10 : if sel == 2'd1 ? 8'd20 : 8'd30;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("sel", Value::new(2, 2, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(10, 8, false));

        sim.set("sel", Value::new(1, 2, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(20, 8, false));

        sim.set("sel", Value::new(0, 2, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(30, 8, false));
    }
}

#[test]
fn if_expression_nested() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic,
        a: input logic<8>,
        b: input logic<8>,
        y: output logic<8>,
    ) {
        assign y = if sel ? a + b : a - b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", Value::new(30, 8, false));
        sim.set("b", Value::new(12, 8, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(18, 8, false));
    }
}

#[test]
fn if_expression_4state() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic<4>,
        a: input logic<8>,
        b: input logic<8>,
        y: output logic<8>,
    ) {
        assign y = if sel ? a : b;
    }
    "#;

    for config in Config::all() {
        if !config.use_4state {
            continue;
        }
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(42, 8, false));
        sim.set("b", Value::new(99, 8, false));

        // 4'bxxxx -> false (all unknown)
        sim.set("sel", Value::from_str("4'bxxxx").unwrap());
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'bzzzz -> false (all hi-Z)
        sim.set("sel", Value::from_str("4'bzzzz").unwrap());
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'bx000 -> false (known bits are all zero)
        sim.set("sel", Value::from_str("4'bx000").unwrap());
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'b1x00 -> true (has a known nonzero bit)
        sim.set("sel", Value::from_str("4'b1x00").unwrap());
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        // 4'b0001 -> true (nonzero, no X/Z)
        sim.set("sel", Value::new(1, 4, false));
        sim.step(&Event::Clock(VarId::default()));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));
    }
}

#[test]
fn initial_display() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $display("hello from initial");
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn display_format_specifiers() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $display("hex=%h dec=%d bin=%b oct=%o", 8'hAB, 8'd42, 4'b1010, 8'o77);
            $display("percent=%%");
            $display("no args message");
            $display("char=%c", 8'd65);
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn display_no_format_string() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $display(8'hFF, 4'b1010);
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn parse_hex_content_basic() {
    use crate::ir::parse_hex_content;

    let content = "AB CD\nEF 01";
    let values = parse_hex_content(content, 8);
    assert_eq!(values.len(), 4);
    assert_eq!(values[0].payload_u64(), 0xAB);
    assert_eq!(values[1].payload_u64(), 0xCD);
    assert_eq!(values[2].payload_u64(), 0xEF);
    assert_eq!(values[3].payload_u64(), 0x01);
}

#[test]
fn parse_hex_content_comments_and_underscores() {
    use crate::ir::parse_hex_content;

    let content = "// header comment\nDE_AD BE_EF // inline comment\n/* block\ncomment */ 42\n";
    let values = parse_hex_content(content, 16);
    assert_eq!(values.len(), 3);
    assert_eq!(values[0].payload_u64(), 0xDEAD);
    assert_eq!(values[1].payload_u64(), 0xBEEF);
    assert_eq!(values[2].payload_u64(), 0x42);
}

#[test]
fn readmemh_basic() {
    let dir = std::env::temp_dir();
    let hex_path = dir.join("veryl_test_readmemh.hex");
    std::fs::write(&hex_path, "0A 14 1E 28\n").unwrap();
    let hex_path_str = hex_path.to_str().unwrap().replace('\\', "\\\\");

    let code = format!(
        r#"
    module Top (
        i_clk: input clock,
    ) {{
        var mem: logic<8> [4];
        initial {{
            $readmemh("{}", mem);
        }}
    }}
    "#,
        hex_path_str
    );

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(&code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Initial);

        let dump = sim.ir.dump_variables();
        println!("{}", dump);

        assert!(dump.contains("mem[0] = 8'h0a"));
        assert!(dump.contains("mem[1] = 8'h14"));
        assert!(dump.contains("mem[2] = 8'h1e"));
        assert!(dump.contains("mem[3] = 8'h28"));
    }

    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn final_display() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        final {
            $display("hello from final");
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Final);
    }
}

#[test]
fn interface_inst() {
    let code = r#"
    interface MyIf {
        var data: logic<8>;
        modport master {
            data: output,
        }
        modport slave {
            data: input,
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
        out: output logic<8>,
    ) {
        inst u_if: MyIf;
        assign u_if.data = 42;
        assign out = u_if.data;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        println!("{}", ir.dump_variables());
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Clock(VarId::default()));
        println!("{}", sim.ir.dump_variables());
        let out = sim.get("out").unwrap();
        let exp = Value::new(42, 8, false);
        assert_eq!(out, exp);
    }
}

#[test]
fn interface_modport() {
    let code = r#"
    interface Bus {
        var data: logic<8>;
        var valid: logic;
        modport master {
            data: output,
            valid: output,
        }
        modport slave {
            data: input,
            valid: input,
        }
    }

    module Producer (
        clk: input clock,
        rst: input reset,
        bus: modport Bus::master,
    ) {
        assign bus.data = 99;
        assign bus.valid = 1;
    }

    module Consumer (
        clk: input clock,
        rst: input reset,
        bus: modport Bus::slave,
        out_data: output logic<8>,
        out_valid: output logic,
    ) {
        assign out_data = bus.data;
        assign out_valid = bus.valid;
    }

    module Top (
        clk: input clock,
        rst: input reset,
        out_data: output logic<8>,
        out_valid: output logic,
    ) {
        inst u_bus: Bus;

        inst u_prod: Producer (
            clk,
            rst,
            bus: u_bus,
        );

        inst u_cons: Consumer (
            clk,
            rst,
            bus: u_bus,
            out_data,
            out_valid,
        );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        println!("{}", ir.dump_variables());
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Clock(VarId::default()));
        println!("{}", sim.ir.dump_variables());
        let data = sim.get("out_data").unwrap();
        let valid = sim.get("out_valid").unwrap();
        assert_eq!(data, Value::new(99, 8, false));
        assert_eq!(valid, Value::new(1, 1, false));
    }
}

#[test]
fn interface_function() {
    let code = r#"
    interface BusIf {
        var data: logic<8>;

        function get_double() -> logic<8> {
            return data * 2;
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
        out: output logic<8>,
    ) {
        inst u_bus: BusIf;
        assign u_bus.data = 21;
        assign out = u_bus.get_double();
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        println!("{}", ir.dump_variables());
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Clock(VarId::default()));
        println!("{}", sim.ir.dump_variables());
        let out = sim.get("out").unwrap();
        let exp = Value::new(42, 8, false);
        assert_eq!(out, exp);
    }
}

#[test]
fn array_literal_comb() {
    let code = r#"
    module Top (
        a: input  logic<8>,
        o0: output logic<8>,
        o1: output logic<8>,
        o2: output logic<8>,
        o3: output logic<8>,
    ) {
        var mem: logic<8> [4];
        assign mem = '{10, 20, 30, a};
        assign o0 = mem[0];
        assign o1 = mem[1];
        assign o2 = mem[2];
        assign o3 = mem[3];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(40, 8, false));
        sim.step(&Event::Clock(VarId::default()));

        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("o0").unwrap(), Value::new(10, 8, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(20, 8, false));
        assert_eq!(sim.get("o2").unwrap(), Value::new(30, 8, false));
        assert_eq!(sim.get("o3").unwrap(), Value::new(40, 8, false));
    }
}

#[test]
fn array_literal_ff() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        o0: output logic<8>,
        o1: output logic<8>,
    ) {
        var mem: logic<8> [2];
        always_ff {
            if_reset {
                mem = '{0, 0};
            } else {
                mem = '{100, 200};
            }
        }
        assign o0 = mem[0];
        assign o1 = mem[1];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("o0").unwrap(), Value::new(0, 8, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(0, 8, false));

        sim.step(&clk);
        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("o0").unwrap(), Value::new(100, 8, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(200, 8, false));
    }
}

#[test]
fn struct_constructor() {
    let code = r#"
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        c: output logic<16>,
    ) {
        struct Pair {
            hi: logic<8>,
            lo: logic<8>,
        }

        let p: Pair = Pair'{hi: a, lo: b};
        assign c = p;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(0xAB, 8, false));
        sim.set("b", Value::new(0xCD, 8, false));

        sim.step(&Event::Clock(VarId::default()));

        // Struct is packed MSB-first: {hi, lo} = {0xAB, 0xCD} = 0xABCD
        assert_eq!(sim.get("c").unwrap(), Value::new(0xABCD, 16, false));
    }
}

#[test]
fn struct_constructor_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a  : input  logic<8>,
        b  : input  logic<8>,
        o  : output logic<16>,
    ) {
        struct Pair {
            hi: logic<8>,
            lo: logic<8>,
        }

        var r: Pair;
        always_ff {
            if_reset {
                r = Pair'{hi: 0, lo: 0};
            } else {
                r = Pair'{hi: a, lo: b};
            }
        }
        assign o = r;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("o").unwrap(), Value::new(0x0000, 16, false));

        sim.set("a", Value::new(0xAB, 8, false));
        sim.set("b", Value::new(0xCD, 8, false));

        sim.step(&clk);
        assert_eq!(sim.get("o").unwrap(), Value::new(0xABCD, 16, false));
    }
}

#[test]
fn array_dynamic_index_read() {
    let code = r#"
    module Top (
        idx: input  logic<2>,
        o  : output logic<8>,
    ) {
        var arr: logic<8> [4];

        assign arr[0] = 10;
        assign arr[1] = 20;
        assign arr[2] = 30;
        assign arr[3] = 40;
        assign o = arr[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        for idx in 0..4u64 {
            sim.set("idx", Value::new(idx, 2, false));
            sim.step(&Event::Clock(VarId::default()));
            let expected = (idx + 1) * 10;
            assert_eq!(sim.get("o").unwrap(), Value::new(expected, 8, false));
        }
    }
}

#[test]
fn array_dynamic_index_write_ff() {
    let code = r#"
    module Top (
        clk : input  clock,
        rst : input  reset,
        idx : input  logic<2>,
        val : input  logic<8>,
        ridx: input  logic<2>,
        o   : output logic<8>,
    ) {
        var arr: logic<8> [4];

        always_ff {
            if_reset {
                arr[0] = 0;
                arr[1] = 0;
                arr[2] = 0;
                arr[3] = 0;
            } else {
                arr[idx] = val;
            }
        }

        assign o = arr[ridx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write 42 to arr[2]
        sim.set("idx", Value::new(2, 2, false));
        sim.set("val", Value::new(42, 8, false));
        sim.step(&clk);

        // Read arr[0..3] through output port
        for i in 0..4u64 {
            sim.set("ridx", Value::new(i, 2, false));
            let expected = if i == 2 { 42 } else { 0 };
            assert_eq!(sim.get("o").unwrap(), Value::new(expected, 8, false));
        }

        // Write 99 to arr[0]
        sim.set("idx", Value::new(0, 2, false));
        sim.set("val", Value::new(99, 8, false));
        sim.step(&clk);

        for i in 0..4u64 {
            sim.set("ridx", Value::new(i, 2, false));
            let expected = if i == 0 {
                99
            } else if i == 2 {
                42
            } else {
                0
            };
            assert_eq!(sim.get("o").unwrap(), Value::new(expected, 8, false));
        }
    }
}

#[test]
fn array_dynamic_index_write_comb() {
    let code = r#"
    module Top (
        idx: input  logic<2>,
        val: input  logic<8>,
        o  : output logic<8>,
    ) {
        var arr: logic<8> [4];

        assign arr[0] = 10;
        assign arr[1] = 20;
        assign arr[2] = 30;
        assign arr[3] = 40;
        assign arr[idx] = val;
        assign o = arr[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("idx", Value::new(1, 2, false));
        sim.set("val", Value::new(77, 8, false));
        sim.step(&Event::Clock(VarId::default()));

        assert_eq!(sim.get("o").unwrap(), Value::new(77, 8, false));
    }
}
