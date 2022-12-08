extern crate parol_runtime;

// auto generation needs derive_builder
mod derive_builder {
    pub use parol_runtime::derive_builder::*;
}

mod veryl_grammar;
// The output is version controlled
mod veryl_grammar_trait;
mod veryl_parser;

use crate::veryl_grammar::VerylGrammar;
use crate::veryl_parser::parse;
use parol_runtime::log::debug;
use parol_runtime::miette::{miette, IntoDiagnostic, Result, WrapErr};
use std::env;
use std::fs;
use std::time::Instant;

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
            println!("Success!\n{}", veryl_grammar);
            Ok(())
        }
    } else {
        Err(miette!("Please provide a file name as first parameter!"))
    }
}


