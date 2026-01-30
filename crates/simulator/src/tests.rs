use crate::Simulator;
use crate::conv::{Config, build_ir};
use crate::ir as sir;
use crate::ir::{Event, Value};
use std::str::FromStr;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::VarId;
use veryl_analyzer::{Analyzer, AnalyzerError, Context, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn analyze(code: &str, config: &Config) -> sir::Ir {
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

    let ir = build_ir(ir, "Top".into(), config).unwrap();

    ir
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
fn long_bit() {
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

        println!("{}", sim.ir.dump_variables());

        sim.step(&rst);

        println!("{}", sim.ir.dump_variables());

        for _ in 0..100 {
            sim.step(&clk);
        }

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(100, 128, false);

        assert_eq!(sim.get("cnt").unwrap(), exp);
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
    binary_test("8'h43 ", "==?", "8'h4x ", 16, "16'b0000000000000001", false);
    binary_test("8'hz0 ", "==?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h11 ", "==?", "8'h1z ", 16, "16'b0000000000000001", false);
    binary_test("8'hxz ", "==?", "8'hxz ", 16, "16'b0000000000000001", true);
    binary_test("8'hzx ", "==?", "8'hxz ", 16, "16'b0000000000000001", true);

    binary_test("8'h00 ", "!=?", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "!=?", "8'he2 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx0 ", "!=?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h43 ", "!=?", "8'h4x ", 16, "16'b0000000000000000", false);
    binary_test("8'hz0 ", "!=?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h11 ", "!=?", "8'h1z ", 16, "16'b0000000000000000", false);
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
