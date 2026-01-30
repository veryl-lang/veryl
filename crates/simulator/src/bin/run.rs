use std::path::PathBuf;
use veryl_analyzer::ir as air;
use veryl_analyzer::{Analyzer, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_simulator::ir::{self, Ir};
use veryl_simulator::ir::Event;
use veryl_simulator::{Config, Simulator};

#[derive(clap::Parser)]
pub struct Opt {
    pub path: Option<PathBuf>,
    #[arg(long)]
    pub cycle: Option<usize>,
    #[arg(long)]
    pub use_4state: bool,
    #[arg(long)]
    pub use_jit: bool,
    #[arg(long)]
    pub dump_cranelift: bool,
    #[arg(long)]
    pub dump_asm: bool,
}

impl From<Opt> for Config {
    fn from(value: Opt) -> Self {
        Self {
            use_jit: value.use_jit,
            use_4state: value.use_4state,
            dump_cranelift: value.dump_cranelift,
            dump_asm: value.dump_asm,
        }
    }
}

fn build(code: &str, top: &str, config: &Config) -> Ir {
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut ir = air::Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2("prj", &parser.veryl, &mut context, Some(&mut ir));

    ir::build_ir(ir, top.into(), config).unwrap()
}

fn main() {
    use clap::Parser;
    let opt = Opt::parse();

    let code = if let Some(path) = &opt.path {
        std::fs::read_to_string(path).unwrap()
    } else {
        r#"
        module Top #(
            param N: u32 = 1000,
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
        "#
        .to_string()
    };
    let code = &code;
    let cycle = opt.cycle.unwrap_or(1000000);

    let config: Config = opt.into();

    let ir = build(code, "Top", &config);

    let mut sim = Simulator::<std::io::Empty>::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    let rst = sim.get_reset("rst").unwrap();

    sim.step(&Event::Initial);
    sim.step(&rst);

    for _ in 0..cycle {
        sim.step(&clk);
    }

    println!("{}", sim.ir.dump_variables());

    let (jit, total) = sim.ir.jit_stats();
    if total > 0 {
        let pct = jit as f64 / total as f64 * 100.0;
        eprintln!("JIT: {jit}/{total} statements ({pct:.1}%)");
    }

    veryl_analyzer::stopwatch::dump();
}
