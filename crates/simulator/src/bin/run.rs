use std::path::PathBuf;
use veryl_analyzer::ir::Ir;
use veryl_analyzer::{Analyzer, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_simulator::Simulator;

#[derive(clap::Parser)]
pub struct Opt {
    pub path: PathBuf,
    #[arg(long)]
    pub cycle: Option<usize>,
}

fn build_ir(code: &str) -> Ir {
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut ir = Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2("prj", &parser.veryl, &mut context, Some(&mut ir));

    ir
}

fn main() {
    //use clap::Parser;
    //let opt = Opt::parse();
    //let code = std::fs::read_to_string(&opt.path).unwrap();
    //let code = &code;
    //let cycle = opt.cycle;

    let code = r#"
    module Top #(
        param N: u32 = 10,
    )(
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>[N],
    ) {
        for i in 0..N: g {
            always_ff {
                if_reset {
                    cnt[i] = 0;
                } else {
                    cnt[i] += 1;
                }
            }
        }
    }
    "#;
    let cycle = Some(1000000);

    let ir = build_ir(code);

    let Some(cycle) = cycle else { return };

    let mut sim = Simulator::<std::io::Empty>::new("Top", ir, None).unwrap();
    let clk = sim.get_clock("clk").unwrap();
    let rst = sim.get_reset("rst").unwrap();

    sim.step(&rst);

    for _ in 0..cycle {
        sim.step(&clk);
    }

    println!("{}", sim.top.dump_variables());

    veryl_analyzer::stopwatch::dump();
}
