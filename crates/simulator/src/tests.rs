use crate::Simulator;
use veryl_analyzer::ir::{Event, Ir, VarId};
use veryl_analyzer::value::Value;
use veryl_analyzer::{Analyzer, AnalyzerError, Context, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn analyze(code: &str) -> (Ir, Vec<AnalyzerError>) {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut errors = vec![];
    let mut ir = Ir::default();
    errors.append(&mut analyzer.analyze_pass1(&"prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2());
    dbg!(&errors);
    (ir, errors)
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

    // Call Veryl compiler
    let (ir, errors) = analyze(code);
    assert!(errors.is_empty());

    // Create new simulator instance specifing "Top" as top module
    let mut sim = Simulator::<std::io::Empty>::new("Top", ir, None).unwrap();

    println!("{}", sim.top.dump_variables());

    let a = Value::new(10u32.into(), 32, false);
    let b = Value::new(20u32.into(), 32, false);

    // Set values to input ports
    sim.set("a", a);
    sim.set("b", b);

    println!("{}", sim.top.dump_variables());

    // Execute 1 clock cycle simulation
    sim.step(&Event::Clock(VarId::default()));

    println!("{}", sim.top.dump_variables());

    let exp = Value::new(30u32.into(), 32, false);

    // Get values from output ports
    assert_eq!(sim.get("c").unwrap(), exp);
    //assert!(false);
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

    let (ir, errors) = analyze(code);
    assert!(errors.is_empty());

    let mut dump = Vec::new();

    let mut sim = Simulator::new("Top", ir, Some(&mut dump)).unwrap();

    let a = Value::new(10u32.into(), 32, false);
    let b = Value::new(20u32.into(), 32, false);

    sim.set("a", a);
    sim.set("b", b);

    sim.step(&Event::Clock(VarId::default()));

    let a = Value::new(30u32.into(), 32, false);
    let b = Value::new(10u32.into(), 32, false);

    sim.set("a", a);
    sim.set("b", b);

    sim.step(&Event::Clock(VarId::default()));

    let a = Value::new(50u32.into(), 32, false);
    let b = Value::new(20u32.into(), 32, false);

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

    let (ir, errors) = analyze(code);
    assert!(errors.is_empty());

    let mut sim = Simulator::<std::io::Empty>::new("Top", ir, None).unwrap();

    let clk = sim.get_clock("clk").unwrap();
    let rst = sim.get_reset("rst").unwrap();

    println!("{}", sim.top.dump_variables());

    sim.step(&rst);

    println!("{}", sim.top.dump_variables());

    for _ in 0..100 {
        sim.step(&clk);
    }

    println!("{}", sim.top.dump_variables());

    let exp = Value::new(100u32.into(), 32, false);

    assert_eq!(sim.get("cnt").unwrap(), exp);
}
