use parol_runtime::log::debug;
use parol_runtime::miette::{miette, IntoDiagnostic, Result, WrapErr};
use std::env;
use std::fs;
use std::time::Instant;
use veryl_parser::formatter::Formatter;
use veryl_parser::veryl_grammar::VerylGrammar;
use veryl_parser::veryl_parser::parse;

// To generate:
// parol -f ./veryl.par -e ./veryl-exp.par -p ./src/veryl_parser.rs -a ./src/veryl_grammar_trait.rs -t VerylGrammar -m veryl_grammar -g

fn main() -> Result<()> {
    env_logger::init();
    debug!("env logger started");

    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 {
        let file_name = args[1].clone();
        let input = fs::read_to_string(file_name.clone())
            .into_diagnostic()
            .wrap_err(format!("Can't read file {}", file_name))?;
        let mut veryl_grammar = VerylGrammar::new();
        let now = Instant::now();
        parse(&input, &file_name, &mut veryl_grammar)
            .wrap_err(format!("Failed parsing file {}", file_name))?;
        let elapsed_time = now.elapsed();
        println!("Parsing took {} milliseconds.", elapsed_time.as_millis());
        if args.len() > 2 && args[2] == "-q" {
            Ok(())
        } else {
            let mut formatter = Formatter::new();
            if let Some(ref veryl) = veryl_grammar.veryl {
                formatter.format(&veryl);
                println!("{}", formatter.as_str());
            }
            //println!("Success!\n{}", veryl_grammar);
            Ok(())
        }
    } else {
        Err(miette!("Please provide a file name as first parameter!"))
    }
}
