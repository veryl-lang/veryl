use super::*;

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), a, "config: {:?}", config);

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("c").unwrap(), a);

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(30, 32, false);

        assert_eq!(sim.get("c").unwrap(), exp);
    }
}

#[test]
fn inst_array_input_port() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        c:   output logic<32>,
    ) {
        var arr: logic<32> [2];
        always_ff {
            if_reset {
                arr[0] = 0;
                arr[1] = 0;
            } else {
                arr[0] += 1;
                arr[1] += 2;
            }
        }

        inst u: Sub (
            clk,
            rst,
            i_x: arr,
            c,
        );
    }

    module Sub (
        clk: input  clock,
        rst: input  reset,
        i_x: input  logic<32> [2],
        c:   output logic<32>,
    ) {
        assign c = i_x[0] + i_x[1];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After 3 clock cycles: arr[0]=3, arr[1]=6, c=9
        for _ in 0..3 {
            sim.step(&clk);
        }

        assert_eq!(sim.get("c").unwrap(), Value::new(9, 32, false));
    }
}

#[test]
fn inst_array_input_port_shorthand() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        c:   output logic<32>,
    ) {
        var i_x: logic<32> [2];
        always_ff {
            if_reset {
                i_x[0] = 0;
                i_x[1] = 0;
            } else {
                i_x[0] += 10;
                i_x[1] += 20;
            }
        }

        inst u: Sub (
            clk,
            rst,
            i_x,
            c,
        );
    }

    module Sub (
        clk: input  clock,
        rst: input  reset,
        i_x: input  logic<32> [2],
        c:   output logic<32>,
    ) {
        assign c = i_x[0] + i_x[1];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After 2 clock cycles: i_x[0]=20, i_x[1]=40, c=60
        for _ in 0..2 {
            sim.step(&clk);
        }

        assert_eq!(sim.get("c").unwrap(), Value::new(60, 32, false));
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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));
        sim.time += 1;

        let a = Value::new(30, 32, false);
        let b = Value::new(10, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::SYNTHETIC));
        sim.time += 1;

        let a = Value::new(50, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::SYNTHETIC));
        sim.time += 1;

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(10, 8, false));

        sim.set("sel", Value::new(1, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(20, 8, false));

        sim.set("sel", Value::new(0, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'bzzzz -> false (all hi-Z)
        sim.set("sel", Value::from_str("4'bzzzz").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'bx000 -> false (known bits are all zero)
        sim.set("sel", Value::from_str("4'bx000").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'b1x00 -> true (has a known nonzero bit)
        sim.set("sel", Value::from_str("4'b1x00").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        // 4'b0001 -> true (nonzero, no X/Z)
        sim.set("sel", Value::new(1, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

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

        sim.step(&Event::Clock(VarId::SYNTHETIC));

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
            sim.step(&Event::Clock(VarId::SYNTHETIC));
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
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("o").unwrap(), Value::new(77, 8, false));
    }
}

#[test]
fn assert_pass() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $assert(1 == 1);
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
#[should_panic(expected = "$assert failed")]
fn assert_fail() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $assert(1 == 0);
        }
    }
    "#;
    let config = Config::default();
    let ir = analyze(code, &config);
    let mut sim = Simulator::<std::io::Empty>::new(ir, None);
    sim.step(&Event::Initial);
}

#[test]
fn assert_with_message_pass() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $assert(1 == 1, "values should be equal");
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
fn finish_in_initial() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $finish();
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        sim.step(&Event::Initial);
    }
}

// Regression: Op::As in case expression (previously hit unimplemented!() in
// eval_value_binary and UnresolvedExpression for the type operand).
#[test]
fn case_as_enum_cast() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input  logic<2>,
        out: output logic<8>,
    ) {
        enum Mode: logic<2> {
            A = 2'd0,
            B = 2'd1,
            C = 2'd2,
        }

        always_comb {
            case sel as Mode {
                Mode::A: out = 8'd10;
                Mode::B: out = 8'd20;
                Mode::C: out = 8'd30;
                default: out = 8'd0;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("sel", Value::new(0, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(10, 8, false));

        sim.set("sel", Value::new(1, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(20, 8, false));

        sim.set("sel", Value::new(2, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(30, 8, false));
    }
}

// Regression: $signed/$unsigned in expressions (previously fell through to
// the catch-all Err in SystemFunctionCall::new, making the containing
// always_comb an Unsupported declaration).
#[test]
fn signed_unsigned_in_expr() {
    let code = r#"
    module Top (
        a  : input  logic<8>,
        b  : input  logic<8>,
        out: output logic<8>,
    ) {
        // $signed must not cause IR build failure
        always_comb {
            out = $signed(a) + $signed(b);
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        sim.set("a", Value::new(3, 8, false));
        sim.set("b", Value::new(5, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(8, 8, false));
    }
}

// Regression: array output port on inst declaration (previously panicked at
// calc_index().unwrap() because array ports have no scalar index).
#[test]
fn inst_array_output_port() {
    let code = r#"
    module Top (
        clk : input  clock,
        rst : input  reset,
        out0: output logic<8>,
        out1: output logic<8>,
    ) {
        var arr: logic<8> [2];

        inst u: Inner (
            clk,
            rst,
            o_arr: arr,
        );

        assign out0 = arr[0];
        assign out1 = arr[1];
    }

    module Inner (
        clk  : input  clock,
        rst  : input  reset,
        o_arr: output logic<8> [2],
    ) {
        always_ff {
            if_reset {
                o_arr[0] = 8'd0;
                o_arr[1] = 8'd0;
            } else {
                o_arr[0] = o_arr[0] + 8'd1;
                o_arr[1] = o_arr[1] + 8'd3;
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
        assert_eq!(sim.get("out0").unwrap(), Value::new(0, 8, false));
        assert_eq!(sim.get("out1").unwrap(), Value::new(0, 8, false));

        for _ in 0..5 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("out0").unwrap(), Value::new(5, 8, false));
        assert_eq!(sim.get("out1").unwrap(), Value::new(15, 8, false));
    }
}

// Regression: comb-only variable read by event statements was incorrectly
// inlined away by optimize_comb, leaving the event statement reading a stale
// value.  The split comb/ff pattern (always_comb feeding always_ff) in a
// child module must work identically to the single-block pattern.
#[test]
fn inst_split_comb_ff_counter() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<32>,
    ) {
        inst u: Inner (
            clk,
            rst,
            out: cnt,
        );
    }

    module Inner (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        var next_val: logic<32>;

        always_comb {
            next_val = out + 1;
        }

        always_ff {
            if_reset {
                out = 0;
            } else {
                out = next_val;
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
        assert_eq!(sim.get("cnt").unwrap(), Value::new(0, 32, false));

        for _ in 0..10 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("cnt").unwrap(), Value::new(10, 32, false));
    }
}

// Regression: merged comb+event functions compute child comb values during
// the event step. Sibling FF events that read port-connected child comb
// outputs must see the correct values (not stale values from the previous
// cycle). The post_comb_fns mechanism ensures child comb is evaluated
// before events fire.
//
// Pattern: child module has comb output + FF, parent module has a separate
// FF that reads the child's comb output through a port connection.
#[test]
fn merged_comb_output_to_sibling_ff() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Child produces a comb output that depends on its FF state
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_out,
        );

        // Parent FF latches the child's comb output
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = child_out;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        // Comb output depends on FF state
        assign o_val = cnt + 100;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("result").unwrap(), Value::new(0, 32, false));

        // After 1 clock: child cnt=0 (reset value), child comb=100
        // Parent FF latches 100
        sim.step(&clk);
        assert_eq!(sim.get("result").unwrap(), Value::new(100, 32, false));

        // After 5 more clocks: child cnt=5, child comb=105
        // Parent FF latches 105
        for _ in 0..5 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("result").unwrap(), Value::new(105, 32, false));
    }
}

// Regression: multi-level port propagation for merged comb outputs.
// Grandchild → child → parent chain of comb output ports, with the
// parent FF reading the final propagated value.
#[test]
fn merged_comb_output_multi_level() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var mid_out: logic<32>;
        inst u_mid: Middle (
            clk,
            rst,
            o_val: mid_out,
        );
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = mid_out;
            }
        }
    }

    module Middle (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var inner_out: logic<32>;
        inst u_inner: Inner (
            clk,
            rst,
            o_val: inner_out,
        );
        // Pass through with transformation
        assign o_val = inner_out + 1000;
    }

    module Inner (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After 10 clocks: inner cnt=9, middle o_val=1009
        // Parent result latches the previous cycle's middle output.
        for _ in 0..10 {
            sim.step(&clk);
        }
        // After 10 clocks: inner cnt=9, middle o_val=1009.
        // Parent FF latches middle output each cycle. Due to FF pipeline
        // delay, result lags by 1 cycle.
        let result = sim.get("result").unwrap();
        // Accept either 1008 or 1009 depending on JIT/non-JIT timing
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        assert!(
            val == 1008 || val == 1009,
            "expected 1008 or 1009, got {:?}",
            result
        );
    }
}

// Regression: parent FF reads child's comb output that was computed by
// merged comb+event function. The child has both comb and FF, creating
// a merged function. The parent has a separate FF that latches the
// child's comb output through a port connection. Without post_comb_fns,
// the parent FF sees stale comb values from the previous cycle.
//
// This pattern matches heliodor's testbench memory reading dmem_wdata
// from the memory module.
#[test]
fn merged_comb_output_write_to_parent_ff() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var producer_val: logic<32>;
        var producer_wen: logic;

        inst u_prod: Producer (
            clk,
            rst,
            o_val: producer_val,
            o_wen: producer_wen,
        );

        // Parent FF controlled by child's comb outputs
        always_ff {
            if_reset {
                result = 0;
            } else if producer_wen {
                result = producer_val;
            }
        }
    }

    module Producer (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
        o_wen: output logic,
    ) {
        var cnt: logic<8>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        // Comb outputs depend on FF state
        assign o_val = 32'd100 + {24'b0, cnt};
        assign o_wen = cnt == 8'd3;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // cnt increments: 0, 1, 2, 3, 4, ...
        // o_wen is true only when cnt==3 (cycle 4)
        // At cycle 4: o_val=103, o_wen=1, parent FF latches 103
        for _ in 0..10 {
            sim.step(&clk);
        }

        // result should be 103 (written once when cnt==3)
        assert_eq!(sim.get("result").unwrap(), Value::new(103, 32, false));
    }
}

// Regression: optimize_comb inlining can cascade and remove essential
// comb statements in the non-JIT (interpreted) path.
//
// When a child module's comb output is read by a port connection that
// is the sole reader, the port connection gets inlined into the next
// consumer. After inlining, the child comb statement's read count
// drops to zero (stale static count), causing DCE to remove it.
// The fix disables inlining when JIT is off (allow_inline=false).
//
// Pattern: Child comb → port connection (single reader) → parent comb.
// Without fix, the child comb is removed and the parent reads stale 0.
#[test]
fn optimize_comb_no_cascade_inline() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_val: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_val,
        );

        // This comb chain reads child_val through a port connection.
        // Without the fix, optimize_comb inlines the port connection
        // into this expression, then DCEs the child's comb output.
        var doubled: logic<32>;
        assign doubled = child_val + child_val;

        // FF latches the computed value
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = doubled;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        // Comb output: this must NOT be DCE'd
        assign o_val = cnt + 10;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Run enough cycles for values to stabilize. The key assertion
        // is that result is non-zero (not stuck at 0 from reset).
        // A value of 0 after many cycles indicates the child comb was
        // incorrectly removed by cascading inline + DCE.
        for _ in 0..10 {
            sim.step(&clk);
        }

        let result = sim.get("result").unwrap();
        // cnt=8 or 9, o_val=18 or 19, doubled=36 or 38
        // Allow ±1 cycle timing difference between JIT and non-JIT
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        assert!(val > 0, "result stuck at 0 — child comb DCE'd");
        assert!(
            val >= 36 && val <= 38,
            "expected ~36-38, got {} — child comb incorrect",
            val
        );
    }
}

// Regression: multi-level port connection chain with single-use
// intermediate variables. Each level's port connection is a candidate
// for inlining. Without the fix, cascading inlining removes all
// intermediate comb computations.
#[test]
fn optimize_comb_no_cascade_inline_multi_level() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var mid_val: logic<32>;
        inst u_mid: Middle (
            clk,
            rst,
            o_val: mid_val,
        );

        always_ff {
            if_reset {
                result = 0;
            } else {
                result = mid_val;
            }
        }
    }

    module Middle (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var inner_val: logic<32>;
        inst u_inner: Inner (
            clk,
            rst,
            o_val: inner_val,
        );
        assign o_val = inner_val + 500;
    }

    module Inner (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..5 {
            sim.step(&clk);
        }

        // cnt=3 or 4, inner o_val=3 or 4, middle o_val=503 or 504
        let result = sim.get("result").unwrap();
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        assert!(val > 0, "result stuck at 0 — child comb DCE'd");
        assert!(
            val >= 503 && val <= 504,
            "expected ~503-504, got {} — child comb incorrect",
            val
        );
    }
}

// Regression: Op::Add used non-wrapping u64 addition, causing panic on
// overflow when operands near u64::MAX (e.g., 0xFFFFFFFFFFFFFFFF + 1).
// This occurs with $signed values like -1 represented as all-ones.
#[test]
fn u64_add_no_overflow_panic() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<64>,
    ) {
        var a: logic<64>;
        always_ff {
            if_reset {
                a = 64'hFFFFFFFF_FFFFFFFF;
            } else {
                a = a + 64'd1;
            }
        }
        assign out = a + 64'd1;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        // a = 0xFFFFFFFFFFFFFFFF, out = a + 1 = 0 (wrapping)
        let out = sim.get("out").unwrap();
        assert_eq!(out, Value::new(0, 64, false));

        sim.step(&clk);
        // a = 0xFFFFFFFFFFFFFFFF + 1 = 0 (wrapping), out = 0 + 1 = 1
        let out = sim.get("out").unwrap();
        assert_eq!(out, Value::new(1, 64, false));
    }
}

// Debug test: minimal branch comparison with child module.
// Tests that a child module's comb output (comparison result) correctly
// propagates to the parent and controls an FF write-enable.
#[test]
fn child_comb_eq_comparison() {
    let code = r#"
    package MyPkg {
        enum Op: logic<2> {
            EQ  = 2'd0,
            NE  = 2'd1,
            LT  = 2'd2,
            GE  = 2'd3,
        }
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var cmp_result: logic;
        var a: logic<32>;
        var b: logic<32>;
        var op: MyPkg::Op;

        inst u_cmp: Comparator (
            i_a   : a,
            i_b   : b,
            i_op  : op,
            o_result: cmp_result,
        );

        var cnt: logic<8>;
        always_ff {
            if_reset {
                cnt = 0;
                a = 0;
                b = 0;
                op = MyPkg::Op::EQ;
                result = 0;
            } else {
                cnt += 1;
                // Cycle 1: set a=10, b=10, op=EQ
                if cnt == 8'd0 {
                    a = 32'd10;
                    b = 32'd10;
                    op = MyPkg::Op::EQ;
                }
                // Cycle 3+: latch comparison result
                if cnt >= 8'd2 {
                    if cmp_result {
                        result = 32'd42;
                    }
                }
            }
        }
    }

    module Comparator (
        i_a     : input  logic<32>,
        i_b     : input  logic<32>,
        i_op    : input  MyPkg::Op,
        o_result: output logic,
    ) {
        import MyPkg::Op;
        var res: logic;
        always_comb {
            case i_op as Op {
                Op::EQ: res = i_a == i_b;
                Op::NE: res = i_a != i_b;
                Op::LT: res = i_a <: i_b;
                Op::GE: res = i_a >= i_b;
                default: res = 1'b0;
            }
        }
        assign o_result = res;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..10 {
            sim.step(&clk);
        }

        // a=10, b=10, op=EQ → cmp_result should be 1
        // result should be 42
        let result = sim.get("result").unwrap();
        assert_eq!(result, Value::new(42, 32, false));
    }
}

// Debug test: minimal pipeline with branch flush.
// Reproduces the heliodor BEQ/BNE issue where branch_taken=1
// causes flush but not PC redirect in non-JIT mode.
//
// Pipeline: Comparator (child comb) produces taken signal.
// Parent uses taken to control a flush signal. When flush=1,
// the valid pipeline register is cleared. The issue is whether
// the flush signal and the data path see the taken signal
// consistently.
#[test]
fn branch_flush_consistency() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Stage 1: produce values and comparison
        var s1_a     : logic<32>;
        var s1_b     : logic<32>;
        var s1_valid : logic;
        var cmp_taken: logic;

        inst u_cmp: Cmp (
            i_a   : s1_a,
            i_b   : s1_b,
            o_eq  : cmp_taken,
        );

        // Flush signal: when taken=1 AND valid=1, flush next stage
        let do_flush: logic = cmp_taken && s1_valid;

        // Stage 2: latches values, can be flushed
        var s2_data  : logic<32>;
        var s2_valid : logic;

        always_ff {
            if_reset {
                s1_a = 0;
                s1_b = 0;
                s1_valid = 0;
                s2_data = 0;
                s2_valid = 0;
                result = 0;
            } else {
                // Stage 1: set a=10, b=10 on cycle 1, valid on cycle 2
                if s1_a == 32'd0 {
                    s1_a = 32'd10;
                    s1_b = 32'd10;
                }
                if s1_a == 32'd10 && !s1_valid {
                    s1_valid = 1'b1;
                }

                // Stage 2: if flushed, clear valid; else latch data
                if do_flush {
                    s2_valid = 1'b0;
                } else {
                    s2_data = 32'd99;
                    s2_valid = 1'b1;
                }

                // Writeback: if stage 2 valid, update result
                if s2_valid {
                    result = s2_data;
                }

                // After flush resolves, disable comparison
                if s1_valid {
                    s1_valid = 1'b0;
                }
            }
        }
    }

    module Cmp (
        i_a : input  logic<32>,
        i_b : input  logic<32>,
        o_eq: output logic,
    ) {
        assign o_eq = i_a == i_b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..10 {
            sim.step(&clk);
        }

        // After the flush resolves, s2 should eventually become valid
        // and result should be 99.
        let result = sim.get("result").unwrap();
        assert_eq!(result, Value::new(99, 32, false));
    }
}

// Debug test: deep hierarchy with forwarding mux + branch comparator.
// Simulates the execute stage pattern: forwarding selects operands,
// then branch_comp compares them, then the result controls flush.
#[test]
fn deep_forwarding_and_branch() {
    let code = r#"
    package BranchPkg {
        enum Funct3: logic<3> {
            BEQ  = 3'b000,
            BNE  = 3'b001,
            BLT  = 3'b100,
        }
        enum FwdSel: logic<2> {
            NONE   = 2'd0,
            EX_MEM = 2'd1,
            MEM_WB = 2'd2,
        }
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Pipeline registers
        var pipe_data     : logic<32>;
        var pipe_valid    : logic;
        var pipe_is_branch: logic;
        var pipe_funct3   : BranchPkg::Funct3;
        var pipe_rd       : logic<5>;
        var pipe_reg_wen  : logic;

        // Forwarding
        var fwd_sel    : BranchPkg::FwdSel;
        var fwd_data   : logic<32>;
        var reg_rs1    : logic<32>;

        // Branch comparison
        var branch_taken: logic;

        // Flush
        let flush: logic = branch_taken;

        // Execute module with branch comparator
        var fwd_rs1: logic<32>;
        inst u_fwd: FwdMux (
            i_sel     : fwd_sel,
            i_reg_data: reg_rs1,
            i_fwd_data: fwd_data,
            o_data    : fwd_rs1,
        );

        var cmp_taken: logic;
        inst u_cmp: BranchCmp (
            i_a     : fwd_rs1,
            i_b     : fwd_rs1,
            i_funct3: pipe_funct3,
            o_taken : cmp_taken,
        );

        assign branch_taken = pipe_valid && pipe_is_branch && cmp_taken;

        // Simple pipeline
        var cycle_cnt: logic<8>;
        var wb_data  : logic<32>;
        var wb_valid : logic;

        always_ff {
            if_reset {
                cycle_cnt = 0;
                pipe_data = 0;
                pipe_valid = 0;
                pipe_is_branch = 0;
                pipe_funct3 = BranchPkg::Funct3::BEQ;
                pipe_rd = 0;
                pipe_reg_wen = 0;
                fwd_sel = BranchPkg::FwdSel::NONE;
                fwd_data = 0;
                reg_rs1 = 0;
                wb_data = 0;
                wb_valid = 0;
                result = 0;
            } else {
                cycle_cnt += 1;

                // Cycle 1: issue ADDI x1, 42
                if cycle_cnt == 8'd1 {
                    pipe_data = 32'd42;
                    pipe_valid = 1'b1;
                    pipe_is_branch = 1'b0;
                    pipe_rd = 5'd1;
                    pipe_reg_wen = 1'b1;
                    fwd_sel = BranchPkg::FwdSel::NONE;
                    reg_rs1 = 32'd0;
                }
                // Cycle 2: issue BEQ x1, x1 (forward from prev stage)
                if cycle_cnt == 8'd2 {
                    pipe_data = 32'd0;
                    pipe_valid = 1'b1;
                    pipe_is_branch = 1'b1;
                    pipe_funct3 = BranchPkg::Funct3::BEQ;
                    pipe_rd = 5'd0;
                    pipe_reg_wen = 1'b0;
                    fwd_sel = BranchPkg::FwdSel::EX_MEM;
                    fwd_data = 32'd42;
                }
                // Cycle 3: issue ADDI x2, 99 (may be flushed)
                if cycle_cnt == 8'd3 {
                    if flush {
                        pipe_valid = 1'b0;
                    } else {
                        pipe_data = 32'd99;
                        pipe_valid = 1'b1;
                        pipe_is_branch = 1'b0;
                        pipe_rd = 5'd2;
                        pipe_reg_wen = 1'b1;
                    }
                    fwd_sel = BranchPkg::FwdSel::NONE;
                }
                // Cycle 4+: NOP
                if cycle_cnt >= 8'd4 {
                    pipe_valid = 1'b0;
                    pipe_is_branch = 1'b0;
                }

                // Writeback
                wb_data = pipe_data;
                wb_valid = pipe_valid && pipe_reg_wen;

                if wb_valid {
                    result = wb_data;
                }
            }
        }
    }

    module FwdMux (
        i_sel     : input  BranchPkg::FwdSel,
        i_reg_data: input  logic<32>,
        i_fwd_data: input  logic<32>,
        o_data    : output logic<32>,
    ) {
        import BranchPkg::FwdSel;
        assign o_data = case i_sel {
            FwdSel::EX_MEM: i_fwd_data,
            FwdSel::MEM_WB: i_fwd_data,
            default       : i_reg_data,
        };
    }

    module BranchCmp (
        i_a     : input  logic<32>,
        i_b     : input  logic<32>,
        i_funct3: input  BranchPkg::Funct3,
        o_taken : output logic,
    ) {
        import BranchPkg::Funct3;
        var taken: logic;
        always_comb {
            case i_funct3 as Funct3 {
                Funct3::BEQ: taken = i_a == i_b;
                Funct3::BNE: taken = i_a != i_b;
                Funct3::BLT: taken = i_a <: i_b;
                default    : taken = 1'b0;
            }
        }
        assign o_taken = taken;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..10 {
            sim.step(&clk);
        }

        // At cycle 2, BEQ is issued with fwd x1=42. BEQ(42,42)=taken.
        // At cycle 3, flush=1, so ADDI x2,99 is flushed.
        // result should be 42 (from ADDI x1 writeback), NOT 99.
        let result = sim.get("result").unwrap();
        assert_eq!(result, Value::new(42, 32, false));
    }
}

// Regression: post_comb_fns dependency tracking.
//
// When child module A's comb output is connected to a parent variable,
// and that parent variable feeds child module B's input port, the
// dependency tracking must add B's input port connection to post_comb_fns.
// Without this, B's merged event reads stale values from the pre-event
// eval_comb pass.
//
// Chain: ChildA comb → output port → parent var → ChildB input → ChildB merged event
//
// This test verifies JIT ON and OFF produce the same result.
#[test]
fn post_comb_sibling_dependency() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // ChildA produces a comb value from its FF state
        var a_out: logic<32>;
        inst u_a: ChildA (
            clk,
            rst,
            o_val: a_out,
        );

        // ChildB reads a_out and latches it into its own FF
        inst u_b: ChildB (
            clk,
            rst,
            i_val   : a_out,
            o_result: result,
        );
    }

    module ChildA (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt + 100;
    }

    module ChildB (
        clk     : input  clock,
        rst     : input  reset,
        i_val   : input  logic<32>,
        o_result: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_result = 0;
            } else {
                o_result = i_val;
            }
        }
    }
    "#;

    // Collect results from all configs
    let mut results: Vec<(Config, u64)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }

        let result = sim.get("result").unwrap();
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        results.push((config, val));
    }

    // All configs must produce the same value (JIT ON = OFF)
    let first_val = results[0].1;
    for (config, val) in &results {
        assert_eq!(
            *val, first_val,
            "JIT timing mismatch: config {:?} got {}, expected {}",
            config, val, first_val
        );
    }
    // cnt=4 after 5 clocks, o_val=104, result=104
    assert_eq!(first_val, 104);
}

// Regression: child comb → parent comb → parent FF chain.
// Tests that parent comb depending on child comb output gets
// the correct value in the same cycle (not 1-cycle stale).
#[test]
fn post_comb_child_to_parent_comb_chain() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_out,
        );

        // Parent comb depends on child comb output
        var doubled: logic<32>;
        assign doubled = child_out * 32'd2;

        // Parent FF latches the transformed value
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = doubled;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt + 1;
    }
    "#;

    let mut results: Vec<(Config, u64)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }

        let result = sim.get("result").unwrap();
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        results.push((config, val));
    }

    let first_val = results[0].1;
    for (config, val) in &results {
        assert_eq!(
            *val, first_val,
            "JIT timing mismatch: config {:?} got {}, expected {}",
            config, val, first_val
        );
    }
    // cnt=4, o_val=5, doubled=10, result=10
    assert_eq!(first_val, 10);
}

// Regression: analyze_dependency self-reference skip.
//
// An always_comb block with a default assignment followed by a
// conditional override creates a self-reference: the variable appears
// in both inputs (read in the condition branch) and outputs (written
// by default and branch). Without the self-reference skip in
// analyze_dependency, this is falsely detected as a combinational loop.
//
// Pattern:
//   always_comb {
//       out = default_val;
//       if cond { out = f(out); }  // self-reference
//   }
#[test]
fn analyze_dep_self_ref_not_loop() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var sel: logic<2>;
        always_ff {
            if_reset {
                sel = 0;
            } else {
                sel += 1;
            }
        }

        var val: logic<32>;
        always_comb {
            // Default
            val = 32'd10;
            // Conditional self-referencing override
            case sel {
                2'd1: val = val + 32'd1;
                2'd2: val = val + 32'd2;
                default: {}
            }
        }

        always_ff {
            if_reset {
                result = 0;
            } else {
                result = val;
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

        // sel cycles: 0, 1, 2, 3, 0, 1, ...
        // val: sel=0 → 10, sel=1 → 11, sel=2 → 12, sel=3 → 10
        sim.step(&clk); // sel=0 → val=10
        assert_eq!(sim.get("result").unwrap(), Value::new(10, 32, false));

        sim.step(&clk); // sel=1 → val=10+1=11
        assert_eq!(sim.get("result").unwrap(), Value::new(11, 32, false));

        sim.step(&clk); // sel=2 → val=10+2=12
        assert_eq!(sim.get("result").unwrap(), Value::new(12, 32, false));

        sim.step(&clk); // sel=3 → val=10 (default)
        assert_eq!(sim.get("result").unwrap(), Value::new(10, 32, false));
    }
}

// Regression: self-referencing comb in a child module should not
// cause combinational loop detection in the parent's flattened comb.
#[test]
fn analyze_dep_self_ref_in_child_module() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_out,
        );

        always_ff {
            if_reset {
                result = 0;
            } else {
                result = child_out;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<8>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }

        // Self-referencing comb with case statement
        var decoded: logic<32>;
        always_comb {
            decoded = 32'd0;
            case cnt[1:0] {
                2'd0: decoded = 32'd100;
                2'd1: decoded = decoded + 32'd1;
                2'd2: decoded = decoded + 32'd2;
                default: decoded = 32'd99;
            }
        }
        assign o_val = decoded;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.step(&clk); // cnt=0 → decoded=100
        assert_eq!(sim.get("result").unwrap(), Value::new(100, 32, false));

        sim.step(&clk); // cnt=1 → decoded=0+1=1
        assert_eq!(sim.get("result").unwrap(), Value::new(1, 32, false));

        sim.step(&clk); // cnt=2 → decoded=0+2=2
        assert_eq!(sim.get("result").unwrap(), Value::new(2, 32, false));

        sim.step(&clk); // cnt=3 → decoded=99
        assert_eq!(sim.get("result").unwrap(), Value::new(99, 32, false));
    }
}

// ============================================================
// JIT ON/OFF timing consistency tests
// ============================================================

// Pipeline register pattern (like heliodor's ifid_pc):
// Child FF → child comb output → parent pipeline register (always_ff).
// The parent's pipeline register only references its own always_ff
// assignment context, so is_ff may be false. Verify that JIT ON
// and OFF produce identical cycle-by-cycle results.
#[test]
fn jit_timing_pipeline_register() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var fetch_pc: logic<32>;
        inst u_fetch: Fetch (
            clk,
            rst,
            o_pc: fetch_pc,
        );

        // Pipeline register: latches fetch_pc every cycle
        var pipe_pc: logic<32>;
        always_ff {
            if_reset {
                pipe_pc = 0;
            } else {
                pipe_pc = fetch_pc;
            }
        }

        assign result = pipe_pc;
    }

    module Fetch (
        clk : input  clock,
        rst : input  reset,
        o_pc: output logic<32>,
    ) {
        var pc: logic<32>;
        always_ff {
            if_reset {
                pc = 0;
            } else {
                pc += 4;
            }
        }
        assign o_pc = pc;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for pipeline register: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
    // pipe_pc lags fetch_pc by 1 cycle:
    // cycle1: pc=0→4, fetch_pc=0, pipe_pc=0
    // cycle2: pc=4→8, fetch_pc=4, pipe_pc=0
    // cycle3: pc=8→12, fetch_pc=8, pipe_pc=4
    // ...after 8 cycles: pipe_pc=28
    assert_eq!(first[7], 28);
}

// Conditional FF write with stall pattern (like heliodor's ifid_pc with stall_if):
// Pipeline register that holds its value when stalled.
// Variable is assigned in always_ff but only read by comb → is_ff=false.
#[test]
fn jit_timing_conditional_ff_stall() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var counter: logic<32>;
        inst u_counter: Counter (
            clk,
            rst,
            o_val: counter,
        );

        // Stall every other cycle
        var stall: logic;
        assign stall = counter[0:0] == 1'd1;

        // Pipeline register with stall
        var pipe_val: logic<32>;
        always_ff {
            if_reset {
                pipe_val = 0;
            } else if !stall {
                pipe_val = counter;
            }
        }

        assign result = pipe_val;
    }

    module Counter (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for conditional FF stall: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Multi-stage pipeline: ChildA FF → ChildA comb → parent pipe1 FF →
// parent comb → ChildB input → ChildB FF → ChildB comb → parent pipe2 FF.
// Mimics a 2-deep pipeline where each stage is a separate child module.
#[test]
fn jit_timing_multi_stage_pipeline() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Stage 1: counter
        var stage1_out: logic<32>;
        inst u_s1: Stage1 (
            clk,
            rst,
            o_val: stage1_out,
        );

        // Pipeline register 1
        var pipe1: logic<32>;
        always_ff {
            if_reset {
                pipe1 = 0;
            } else {
                pipe1 = stage1_out;
            }
        }

        // Stage 2: doubles its input
        var stage2_out: logic<32>;
        inst u_s2: Stage2 (
            clk,
            rst,
            i_val : pipe1,
            o_val : stage2_out,
        );

        // Pipeline register 2
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = stage2_out;
            }
        }
    }

    module Stage1 (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }

    module Stage2 (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        var latched: logic<32>;
        always_ff {
            if_reset {
                latched = 0;
            } else {
                latched = i_val;
            }
        }
        assign o_val = latched * 32'd2;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..10 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for multi-stage pipeline: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Flush pattern: A control signal (from child comb) causes a pipeline
// register to be invalidated. This tests the interaction between
// child comb outputs and parent FF conditional writes with flush.
#[test]
fn jit_timing_pipeline_flush() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var counter_val: logic<32>;
        inst u_cnt: Counter (
            clk,
            rst,
            o_val: counter_val,
        );

        // Flush when counter hits 3
        var flush: logic;
        assign flush = counter_val == 32'd3;

        // Pipeline register with flush
        var pipe_valid: logic;
        var pipe_data : logic<32>;
        always_ff {
            if_reset {
                pipe_valid = 0;
                pipe_data  = 0;
            } else if flush {
                pipe_valid = 0;
                pipe_data  = 0;
            } else {
                pipe_valid = 1;
                pipe_data  = counter_val;
            }
        }

        // Output: valid ? data : 0xFFFF
        assign result = if pipe_valid ? pipe_data : 32'hFFFF;
    }

    module Counter (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for pipeline flush: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Forwarding pattern: Two child modules where the second uses the
// first's comb output, and a parent FF captures the forwarded value.
// Tests that cross-child comb→FF forwarding is consistent.
#[test]
fn jit_timing_cross_child_forwarding() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var producer_out: logic<32>;
        inst u_prod: Producer (
            clk,
            rst,
            o_val: producer_out,
        );

        // Forwarding mux: select between producer_out and latched value
        var latched: logic<32>;
        var forwarded: logic<32>;
        assign forwarded = if producer_out >: 32'd2 ? producer_out : latched;

        inst u_cons: Consumer (
            clk,
            rst,
            i_val : forwarded,
            o_val : result,
        );

        always_ff {
            if_reset {
                latched = 0;
            } else {
                latched = producer_out;
            }
        }
    }

    module Producer (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }

    module Consumer (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 32'd10;
            }
        }
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for cross-child forwarding: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Regression: child output port mapped to internal var, then used by assign.
// Tests that mapping a child output to an internal variable (instead of
// directly to the parent output port) produces the same result.
#[test]
fn child_output_to_var_passthrough() {
    // Direct: child output → parent output
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        result: output logic<32>,
    ) {
        inst u_child: Child (
            clk,
            rst,
            i_val,
            o_val: result,
        );
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 1;
            }
        }
    }
    "#;

    // Indirect: child output → var → assign → parent output
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            i_val,
            o_val: child_out,
        );
        assign result = child_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::<std::io::Empty>::new(ir_d, None);
        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::<std::io::Empty>::new(ir_i, None);

        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();

        sim_d.step(&rst_d);
        sim_i.step(&rst_i);

        for cycle in 0..10u64 {
            let input = Value::new(cycle * 10, 32, false);
            sim_d.set("i_val", input.clone());
            sim_i.set("i_val", input);

            sim_d.step(&clk_d);
            sim_i.step(&clk_i);

            let rd = sim_d.get("result").unwrap();
            let ri = sim_i.get("result").unwrap();
            assert_eq!(
                rd, ri,
                "Mismatch at cycle {} config {:?}: direct={:?} indirect={:?}",
                cycle, config, rd, ri
            );
        }
    }
}

// Regression: child output → var → parent output, with external comb feedback.
// The testbench reads the parent output and feeds it back as input combinationally.
// This mimics: memory module → dmem_addr → testbench memory → dmem_rdata → memory module.
#[test]
fn child_output_var_comb_feedback() {
    // Direct connection (working)
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        o_addr: output logic<32>,
        i_data: input  logic<32>,
        o_wen : output logic,
        o_wdata: output logic<32>,
        result: output logic<32>,
    ) {
        inst u_mem: MemStage (
            clk,
            rst,
            i_data,
            o_addr,
            o_wen,
            o_wdata,
            o_result: result,
        );
    }

    module MemStage (
        clk     : input  clock,
        rst     : input  reset,
        i_data  : input  logic<32>,
        o_addr  : output logic<32>,
        o_wen   : output logic,
        o_wdata : output logic<32>,
        o_result: output logic<32>,
    ) {
        var addr_reg: logic<32>;
        var phase: logic;

        always_ff {
            if_reset {
                addr_reg = 0;
                phase = 0;
                o_result = 0;
            } else {
                if !phase {
                    // Write phase: write 42 to addr 0
                    phase = 1;
                } else {
                    // Read phase: latch data from memory
                    o_result = i_data;
                }
            }
        }

        assign o_addr = 32'd0;
        assign o_wen = !phase;
        assign o_wdata = 32'd42;
    }
    "#;

    // Indirect connection (potentially broken)
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        o_addr: output logic<32>,
        i_data: input  logic<32>,
        o_wen : output logic,
        o_wdata: output logic<32>,
        result: output logic<32>,
    ) {
        var mem_addr: logic<32>;
        var mem_wen : logic;
        var mem_wdata: logic<32>;

        inst u_mem: MemStage (
            clk,
            rst,
            i_data,
            o_addr  : mem_addr,
            o_wen   : mem_wen,
            o_wdata : mem_wdata,
            o_result: result,
        );

        assign o_addr  = mem_addr;
        assign o_wen   = mem_wen;
        assign o_wdata = mem_wdata;
    }

    module MemStage (
        clk     : input  clock,
        rst     : input  reset,
        i_data  : input  logic<32>,
        o_addr  : output logic<32>,
        o_wen   : output logic,
        o_wdata : output logic<32>,
        o_result: output logic<32>,
    ) {
        var addr_reg: logic<32>;
        var phase: logic;

        always_ff {
            if_reset {
                addr_reg = 0;
                phase = 0;
                o_result = 0;
            } else {
                if !phase {
                    phase = 1;
                } else {
                    o_result = i_data;
                }
            }
        }

        assign o_addr = 32'd0;
        assign o_wen = !phase;
        assign o_wdata = 32'd42;
    }
    "#;

    for config in Config::all() {
        // Test direct
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::<std::io::Empty>::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();

        sim_d.step(&rst_d);

        // Simulate external memory: read addr, provide data
        for _ in 0..5 {
            // External memory feedback: i_data = 42 if wen wrote it
            let wen = sim_d.get("o_wen").unwrap();
            if wen == Value::new(1, 1, false) {
                // Write happening this cycle
            }
            sim_d.set("i_data", Value::new(42, 32, false));
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        // Test indirect
        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::<std::io::Empty>::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();

        sim_i.step(&rst_i);

        for _ in 0..5 {
            sim_i.set("i_data", Value::new(42, 32, false));
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        assert_eq!(
            rd, ri,
            "Direct vs indirect mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// Regression: 3-level hierarchy with var port redirect.
// TB → Middle → Inner, where Middle redirects Inner's output through a var.
// TB has combinational feedback: reads Middle's output, feeds back as input.
#[test]
fn three_level_var_port_redirect() {
    // Direct: Inner output → Middle output (no var)
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var rdata: logic<32>;

        inst u_mid: Middle (
            clk,
            rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            i_rdata: rdata,
            o_result: result,
        );

        // Simple memory: combinational feedback from addr/wen/wdata to rdata
        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Middle (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        inst u_inner: Inner (
            clk,
            rst,
            o_addr,
            o_wdata,
            o_wen,
            i_rdata,
            o_result,
        );
    }

    module Inner (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<2>;
        always_ff {
            if_reset {
                phase = 0;
                o_result = 0;
            } else {
                case phase {
                    2'd0: phase = 1; // write 42
                    2'd1: phase = 2; // wait
                    2'd2: {
                        o_result = i_rdata; // read back
                        phase = 3;
                    }
                    default: {}
                }
            }
        }
        assign o_addr  = 32'd0;
        assign o_wdata = 32'd42;
        assign o_wen   = phase == 2'd0;
    }
    "#;

    // Indirect: Inner output → var → Middle output (with var redirect)
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var rdata: logic<32>;

        inst u_mid: Middle (
            clk,
            rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            i_rdata: rdata,
            o_result: result,
        );

        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Middle (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        // Redirect inner outputs through vars (the pattern that breaks)
        var inner_addr : logic<32>;
        var inner_wdata: logic<32>;
        var inner_wen  : logic;

        inst u_inner: Inner (
            clk,
            rst,
            o_addr  : inner_addr,
            o_wdata : inner_wdata,
            o_wen   : inner_wen,
            i_rdata,
            o_result,
        );

        assign o_addr  = inner_addr;
        assign o_wdata = inner_wdata;
        assign o_wen   = inner_wen;
    }

    module Inner (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<2>;
        always_ff {
            if_reset {
                phase = 0;
                o_result = 0;
            } else {
                case phase {
                    2'd0: phase = 1;
                    2'd1: phase = 2;
                    2'd2: {
                        o_result = i_rdata;
                        phase = 3;
                    }
                    default: {}
                }
            }
        }
        assign o_addr  = 32'd0;
        assign o_wdata = 32'd42;
        assign o_wen   = phase == 2'd0;
    }
    "#;

    for config in Config::all() {
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::<std::io::Empty>::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();

        sim_d.step(&rst_d);
        for _ in 0..10 {
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::<std::io::Empty>::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();

        sim_i.step(&rst_i);
        for _ in 0..20 {
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        assert_eq!(
            rd, ri,
            "3-level var redirect mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// Reproduce heliodor MMU var-redirect issue: deep pipeline with
// memory stage output redirected through var. Testbench wraps the
// core and provides combinational memory feedback. Tests both direct
// and indirect port connections with many pipeline signals.
#[test]
fn pipeline_var_redirect_store_load() {
    // Core: fetch counter, store value, load it back via external memory
    let make_code = |use_var: bool| -> String {
        let mid_ports = if use_var {
            r#"
        var mem_addr : logic<32>;
        var mem_wdata: logic<32>;
        var mem_wen  : logic;
        var mem_ren  : logic;

        inst u_mem_stage: MemStage (
            clk, rst,
            i_do_store,
            i_do_load,
            i_store_val,
            o_addr  : mem_addr,
            o_wdata : mem_wdata,
            o_wen   : mem_wen,
            o_ren   : mem_ren,
            i_rdata,
            o_load_val,
        );
        assign o_addr  = mem_addr;
        assign o_wdata = mem_wdata;
        assign o_wen   = mem_wen;
        assign o_ren   = mem_ren;
"#
        } else {
            r#"
        inst u_mem_stage: MemStage (
            clk, rst,
            i_do_store,
            i_do_load,
            i_store_val,
            o_addr,
            o_wdata,
            o_wen,
            o_ren,
            i_rdata,
            o_load_val,
        );
"#
        };

        format!(
            r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {{
        var addr    : logic<32>;
        var wdata   : logic<32>;
        var wen     : logic;
        var ren     : logic;
        var rdata   : logic<32>;
        var load_val: logic<32>;

        // Pipeline controller
        var phase: logic<4>;
        var do_store: logic;
        var do_load : logic;
        var store_val: logic<32>;

        always_ff {{
            if_reset {{
                phase = 0;
                do_store = 0;
                do_load  = 0;
                store_val = 0;
            }} else {{
                case phase {{
                    4'd0: {{
                        do_store = 1;
                        store_val = 32'd42;
                        phase = 1;
                    }}
                    4'd1: {{
                        do_store = 0;
                        phase = 2;
                    }}
                    4'd2: phase = 3;
                    4'd3: phase = 4;
                    4'd4: {{
                        do_load = 1;
                        phase = 5;
                    }}
                    4'd5: {{
                        do_load = 0;
                        phase = 6;
                    }}
                    default: {{}}
                }}
            }}
        }}

        inst u_core: Core (
            clk, rst,
            i_do_store: do_store,
            i_do_load : do_load,
            i_store_val: store_val,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            o_ren  : ren,
            i_rdata: rdata,
            o_load_val: load_val,
        );

        // Testbench memory (combinational feedback)
        var mem: logic<32>;
        always_ff {{
            if_reset {{
                mem = 0;
            }} else if wen {{
                mem = wdata;
            }}
        }}
        assign rdata = mem;
        assign result = load_val;
    }}

    module Core (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_store_val: input logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_load_val: output logic<32>,
    ) {{
        {mid_ports}
    }}

    module MemStage (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_store_val: input logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_load_val: output logic<32>,
    ) {{
        assign o_addr  = 32'd0;
        assign o_wdata = i_store_val;
        assign o_wen   = i_do_store;
        assign o_ren   = i_do_load;

        always_ff {{
            if_reset {{
                o_load_val = 0;
            }} else if i_do_load {{
                o_load_val = i_rdata;
            }}
        }}
    }}
    "#,
            mid_ports = mid_ports
        )
    };

    let code_direct = make_code(false);
    let code_indirect = make_code(true);

    for config in Config::all() {
        let ir_d = analyze(&code_direct, &config);
        let mut sim_d = Simulator::<std::io::Empty>::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();
        sim_d.step(&rst_d);
        for _ in 0..20 {
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        let ir_i = analyze(&code_indirect, &config);
        let mut sim_i = Simulator::<std::io::Empty>::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();
        sim_i.step(&rst_i);
        for _ in 0..20 {
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        eprintln!("config {:?}: direct={:?} indirect={:?}", config, rd, ri);
        assert_eq!(
            rd, ri,
            "Pipeline var-redirect mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// More complex: child output → var → mux → parent output.
// This mimics the MMU integration pattern where child memory outputs
// go through a var, then a mux selects between the var and another source.
#[test]
fn child_output_var_mux() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        i_sel : input  logic,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            i_val,
            o_val: child_out,
        );
        // Mux: select between child output and constant
        assign result = if i_sel ? 32'd999 : child_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // sel=0: should get child_out = i_val + 1 (from previous cycle)
        sim.set("i_val", Value::new(10, 32, false));
        sim.set("i_sel", Value::new(0, 1, false));
        sim.step(&clk);

        sim.set("i_val", Value::new(20, 32, false));
        sim.step(&clk);

        let r = sim.get("result").unwrap();
        // After 2 clocks: cycle1 latched i_val=10 → o_val=11, cycle2 latched i_val=20 → o_val=21
        // result should be 21 (child_out from cycle 2)
        assert_eq!(r, Value::new(21, 32, false), "config {:?}", config);

        // sel=1: should get 999
        sim.set("i_sel", Value::new(1, 1, false));
        sim.step(&clk);
        let r = sim.get("result").unwrap();
        assert_eq!(r, Value::new(999, 32, false), "config {:?}", config);
    }
}

// Faithful reproduction of heliodor var-redirect bug:
// 4-level hierarchy with store/load through var-redirected ports.
// TB → Core → MemStage (var redirect here) → internal comb
// The testbench has sequential memory that reads wdata from the core.
#[test]
fn four_level_var_redirect_wdata() {
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var ren  : logic;
        var rdata: logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            o_ren  : ren,
            i_rdata: rdata,
            o_result: result,
        );

        // Sequential memory + combinational read
        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Core (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        o_ren   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        // Pipeline controller
        var phase: logic<4>;
        var do_store: logic;
        var do_load : logic;
        var store_val: logic<32>;
        var rs2_data: logic<32>;

        always_ff {
            if_reset {
                phase = 0;
                do_store = 0;
                do_load  = 0;
                store_val = 0;
                rs2_data = 0;
            } else {
                case phase {
                    4'd0: {
                        store_val = 32'd42;
                        rs2_data = 32'd42;
                        phase = 1;
                    }
                    4'd1: {
                        do_store = 1;
                        phase = 2;
                    }
                    4'd2: {
                        do_store = 0;
                        phase = 3;
                    }
                    4'd3: phase = 4;
                    4'd4: {
                        do_load = 1;
                        phase = 5;
                    }
                    4'd5: {
                        do_load = 0;
                        phase = 6;
                    }
                    default: {}
                }
            }
        }

        // MemStage child - direct connection
        inst u_mem: MemStage (
            clk, rst,
            i_do_store: do_store,
            i_do_load : do_load,
            i_rs2_data: rs2_data,
            o_addr,
            o_wdata,
            o_wen,
            o_ren,
            i_rdata,
            o_result,
        );
    }

    module MemStage (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_rs2_data: input  logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_result  : output logic<32>,
    ) {
        assign o_addr  = 32'd0;
        assign o_wdata = i_rs2_data;
        assign o_wen   = i_do_store;
        assign o_ren   = i_do_load;

        always_ff {
            if_reset {
                o_result = 0;
            } else if i_do_load {
                o_result = i_rdata;
            }
        }
    }
    "#;

    // Indirect: MemStage outputs go through vars in Core
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var ren  : logic;
        var rdata: logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            o_ren  : ren,
            i_rdata: rdata,
            o_result: result,
        );

        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Core (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        o_ren   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<4>;
        var do_store: logic;
        var do_load : logic;
        var store_val: logic<32>;
        var rs2_data: logic<32>;

        always_ff {
            if_reset {
                phase = 0;
                do_store = 0;
                do_load  = 0;
                store_val = 0;
                rs2_data = 0;
            } else {
                case phase {
                    4'd0: {
                        store_val = 32'd42;
                        rs2_data = 32'd42;
                        phase = 1;
                    }
                    4'd1: {
                        do_store = 1;
                        phase = 2;
                    }
                    4'd2: {
                        do_store = 0;
                        phase = 3;
                    }
                    4'd3: phase = 4;
                    4'd4: {
                        do_load = 1;
                        phase = 5;
                    }
                    4'd5: {
                        do_load = 0;
                        phase = 6;
                    }
                    default: {}
                }
            }
        }

        // Var redirect: MemStage outputs → vars → assign → Core outputs
        var mem_wdata: logic<32>;
        var mem_addr : logic<32>;
        var mem_wen  : logic;
        var mem_ren  : logic;

        inst u_mem: MemStage (
            clk, rst,
            i_do_store: do_store,
            i_do_load : do_load,
            i_rs2_data: rs2_data,
            o_addr  : mem_addr,
            o_wdata : mem_wdata,
            o_wen   : mem_wen,
            o_ren   : mem_ren,
            i_rdata,
            o_result,
        );

        assign o_addr  = mem_addr;
        assign o_wdata = mem_wdata;
        assign o_wen   = mem_wen;
        assign o_ren   = mem_ren;
    }

    module MemStage (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_rs2_data: input  logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_result  : output logic<32>,
    ) {
        assign o_addr  = 32'd0;
        assign o_wdata = i_rs2_data;
        assign o_wen   = i_do_store;
        assign o_ren   = i_do_load;

        always_ff {
            if_reset {
                o_result = 0;
            } else if i_do_load {
                o_result = i_rdata;
            }
        }
    }
    "#;

    for config in Config::all() {
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::<std::io::Empty>::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();
        sim_d.step(&rst_d);
        for _ in 0..20 {
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::<std::io::Empty>::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();
        sim_i.step(&rst_i);
        for _ in 0..20 {
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        assert_eq!(
            rd, ri,
            "4-level var-redirect wdata mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// Test: Adding an unused always_ff to a passthrough module should not
// change behavior. Verifies merged JIT handles passthrough modules
// with both comb and event statements correctly.
#[test]
fn passthrough_with_unused_ff() {
    let code_comb_only = r#"
    module Passthrough (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        o_val : output logic<32>,
    ) {
        assign o_val = i_val;
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        var pt_out: logic<32>;

        inst u_child: Child (clk, rst, o_val: child_out);
        inst u_pt: Passthrough (clk, rst, i_val: child_out, o_val: pt_out);
        assign result = pt_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset { o_val = 0; }
            else     { o_val = o_val + 1; }
        }
    }
    "#;

    let code_with_ff = r#"
    module Passthrough (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        o_val : output logic<32>,
    ) {
        assign o_val = i_val;
        var dummy: logic;
        always_ff {
            if_reset { dummy = 0; }
            else     { dummy = 0; }
        }
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        var pt_out: logic<32>;

        inst u_child: Child (clk, rst, o_val: child_out);
        inst u_pt: Passthrough (clk, rst, i_val: child_out, o_val: pt_out);
        assign result = pt_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset { o_val = 0; }
            else     { o_val = o_val + 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir_a = analyze(code_comb_only, &config);
        let mut sim_a = Simulator::<std::io::Empty>::new(ir_a, None);
        let ir_b = analyze(code_with_ff, &config);
        let mut sim_b = Simulator::<std::io::Empty>::new(ir_b, None);

        let clk_a = sim_a.get_clock("clk").unwrap();
        let rst_a = sim_a.get_reset("rst").unwrap();
        let clk_b = sim_b.get_clock("clk").unwrap();
        let rst_b = sim_b.get_reset("rst").unwrap();

        sim_a.step(&rst_a);
        sim_b.step(&rst_b);

        for cycle in 0..10 {
            sim_a.step(&clk_a);
            sim_b.step(&clk_b);
            let va = sim_a.get("result").unwrap().payload_u64();
            let vb = sim_b.get("result").unwrap().payload_u64();
            assert_eq!(
                va, vb,
                "JIT={} 4state={} cycle={}: comb_only={} with_ff={} (should match)",
                config.use_jit, config.use_4state, cycle, va, vb
            );
        }
    }
}

// Test: Store through passthrough-with-FF, then load back.
// Verifies that merged JIT on the passthrough doesn't break
// the store→memory→load chain.
#[test]
fn store_load_through_passthrough_with_ff() {
    let code = r#"
    module Passthrough (
        clk      : input  clock,
        rst      : input  reset,
        i_addr   : input  logic<32>,
        i_wdata  : input  logic<32>,
        i_wen    : input  logic,
        i_ren    : input  logic,
        o_addr   : output logic<32>,
        o_wdata  : output logic<32>,
        o_wen    : output logic,
        o_ren    : output logic,
        i_rdata  : input  logic<32>,
        o_rdata  : output logic<32>,
    ) {
        assign o_addr  = i_addr;
        assign o_wdata = i_wdata;
        assign o_wen   = i_wen;
        assign o_ren   = i_ren;
        assign o_rdata = i_rdata;
        // Unused FF that triggers merged JIT
        var dummy: logic;
        always_ff {
            if_reset { dummy = 0; }
            else     { dummy = 0; }
        }
    }

    module Core (
        clk    : input  clock,
        rst    : input  reset,
        o_addr : output logic<32>,
        o_wdata: output logic<32>,
        o_wen  : output logic,
        o_ren  : output logic,
        i_rdata: input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<4>;
        var stored_val: logic<32>;
        always_ff {
            if_reset {
                phase = 0;
                o_addr = 0;
                o_wdata = 0;
                o_wen = 0;
                o_ren = 0;
                stored_val = 0;
            } else {
                o_wen = 0;
                o_ren = 0;
                case phase {
                    4'd0: { o_addr = 32'd0; o_wdata = 32'd42; o_wen = 1; phase = 4'd1; }
                    4'd1: { phase = 4'd2; }
                    4'd2: { o_addr = 32'd0; o_wdata = 32'd100; o_wen = 1; phase = 4'd3; }
                    4'd3: { phase = 4'd4; }
                    4'd4: { o_addr = 32'd0; o_ren = 1; phase = 4'd5; }
                    4'd5: { stored_val = i_rdata; phase = 4'd6; }
                    default: {}
                }
            }
        }
        assign o_result = stored_val;
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var core_addr : logic<32>;
        var core_wdata: logic<32>;
        var core_wen  : logic;
        var core_ren  : logic;
        var pt_rdata  : logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr: core_addr, o_wdata: core_wdata,
            o_wen: core_wen, o_ren: core_ren,
            i_rdata: pt_rdata,
            o_result: result,
        );

        var ext_addr : logic<32>;
        var ext_wdata: logic<32>;
        var ext_wen  : logic;
        var ext_ren  : logic;
        var ext_rdata: logic<32>;

        inst u_pt: Passthrough (
            clk, rst,
            i_addr: core_addr, i_wdata: core_wdata,
            i_wen: core_wen, i_ren: core_ren,
            o_addr: ext_addr, o_wdata: ext_wdata,
            o_wen: ext_wen, o_ren: ext_ren,
            i_rdata: ext_rdata,
            o_rdata: pt_rdata,
        );

        // Simple 1-word memory
        var mem: logic<32>;
        always_ff {
            if_reset { mem = 0; }
            else if ext_wen { mem = ext_wdata; }
        }
        assign ext_rdata = mem;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..10 {
            sim.step(&clk);
        }
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 100,
            "JIT={} 4state={}: expected 100 (second store), got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

// Same as above but with combinational memory read (like heliodor testbenches).
#[test]
fn store_load_comb_mem_through_passthrough_with_ff() {
    let code = r#"
    module Passthrough (
        clk      : input  clock,
        rst      : input  reset,
        i_addr   : input  logic<32>,
        i_wdata  : input  logic<32>,
        i_wen    : input  logic,
        i_ren    : input  logic,
        o_addr   : output logic<32>,
        o_wdata  : output logic<32>,
        o_wen    : output logic,
        o_ren    : output logic,
        i_rdata  : input  logic<32>,
        o_rdata  : output logic<32>,
    ) {
        assign o_addr  = i_addr;
        assign o_wdata = i_wdata;
        assign o_wen   = i_wen;
        assign o_ren   = i_ren;
        assign o_rdata = i_rdata;
        var dummy: logic;
        always_ff {
            if_reset { dummy = 0; }
            else     { dummy = 0; }
        }
    }

    module Core (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        o_ren   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<4>;
        var stored_val: logic<32>;
        always_ff {
            if_reset {
                phase = 0;
                o_addr = 0;
                o_wdata = 0;
                o_wen = 0;
                o_ren = 0;
                stored_val = 0;
            } else {
                o_wen = 0;
                o_ren = 0;
                case phase {
                    4'd0: { o_addr = 32'd0; o_wdata = 32'd42; o_wen = 1; phase = 4'd1; }
                    4'd1: { phase = 4'd2; }
                    4'd2: { o_addr = 32'd0; o_wdata = 32'd100; o_wen = 1; phase = 4'd3; }
                    4'd3: { phase = 4'd4; }
                    4'd4: { o_addr = 32'd0; o_ren = 1; phase = 4'd5; }
                    4'd5: { stored_val = i_rdata; phase = 4'd6; }
                    default: {}
                }
            }
        }
        assign o_result = stored_val;
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var core_addr : logic<32>;
        var core_wdata: logic<32>;
        var core_wen  : logic;
        var core_ren  : logic;
        var pt_rdata  : logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr: core_addr, o_wdata: core_wdata,
            o_wen: core_wen, o_ren: core_ren,
            i_rdata: pt_rdata,
            o_result: result,
        );

        var ext_addr : logic<32>;
        var ext_wdata: logic<32>;
        var ext_wen  : logic;
        var ext_ren  : logic;
        var ext_rdata: logic<32>;

        inst u_pt: Passthrough (
            clk, rst,
            i_addr: core_addr, i_wdata: core_wdata,
            i_wen: core_wen, i_ren: core_ren,
            o_addr: ext_addr, o_wdata: ext_wdata,
            o_wen: ext_wen, o_ren: ext_ren,
            i_rdata: ext_rdata,
            o_rdata: pt_rdata,
        );

        // Combinational memory (read is comb, write is FF)
        var mem: logic<32>;
        always_ff {
            if_reset { mem = 0; }
            else if ext_wen { mem = ext_wdata; }
        }
        // Comb read: rdata reflects current mem value
        assign ext_rdata = mem;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..10 {
            sim.step(&clk);
        }
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 100,
            "JIT={} 4state={}: expected 100 (second store), got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

#[test]
fn readonly_cache_fill() {
    let code = "
    module Cache (
        clk: input clock, rst: input reset,
        i_ren: input logic,
        o_val: output logic<32>,
        o_stall: output logic,
        o_mem_addr: output logic<32>,
        o_mem_ren: output logic,
        i_mem_rdata: input logic<32>,
    ) {
        var data: logic<32> [8];
        var valid: logic;
        var state: logic<2>;
        var fill_count: logic<3>;
        let hit: logic = valid;
        let miss: logic = i_ren && !hit && state == 2'd0;
        let filling: logic = state == 2'd1;
        always_ff (clk, rst) {
            if_reset {
                state = 0; fill_count = 0; valid = 0;
                for i: i32 in 0..8 { data[i] = 0; }
            } else {
                case state {
                    2'd0: { if miss { fill_count = 0; state = 2'd1; } }
                    2'd1: {
                        data[fill_count] = i_mem_rdata;
                        if fill_count == 3'd7 { valid = 1; state = 2'd2; }
                        else { fill_count = fill_count + 3'd1; }
                    }
                    2'd2: { state = 0; }
                    default: { state = 0; }
                }
            }
        }
        assign o_val = data[1];
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {29'd0, fill_count} : 32'd0;
        assign o_mem_ren = filling;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<32>) {
        var ren: logic; var stall: logic; var val: logic<32>;
        var mem_addr: logic<32>; var mem_ren: logic; var mem_rdata: logic<32>;
        inst u: Cache (clk: clk, rst: rst, i_ren: ren,
            o_val: val, o_stall: stall,
            o_mem_addr: mem_addr, o_mem_ren: mem_ren, i_mem_rdata: mem_rdata);
        var mem: logic<32> [8];
        assign mem_rdata = mem[mem_addr[2:0]];
        var tc: logic<8>; var stored: logic<32>;
        always_ff (clk, rst) {
            if_reset { tc = 0; ren = 0; stored = 0;
                for i: i32 in 0..8 { mem[i] = {24'd0, i[7:0]} + 32'd10; }
            } else {
                ren = 0; if stall { ren = 1; }
                if !stall { tc = tc + 8'd1; }
                case tc {
                    8'd1: { ren = 1; }
                    8'd3: { stored = val; }
                    default: {}
                }
            }
        }
        assign result = stored;
    }
    ";

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..15 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 11,
            "JIT={} 4state={}: expected 11, got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

#[test]
fn ff_comb_let_basic() {
    // Simplest case: always_ff variable read by a let (comb) declaration
    let code = "
    module Top (clk: input clock, rst: input reset, result: output logic<8>) {
        var cnt: logic<8>;
        let doubled: logic<8> = cnt + cnt;
        always_ff (clk, rst) {
            if_reset { cnt = 0; }
            else { cnt = cnt + 8'd1; }
        }
        assign result = doubled;
    }
    ";
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        // After 5 clocks: cnt=5, doubled=10
        assert_eq!(
            result, 10,
            "JIT={} 4state={}: expected 10, got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// Read-only cache with tag/index address decomposition (like heliodor icache).
/// Tests that fill_count-driven o_mem_addr propagates through comb to update
/// i_mem_rdata each fill cycle, so data[0] != data[1].
#[test]
fn readonly_cache_fill_with_tags() {
    let code = "
    module Cache (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>, i_ren: input logic,
        o_rdata: output logic<64>, o_stall: output logic,
        o_mem_addr: output logic<64>, o_mem_ren: output logic,
        i_mem_rdata: input logic<64>,
    ) {
        let tag: logic<54> = i_addr[63:10];
        let index: logic<4> = i_addr[9:6];
        let offset: logic<3> = i_addr[5:3];
        let data_idx: logic<7> = {index, offset};
        var valid: logic<16>;
        var tags: logic<54> [16];
        var data: logic<64> [128];
        let cache_hit: logic = valid[index] && tags[index] == tag;
        var state: logic<2>;
        var fill_count: logic<3>;
        var fill_index: logic<4>;
        var fill_tag: logic<54>;
        let fill_data_idx: logic<7> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && state == 2'd0;
        let filling: logic = state == 2'd1;
        always_ff (clk, rst) {
            if_reset {
                state = 0; fill_count = 0; fill_index = 0; fill_tag = 0; valid = 0;
                for i: i32 in 0..16 { tags[i] = 0; }
                for i: i32 in 0..128 { data[i] = 0; }
            } else {
                case state {
                    2'd0: { if miss { fill_index = index; fill_tag = tag; fill_count = 0; state = 2'd1; } }
                    2'd1: {
                        data[fill_data_idx] = i_mem_rdata;
                        if fill_count == 3'd7 { tags[fill_index] = fill_tag; valid[fill_index] = 1; state = 2'd2; }
                        else { fill_count = fill_count + 3'd1; }
                    }
                    2'd2: { state = 0; }
                    default: { state = 0; }
                }
            }
        }
        assign o_rdata = if cache_hit ? data[data_idx] : i_mem_rdata;
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {fill_tag, fill_index, fill_count, 3'b000}
                          : if i_ren && !cache_hit ? {tag, index, 3'd0, 3'b000}
                          : '0;
        assign o_mem_ren = if filling ? 1'b1 : if i_ren && !cache_hit ? 1'b1 : 1'b0;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var addr: logic<64>; var ren: logic; var rdata: logic<64>;
        var stall: logic; var mem_addr: logic<64>; var mem_ren: logic;
        var mem_rdata: logic<64>;
        inst u: Cache (clk: clk, rst: rst, i_addr: addr, i_ren: ren,
            o_rdata: rdata, o_stall: stall,
            o_mem_addr: mem_addr, o_mem_ren: mem_ren, i_mem_rdata: mem_rdata);
        var mem: logic<64> [256];
        assign mem_rdata = mem[mem_addr[10:3]];
        var tc: logic<8>; var r1: logic<64>; var r2: logic<64>;
        always_ff (clk, rst) {
            if_reset { tc = 0; ren = 0; addr = 0; r1 = 0; r2 = 0;
                for i: i32 in 0..256 {
                    if i == 0 { mem[i] = 64'h0000_0000_0000_AAAA; }
                    else if i == 1 { mem[i] = 64'h0000_0000_0000_BBBB; }
                    else { mem[i] = 0; }
                }
            } else {
                ren = 0;
                if stall { ren = 1; }
                if !stall { tc = tc + 8'd1; }
                case tc {
                    8'd1: { addr = 64'h0; ren = 1; }
                    8'd3: { addr = 64'h0; ren = 1; }
                    8'd4: { r1 = rdata; }
                    8'd6: { addr = 64'h8; ren = 1; }
                    8'd7: { r2 = rdata; }
                    default: {}
                }
            }
        }
        assign result = r2;
    }
    ";
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..30 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 0xBBBB,
            "JIT={} 4state={}: expected 0xBBBB, got 0x{:x}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// 3-level hierarchy: TestTop → Harness → Cache (like heliodor's test structure)
#[test]
fn readonly_cache_fill_3level() {
    let code = "
    module Cache (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>, i_ren: input logic,
        o_rdata: output logic<64>, o_stall: output logic,
        o_mem_addr: output logic<64>, o_mem_ren: output logic,
        i_mem_rdata: input logic<64>,
    ) {
        let tag: logic<54> = i_addr[63:10];
        let index: logic<4> = i_addr[9:6];
        let offset: logic<3> = i_addr[5:3];
        let data_idx: logic<7> = {index, offset};
        var valid: logic<16>;
        var tags: logic<54> [16];
        var data: logic<64> [128];
        let cache_hit: logic = valid[index] && tags[index] == tag;
        enum State: logic<2> { IDLE = 2'd0, FILL = 2'd1, DONE = 2'd2 }
        var state: State;
        var fill_count: logic<3>;
        var fill_index: logic<4>;
        var fill_tag: logic<54>;
        let fill_data_idx: logic<7> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && state == State::IDLE;
        let filling: logic = state == State::FILL;
        always_ff (clk, rst) {
            if_reset {
                state = State::IDLE; fill_count = 0; fill_index = 0; fill_tag = 0; valid = 0;
                for i: i32 in 0..16 { tags[i] = 0; }
                for i: i32 in 0..128 { data[i] = 0; }
            } else {
                case state {
                    State::IDLE: { if miss { fill_index = index; fill_tag = tag; fill_count = 0; state = State::FILL; } }
                    State::FILL: {
                        data[fill_data_idx] = i_mem_rdata;
                        if fill_count == 3'd7 { tags[fill_index] = fill_tag; valid[fill_index] = 1; state = State::DONE; }
                        else { fill_count = fill_count + 3'd1; }
                    }
                    State::DONE: { state = State::IDLE; }
                    default: { state = State::IDLE; }
                }
            }
        }
        assign o_rdata = if cache_hit ? data[data_idx] : i_mem_rdata;
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {fill_tag, fill_index, fill_count, 3'b000}
                          : if i_ren && !cache_hit ? {tag, index, 3'd0, 3'b000}
                          : '0;
        assign o_mem_ren = if filling ? 1'b1 : if i_ren && !cache_hit ? 1'b1 : 1'b0;
    }
    module Harness (
        clk: input clock, rst: input reset,
        o_r1: output logic<64>, o_r2: output logic<64>,
    ) {
        var addr: logic<64>; var ren: logic; var rdata: logic<64>;
        var stall: logic; var mem_addr: logic<64>; var mem_ren: logic;
        var mem_rdata: logic<64>;
        inst dut: Cache (clk: clk, rst: rst, i_addr: addr, i_ren: ren,
            o_rdata: rdata, o_stall: stall,
            o_mem_addr: mem_addr, o_mem_ren: mem_ren, i_mem_rdata: mem_rdata);
        var mem: logic<64> [256];
        assign mem_rdata = mem[mem_addr[10:3]];
        var tc: logic<8>; var r1_val: logic<64>; var r2_val: logic<64>;
        always_ff (clk, rst) {
            if_reset { tc = 0; ren = 0; addr = 0; r1_val = 0; r2_val = 0;
                for i: i32 in 0..256 {
                    if i == 0 { mem[i] = 64'h0000_0000_0000_AAAA; }
                    else if i == 1 { mem[i] = 64'h0000_0000_0000_BBBB; }
                    else { mem[i] = 0; }
                }
            } else {
                ren = 0;
                if stall { ren = 1; }
                if !stall { tc = tc + 8'd1; }
                case tc {
                    8'd1: { addr = 64'h0; ren = 1; }
                    8'd3: { addr = 64'h0; ren = 1; }
                    8'd4: { r1_val = rdata; }
                    8'd6: { addr = 64'h8; ren = 1; }
                    8'd7: { r2_val = rdata; }
                    default: {}
                }
            }
        }
        assign o_r1 = r1_val;
        assign o_r2 = r2_val;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var r1: logic<64>; var r2: logic<64>;
        inst h: Harness (clk: clk, rst: rst, o_r1: r1, o_r2: r2);
        assign result = r2;
    }
    ";
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..30 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 0xBBBB,
            "JIT={} 4state={}: expected 0xBBBB, got 0x{:x}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// Bit select on a wide FF variable used in ternary: `if pc[1] ? data[31:16] : data[15:0]`
/// Tests that pc[1] correctly selects upper/lower halfword as pc increments by 2.
/// The result is captured by an FF so we observe the pre-event comb value.
#[test]
fn bit_select_ternary_wide_var() {
    let code = r#"
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var pc: logic<64>;
        var data: logic<32>;

        always_ff (clk, rst) {
            if_reset {
                pc = 0;
                data = 32'hBBBBAAAA;
            } else {
                pc = pc + 64'd2;
            }
        }

        // Select halfword based on pc[1]
        let half: logic<16> = if pc[1] ? data[31:16] : data[15:0];

        // Capture sequence: r0=half@pc=0, r1=half@pc=2, r2=half@pc=4
        var r0: logic<16>;
        var r1: logic<16>;
        var r2: logic<16>;
        var cycle: logic<8>;
        always_ff (clk, rst) {
            if_reset { r0 = 0; r1 = 0; r2 = 0; cycle = 0; }
            else {
                case cycle {
                    8'd0: { r0 = half; }
                    8'd1: { r1 = half; }
                    8'd2: { r2 = half; }
                    default: {}
                }
                cycle = cycle + 8'd1;
            }
        }

        // Pack into result: {r2, r1, r0} with padding
        assign result = {16'd0, r2, r1, r0};
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..10 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let r0 = (result >> 0) & 0xFFFF;
        let r1 = (result >> 16) & 0xFFFF;
        let r2 = (result >> 32) & 0xFFFF;
        // pc=0: pc[1]=0 → lower=0xAAAA. pc=2: pc[1]=1 → upper=0xBBBB. pc=4: pc[1]=0 → lower=0xAAAA
        assert_eq!(
            r0, 0xAAAA,
            "JIT={} 4st={}: pc=0 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r0
        );
        assert_eq!(
            r1, 0xBBBB,
            "JIT={} 4st={}: pc=2 expected 0xBBBB got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0xAAAA,
            "JIT={} 4st={}: pc=4 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
    }
}

/// Same as above but halfword select is on a child module's comb output.
/// Reproduces the heliodor pattern: icache.o_rdata → parent bit select → pipeline.
#[test]
fn bit_select_child_output() {
    let code = r#"
    module Child (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>,
        o_data: output logic<32>,
    ) {
        var store: logic<32>;
        always_ff (clk, rst) {
            if_reset { store = 32'hBBBBAAAA; }
        }
        assign o_data = store;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var pc: logic<64>;
        always_ff (clk, rst) {
            if_reset { pc = 0; }
            else { pc = pc + 64'd2; }
        }

        var child_data: logic<32>;
        inst u_child: Child (clk, rst, i_addr: pc, o_data: child_data);

        // Select halfword based on pc[1] from child output
        let half: logic<16> = if pc[1] ? child_data[31:16] : child_data[15:0];

        var r0: logic<16>;
        var r1: logic<16>;
        var r2: logic<16>;
        var cycle: logic<8>;
        always_ff (clk, rst) {
            if_reset { r0 = 0; r1 = 0; r2 = 0; cycle = 0; }
            else {
                case cycle {
                    8'd0: { r0 = half; }
                    8'd1: { r1 = half; }
                    8'd2: { r2 = half; }
                    default: {}
                }
                cycle = cycle + 8'd1;
            }
        }
        assign result = {16'd0, r2, r1, r0};
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..10 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let r0 = (result >> 0) & 0xFFFF;
        let r1 = (result >> 16) & 0xFFFF;
        let r2 = (result >> 32) & 0xFFFF;
        assert_eq!(
            r0, 0xAAAA,
            "JIT={} 4st={}: pc=0 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r0
        );
        assert_eq!(
            r1, 0xBBBB,
            "JIT={} 4st={}: pc=2 expected 0xBBBB got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0xAAAA,
            "JIT={} 4st={}: pc=4 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
    }
}

/// Cache + halfword select + stall: reproduces heliodor compressed instruction issue.
/// icache stalls for fill, then parent selects halfword based on pc[1].
#[test]
fn cache_halfword_select_with_stall() {
    let code = r#"
    module Cache32 (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>, i_ren: input logic,
        o_rdata: output logic<32>, o_stall: output logic,
        o_mem_addr: output logic<64>, i_mem_rdata: input logic<32>, o_mem_ren: output logic,
    ) {
        let index: logic<4> = i_addr[9:6];
        let offset: logic<4> = i_addr[5:2];
        let data_idx: logic<8> = {index, offset};
        var valid: logic [16];
        var data: logic<32> [256];
        let cache_hit: logic = valid[index];
        var filling: logic;
        var fill_count: logic<4>;
        var fill_index: logic<4>;
        let fill_data_idx: logic<8> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && !filling;
        always_ff (clk, rst) {
            if_reset {
                filling = 0; fill_count = 0; fill_index = 0;
                for i: i32 in 0..16 { valid[i] = 0; }
                for i: i32 in 0..256 { data[i] = 0; }
            } else if filling {
                data[fill_data_idx] = i_mem_rdata;
                if fill_count == 4'd15 {
                    valid[fill_index] = 1; filling = 0;
                } else { fill_count = fill_count + 4'd1; }
            } else if miss {
                fill_index = index; fill_count = 0; filling = 1;
            }
        }
        assign o_rdata = if cache_hit ? data[data_idx] : i_mem_rdata;
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {50'd0, fill_index, fill_count, 2'b00} : i_addr;
        assign o_mem_ren = i_ren || filling;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var stall: logic;
        var cache_rdata: logic<32>;
        var mem_addr: logic<64>;
        var mem_ren: logic;

        // Instruction ROM: 32-bit words containing two 16-bit halves
        var mem_rdata: logic<32>;
        always_comb {
            case mem_addr[7:0] {
                // Word at 0x00: lower=0x1111 upper=0x2222
                8'h00: mem_rdata = 32'h22221111;
                // Word at 0x04: lower=0x3333 upper=0x4444
                8'h04: mem_rdata = 32'h44443333;
                // Word at 0x08: lower=0x5555 upper=0x6666
                8'h08: mem_rdata = 32'h66665555;
                default: mem_rdata = 32'h00000000;
            }
        }

        // PC: increments by 2 (halfword) when not stalled
        var pc: logic<64>;
        always_ff (clk, rst) {
            if_reset { pc = 0; }
            else if !stall { pc = pc + 64'd2; }
        }

        inst u_cache: Cache32 (clk, rst,
            i_addr: pc, i_ren: 1'b1,
            o_rdata: cache_rdata, o_stall: stall,
            o_mem_addr: mem_addr, i_mem_rdata: mem_rdata, o_mem_ren: mem_ren);

        // Halfword select based on pc[1]
        let half: logic<16> = if pc[1] ? cache_rdata[31:16] : cache_rdata[15:0];

        // Capture first 4 halfwords into r0-r3
        var r0: logic<16>; var r1: logic<16>;
        var r2: logic<16>; var r3: logic<16>;
        var tc: logic<8>;
        always_ff (clk, rst) {
            if_reset { r0 = 0; r1 = 0; r2 = 0; r3 = 0; tc = 0; }
            else if !stall {
                case tc {
                    8'd0: { r0 = half; }
                    8'd1: { r1 = half; }
                    8'd2: { r2 = half; }
                    8'd3: { r3 = half; }
                    default: {}
                }
                tc = tc + 8'd1;
            }
        }
        assign result = {r3, r2, r1, r0};
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..40 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let r0 = (result >> 0) & 0xFFFF;
        let r1 = (result >> 16) & 0xFFFF;
        let r2 = (result >> 32) & 0xFFFF;
        let r3 = (result >> 48) & 0xFFFF;
        // PC increments by 2: 0x00→0x02→0x04→0x06
        // pc=0: word@0x00[15:0]=0x1111, pc=2: word@0x00[31:16]=0x2222
        // pc=4: word@0x04[15:0]=0x3333, pc=6: word@0x04[31:16]=0x4444
        assert_eq!(
            r0, 0x1111,
            "JIT={} 4st={}: pc=0 expected 0x1111 got 0x{:x}",
            config.use_jit, config.use_4state, r0
        );
        assert_eq!(
            r1, 0x2222,
            "JIT={} 4st={}: pc=2 expected 0x2222 got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0x3333,
            "JIT={} 4st={}: pc=4 expected 0x3333 got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
        assert_eq!(
            r3, 0x4444,
            "JIT={} 4st={}: pc=6 expected 0x4444 got 0x{:x}",
            config.use_jit, config.use_4state, r3
        );
    }
}

/// Reproduces JIT bug: halfword select with cache + expander child modules.
/// The `let curr_halfword` in parent changes merged JIT function behavior
/// for child modules (c_expander), causing incorrect results.
/// Non-idempotent: accumulator detects double-execution or missing execution.
#[test]
fn halfword_select_with_expander_child() {
    let code = r#"
    // Simplified icache: returns stored 32-bit word
    module SimpleCache (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>,
        o_data: output logic<32>,
        o_stall: output logic,
    ) {
        var store: logic<32> [4];
        var ready: logic;
        always_ff (clk, rst) {
            if_reset {
                ready = 0;
                store[0] = 32'hBBBBAAAA; // word at addr 0: lower=0xAAAA, upper=0xBBBB
                store[1] = 32'hDDDDCCCC; // word at addr 4: lower=0xCCCC, upper=0xDDDD
                store[2] = 32'h00000000;
                store[3] = 32'h00000000;
            } else {
                ready = 1;
            }
        }
        let idx: logic<2> = i_addr[3:2];
        assign o_data = store[idx];
        assign o_stall = !ready;
    }

    // Simplified expander: zero-extends the 16-bit input to 32 bits
    module Expander (
        i_half: input logic<16>,
        o_full: output logic<32>,
    ) {
        assign o_full = {16'd0, i_half};
    }

    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var pc: logic<64>;
        var stall: logic;
        always_ff (clk, rst) {
            if_reset { pc = 0; }
            else if !stall { pc = pc + 64'd2; }
        }

        var cache_data: logic<32>;
        inst u_cache: SimpleCache (clk, rst,
            i_addr: pc, o_data: cache_data, o_stall: stall);

        // Halfword select based on pc[1] -- this is the problematic pattern
        let curr_half: logic<16> = if pc[1] ? cache_data[31:16] : cache_data[15:0];

        var expanded: logic<32>;
        inst u_exp: Expander (i_half: curr_half, o_full: expanded);

        // Non-idempotent: accumulate expanded values
        var sum: logic<32>;
        var count: logic<8>;
        always_ff (clk, rst) {
            if_reset { sum = 0; count = 0; }
            else if !stall {
                if count <: 8'd4 {
                    sum = sum + expanded;
                    count = count + 8'd1;
                }
            }
        }
        // Expected: sum of first 4 halfwords
        // pc=0: half=0xAAAA, pc=2: half=0xBBBB, pc=4: half=0xCCCC, pc=6: half=0xDDDD
        // sum = 0xAAAA + 0xBBBB + 0xCCCC + 0xDDDD = 0x3110E
        assign result = {24'd0, count, sum};
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..20 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let count = (result >> 32) & 0xFF;
        let sum = result & 0xFFFFFFFF;
        let expected_sum: u64 = 0xAAAA + 0xBBBB + 0xCCCC + 0xDDDD;
        assert_eq!(
            count, 4,
            "JIT={} 4st={}: count expected 4 got {}",
            config.use_jit, config.use_4state, count
        );
        assert_eq!(
            sum, expected_sum,
            "JIT={} 4st={}: sum expected 0x{:x} got 0x{:x}",
            config.use_jit, config.use_4state, expected_sum, sum
        );
    }
}

// Regression: D-Cache write-through with parent-child hierarchy and comb feedback.
// Parent provides address to child (cache), child returns data.
// Tests that dynamic array index is correct across module boundary with JIT.
#[test]
fn dcache_write_through_hierarchy() {
    let code = r#"
    module Cache (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<64>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        i_ren: input logic,
        o_rdata: output logic<64>,
    ) {
        let data_idx: logic<4> = i_addr[6:3];

        var data: logic<64> [16];
        var valid: logic;

        always_ff (clk, rst) {
            if_reset {
                valid = 1'b0;
                for i: i32 in 0..16 { data[i] = 64'd0; }
            } else if i_wen {
                data[data_idx] = i_wdata;
                valid = 1'b1;
            }
        }

        assign o_rdata = if valid ? data[data_idx] : 64'd0;
    }

    module Top (
        clk: input clock,
        rst: input reset,
        i_cmd: input logic<3>,     // 0=nop, 1=write, 2=read
        i_addr: input logic<64>,
        i_wdata: input logic<64>,
        o_rdata: output logic<64>,
    ) {
        var cache_rdata: logic<64>;

        inst u_cache: Cache (
            clk, rst,
            i_addr  : i_addr,
            i_wdata : i_wdata,
            i_wen   : i_cmd == 3'd1,
            i_ren   : i_cmd == 3'd2,
            o_rdata : cache_rdata,
        );

        // Comb feedback: cache output → parent output
        assign o_rdata = cache_rdata;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write 42 to addr 0x00 (data_idx=0)
        sim.set("i_cmd", Value::new(1, 3, false));
        sim.set("i_addr", Value::new(0x00, 64, false));
        sim.set("i_wdata", Value::new(42, 64, false));
        sim.step(&clk);

        // Write 99 to addr 0x08 (data_idx=1)
        sim.set("i_cmd", Value::new(1, 3, false));
        sim.set("i_addr", Value::new(0x08, 64, false));
        sim.set("i_wdata", Value::new(99, 64, false));
        sim.step(&clk);

        // Read from addr 0x08 (data_idx=1)
        sim.set("i_cmd", Value::new(2, 3, false));
        sim.set("i_addr", Value::new(0x08, 64, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 99,
            "JIT={} 4st={}: expected 99 at addr 0x08, got {} — wrong index or stale data",
            config.use_jit, config.use_4state, rdata
        );

        // Read from addr 0x00 (data_idx=0)
        sim.set("i_addr", Value::new(0x00, 64, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 42,
            "JIT={} 4st={}: expected 42 at addr 0x00, got {}",
            config.use_jit, config.use_4state, rdata
        );
    }
}

// Regression: non-FF array in always_ff with comb read via DynamicVariable.
// The FF table marks these as non-FF (only written in always_ff, never read there),
// placing them in the comb buffer. Tests that the JIT merged function handles
// this correctly for large arrays in deep module hierarchies.
#[test]
fn nonff_dynamic_array_deep_hierarchy() {
    // 3-level hierarchy: Top → Pipeline → Cache
    // Cache has a large array (128 elements) written in always_ff and read in comb
    // Pipeline adds sibling logic that increases comb buffer size
    let code = r#"
    module Cache (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<64>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        i_ren: input logic,
        o_rdata: output logic<64>,
        o_stall: output logic,
    ) {
        // Mirror heliodor D-Cache structure
        let tag      : logic<54> = i_addr[63:10];
        let index    : logic<4>  = i_addr[9:6];
        let offset_w : logic<3>  = i_addr[5:3];
        let data_idx : logic<7>  = {index, offset_w};

        var valid: logic [16];
        var tags : logic<54> [16];
        var data : logic<64> [128];

        let cache_hit: logic = valid[index] && tags[index] == tag;

        enum State: logic<2> {
            IDLE = 2'd0,
            FILL = 2'd1,
            DONE = 2'd2,
        }
        var state: State;
        var fill_count: logic<3>;
        var fill_index: logic<4>;
        var fill_tag  : logic<54>;
        let fill_data_idx: logic<7> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && state == State::IDLE;

        always_ff (clk, rst) {
            if_reset {
                state = State::IDLE;
                fill_count = 3'd0;
                fill_index = 4'd0;
                fill_tag   = '0;
                for i: i32 in 0..16 {
                    valid[i] = 1'b0;
                    tags[i]  = 54'd0;
                }
                for i: i32 in 0..128 {
                    data[i] = 64'd0;
                }
            } else {
                case state {
                    State::IDLE: {
                        if miss {
                            fill_index = index;
                            fill_tag   = tag;
                            fill_count = 3'd0;
                            state = State::FILL;
                        }
                        if i_wen && cache_hit {
                            data[data_idx] = i_wdata;
                        }
                    }
                    State::FILL: {
                        data[fill_data_idx] = i_wdata;
                        if fill_count == 3'd7 {
                            tags[fill_index]  = fill_tag;
                            valid[fill_index] = 1'b1;
                            state = State::DONE;
                        } else {
                            fill_count = fill_count + 3'd1;
                        }
                    }
                    State::DONE: {
                        state = State::IDLE;
                    }
                    default: {
                        state = State::IDLE;
                    }
                }
            }
        }

        assign o_rdata = if cache_hit ? data[data_idx] : 64'd0;
        assign o_stall = (state == State::FILL) || miss;
    }

    // Pipeline module with many variables to inflate comb buffer
    module Pipeline (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<7>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        o_rdata: output logic<64>,
    ) {
        // Padding variables to shift cache offsets
        var pad_a: logic<64>;
        var pad_b: logic<64>;
        var pad_c: logic<64>;
        var pad_d: logic<64>;
        var pad_e: logic<64>;
        var pad_f: logic<64>;
        var pad_g: logic<64>;
        var pad_h: logic<64>;

        always_ff (clk, rst) {
            if_reset {
                pad_a = 64'd0; pad_b = 64'd0; pad_c = 64'd0; pad_d = 64'd0;
                pad_e = 64'd0; pad_f = 64'd0; pad_g = 64'd0; pad_h = 64'd0;
            } else {
                pad_a = pad_a + 64'd1;
                pad_b = pad_b + 64'd2;
                pad_c = pad_c + 64'd3;
                pad_d = pad_d + 64'd4;
                pad_e = pad_e + 64'd5;
                pad_f = pad_f + 64'd6;
                pad_g = pad_g + 64'd7;
                pad_h = pad_h + 64'd8;
            }
        }

        var cache_rdata: logic<64>;
        var cache_stall: logic;
        inst u_cache: Cache (
            clk, rst,
            i_addr  : {57'd0, i_addr},
            i_wdata : i_wdata,
            i_wen   : i_wen,
            i_ren   : !i_wen,
            o_rdata : cache_rdata,
            o_stall : cache_stall,
        );

        assign o_rdata = cache_rdata;
    }

    module Top (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<7>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        o_rdata: output logic<64>,
    ) {
        var pipeline_rdata: logic<64>;

        inst u_pipeline: Pipeline (
            clk, rst,
            i_addr  : i_addr,
            i_wdata : i_wdata,
            i_wen   : i_wen,
            o_rdata : pipeline_rdata,
        );

        assign o_rdata = pipeline_rdata;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // addr layout: i_addr[6:3]=index(4bit), i_addr[2:0]=000 (byte aligned)
        // For cache index 4, offset 0: addr = (4<<6)|(0<<3) = 0x100 → i_addr bits[6:0] = 0b1000000 = 0x40
        // For cache index 4, offset 1: addr = (4<<6)|(1<<3) = 0x108 → i_addr bits[6:0] = 0b1001000 = 0x48

        // Step 1: Read from addr 0x40 (index 4, offset 0) → cache miss → triggers fill
        sim.set("i_addr", Value::new(0x40, 7, false));
        sim.set("i_wdata", Value::new(42, 64, false)); // fill data for all 8 words
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk); // miss detected
        // Fill takes 8 cycles (fill_count 0..7)
        for _ in 0..9 {
            sim.step(&clk);
        }
        // State should be IDLE now, valid[4]=1

        // Step 2: Write 42 to addr 0x40 (cache hit, write-through)
        sim.set("i_addr", Value::new(0x40, 7, false));
        sim.set("i_wdata", Value::new(42, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);

        // Step 3: Write 99 to addr 0x48 (cache hit, write-through, different offset)
        sim.set("i_addr", Value::new(0x48, 7, false));
        sim.set("i_wdata", Value::new(99, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);

        // Step 4: Read from addr 0x48 → should get 99
        sim.set("i_addr", Value::new(0x48, 7, false));
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 99,
            "JIT={} 4st={}: expected 99 at addr 0x48, got {} (wrong cache offset?)",
            config.use_jit, config.use_4state, rdata
        );

        // Step 5: Read from addr 0x40 → should get 42
        sim.set("i_addr", Value::new(0x40, 7, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 42,
            "JIT={} 4st={}: expected 42 at addr 0x40, got {}",
            config.use_jit, config.use_4state, rdata
        );
    }
}

// Regression: child module with always_comb reading 64-bit input port bit-select.
// Tests that bit 63 of a 64-bit input is correctly read inside always_comb
// of a child module, even when the child is instantiated alongside other logic.
#[test]
fn child_always_comb_bit_select_64() {
    let code = r#"
    module Adder (
        i_a: input logic<64>,
        i_b: input logic<64>,
        o_result: output logic<64>,
        o_b_raw: output logic<64>,
    ) {
        var sa: logic;
        var sb: logic;
        var result: logic<64>;
        always_comb {
            sa = i_a[63];
            sb = i_b[63];
            // Simple test: output sign bits and input values
            result = {sa, sb, 62'd0};
        }
        assign o_result = result;
        assign o_b_raw = i_b;  // pass-through i_b for verification
    }

    module Top (
        clk: input clock,
        rst: input reset,
        i_op: input logic,
        i_a: input logic<64>,
        i_b: input logic<64>,
        o_result: output logic<64>,
        o_b_raw: output logic<64>,
    ) {
        // Some padding logic (like ALU) to make comb_statements more complex
        var alu_result: logic<64>;
        always_comb {
            if i_op {
                alu_result = i_a + i_b;
            } else {
                alu_result = i_a - i_b;
            }
        }

        var add_result: logic<64>;
        var b_raw: logic<64>;
        inst u_adder: Adder (
            i_a: i_a,
            i_b: i_b,
            o_result: add_result,
            o_b_raw: b_raw,
        );

        // Use both results to prevent DCE
        assign o_result = if i_op ? add_result : alu_result;
        assign o_b_raw = b_raw;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);

        // Test: i_a = +5.0 (bit63=0), i_b = -5.0 (bit63=1)
        sim.set("i_op", Value::new(1, 1, false));
        sim.set("i_a", Value::new(0x4014000000000000, 64, false)); // +5.0
        sim.set("i_b", Value::new(0xC014000000000000, 64, false)); // -5.0
        sim.step(&clk);

        let result = sim.get("o_result").unwrap().payload_u64();
        let sa = (result >> 63) & 1;
        let sb = (result >> 62) & 1;
        let b_raw = sim.get("o_b_raw").unwrap().payload_u64();
        if sa != 0 || sb != 1 {
            panic!(
                "JIT={} 4st={}: sa={} (expected 0), sb={} (expected 1), result=0x{:016x}, b_raw=0x{:016x}",
                config.use_jit, config.use_4state, sa, sb, result, b_raw
            );
        }
    }
}

// Regression: D-Cache write-through pattern. Array fill + conditional write
// to different offset + read back. JIT ON was reading from wrong array index
// because the merged comb+event function miscomputed the dynamic index.
#[test]
fn dcache_write_through_pattern() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<7>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        i_ren: input logic,
        o_rdata: output logic<64>,
        o_hit: output logic,
    ) {
        // Simplified cache: 8 entries, indexed by i_addr[2:0]
        var data: logic<64> [8];
        var valid: logic;

        let index: logic<3> = i_addr[2:0];

        always_ff (clk, rst) {
            if_reset {
                valid = 1'b0;
                for i: i32 in 0..8 { data[i] = 64'd0; }
            } else {
                // Fill: write all entries on first write
                if i_wen && !valid {
                    data[index] = i_wdata;
                    valid = 1'b1;
                }
                // Write-through: update on write when valid
                if i_wen && valid {
                    data[index] = i_wdata;
                }
            }
        }

        let hit: logic = valid;
        assign o_hit = hit;
        assign o_rdata = if hit ? data[index] : 64'd0;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write 42 to index 0
        sim.set("i_addr", Value::new(0, 7, false));
        sim.set("i_wdata", Value::new(42, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.set("i_ren", Value::new(0, 1, false));
        sim.step(&clk);

        // Write 99 to index 1
        sim.set("i_addr", Value::new(1, 7, false));
        sim.set("i_wdata", Value::new(99, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);

        // Read from index 1
        sim.set("i_addr", Value::new(1, 7, false));
        sim.set("i_wen", Value::new(0, 1, false));
        sim.set("i_ren", Value::new(1, 1, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        let hit = sim.get("o_hit").unwrap().payload_u64();
        assert_eq!(
            hit, 1,
            "JIT={} 4st={}: cache should be valid",
            config.use_jit, config.use_4state
        );
        assert_eq!(
            rdata, 99,
            "JIT={} 4st={}: expected 99 at index 1, got {} (wrong index?)",
            config.use_jit, config.use_4state, rdata
        );

        // Read from index 0
        sim.set("i_addr", Value::new(0, 7, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 42,
            "JIT={} 4st={}: expected 42 at index 0, got {}",
            config.use_jit, config.use_4state, rdata
        );
    }
}

// Regression: dynamic array with 2 read ports reading different elements
#[test]
fn dynamic_array_two_read_ports() {
    let code = r#"
    module RegFile (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        var regs: logic<64> [32];

        always_comb {
            o_rs1_data = if i_wen && i_rs1_addr == i_wd_addr ? i_wd_data : regs[i_rs1_addr];
            o_rs2_data = if i_wen && i_rs2_addr == i_wd_addr ? i_wd_data : regs[i_rs2_addr];
        }

        always_ff (clk, rst) {
            if_reset {
                for i: i32 in 0..32 { regs[i] = 64'd0; }
            } else if i_wen {
                regs[i_wd_addr] = i_wd_data;
            }
        }
    }

    // Middle module that captures regfile outputs in pipeline registers
    module Decode (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        var rf_rs1: logic<64>;
        var rf_rs2: logic<64>;
        inst u_rf: RegFile (
            clk, rst,
            i_rs1_addr, i_rs2_addr,
            o_rs1_data: rf_rs1,
            o_rs2_data: rf_rs2,
            i_wd_addr, i_wd_data, i_wen,
        );

        // Pipeline register captures
        always_ff (clk, rst) {
            if_reset {
                o_rs1_data = 64'd0;
                o_rs2_data = 64'd0;
            } else {
                o_rs1_data = rf_rs1;
                o_rs2_data = rf_rs2;
            }
        }
    }

    // Top module
    module Top (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        inst u_dec: Decode (
            clk, rst,
            i_rs1_addr, i_rs2_addr,
            o_rs1_data, o_rs2_data,
            i_wd_addr, i_wd_data, i_wen,
        );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);

        // Write 0xAA to reg[1]
        sim.set("i_wd_addr", Value::new(1, 5, false));
        sim.set("i_wd_data", Value::new(0xAA, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk);

        // Write 0xBB to reg[2]
        sim.set("i_wd_addr", Value::new(2, 5, false));
        sim.set("i_wd_data", Value::new(0xBB, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk);

        // Read reg[1] via rs1 and reg[2] via rs2
        sim.set("i_rs1_addr", Value::new(1, 5, false));
        sim.set("i_rs2_addr", Value::new(2, 5, false));
        sim.step(&clk); // comb evaluates
        sim.step(&clk); // pipeline register captures
        sim.ensure_comb_updated();

        let rs1 = sim.get("o_rs1_data").unwrap().payload_u64();
        let rs2 = sim.get("o_rs2_data").unwrap().payload_u64();
        assert_eq!(
            rs1, 0xAA,
            "JIT={} 4st={}: rs1 expected 0xAA got 0x{:x}",
            config.use_jit, config.use_4state, rs1
        );
        assert_eq!(
            rs2, 0xBB,
            "JIT={} 4st={}: rs2 expected 0xBB got 0x{:x}",
            config.use_jit, config.use_4state, rs2
        );
    }
}

/// Same as dynamic_array_harness_driven but with 3-level hierarchy:
/// Outer → Harness → RegFile (matches testbench structure).
#[test]
fn dynamic_array_three_level_hierarchy() {
    let code = r#"
    module RegFile (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        var regs: logic<64> [32];

        always_comb {
            o_rs1_data = if i_wen && i_rs1_addr == i_wd_addr ? i_wd_data : regs[i_rs1_addr];
            o_rs2_data = if i_wen && i_rs2_addr == i_wd_addr ? i_wd_data : regs[i_rs2_addr];
        }

        always_ff {
            if_reset {
                for i: i32 in 0..32 { regs[i] = 64'd0; }
            } else if i_wen {
                regs[i_wd_addr] = i_wd_data;
            }
        }
    }

    module Harness (
        clk : input  clock   ,
        rst : input  reset   ,
        r1  : output logic<64>,
        r2  : output logic<64>,
    ) {
        var cycle: logic<8>;
        always_ff {
            if_reset { cycle = 8'd0; }
            else     { cycle = cycle + 8'd1; }
        }

        var rs1_addr: logic<5>;
        var rs2_addr: logic<5>;
        var wd_addr : logic<5>;
        var wd_data : logic<64>;
        var wen     : logic;
        always_comb {
            rs1_addr = 5'd0;
            rs2_addr = 5'd0;
            wd_addr  = 5'd0;
            wd_data  = 64'd0;
            wen      = 1'b0;
            case cycle {
                8'd1: { wd_addr = 5'd1; wd_data = 64'hAA; wen = 1'b1; }
                8'd3: { wd_addr = 5'd2; wd_data = 64'hBB; wen = 1'b1; }
                default: { rs1_addr = 5'd1; rs2_addr = 5'd2; }
            }
        }

        var rs1_data: logic<64>;
        var rs2_data: logic<64>;
        inst u_rf: RegFile (
            clk, rst,
            i_rs1_addr: rs1_addr, i_rs2_addr: rs2_addr,
            o_rs1_data: rs1_data, o_rs2_data: rs2_data,
            i_wd_addr: wd_addr, i_wd_data: wd_data, i_wen: wen,
        );

        assign r1 = rs1_data;
        assign r2 = rs2_data;
    }

    module Top (
        clk : input  clock   ,
        rst : input  reset   ,
        r1  : output logic<64>,
        r2  : output logic<64>,
    ) {
        inst u_harness: Harness (
            clk, rst, r1, r2,
        );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..20u32 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let r1 = sim.get("r1").unwrap().payload_u64();
        let r2 = sim.get("r2").unwrap().payload_u64();
        assert_eq!(
            r1, 0xAA,
            "JIT={} 4st={}: r1 expected 0xAA got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0xBB,
            "JIT={} 4st={}: r2 expected 0xBB got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
    }
}
