use parol::{build::Builder, ParolErrorReporter};
use parol_runtime::Report;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::Instant;

fn main() {
    // Skip in GitHub Actions
    if let Ok(x) = env::var("GITHUB_ACTIONS") {
        if x == "true" {
            return;
        }
    }

    let par_file = PathBuf::from("veryl.par");
    let exp_file = PathBuf::from("src/generated/veryl-exp.par");

    let par_modified = fs::metadata(par_file).unwrap().modified().unwrap();
    let exp_modified = fs::metadata(exp_file).unwrap().modified().unwrap();

    if par_modified > exp_modified {
        println!("cargo:warning=veryl.par was changed");

        let now = Instant::now();

        // CLI equivalent is:
        // parol -f ./veryl.par -e ./veryl-exp.par -p ./src/veryl_parser.rs -a ./src/veryl_grammar_trait.rs -t VerylGrammar -m veryl_grammar
        if let Err(err) = Builder::with_explicit_output_dir("src/generated")
            .grammar_file("veryl.par")
            .expanded_grammar_output_file("veryl-exp.par")
            .parser_output_file("veryl_parser.rs")
            .actions_output_file("veryl_grammar_trait.rs")
            .user_type_name("VerylGrammar")
            .user_trait_module_name("veryl_grammar")
            .trim_parse_tree()
            .generate_parser()
        {
            {
                ParolErrorReporter::report_error(&err, "veryl.par").unwrap_or_default();
                process::exit(1);
            }
        }

        let elapsed_time = now.elapsed();
        println!(
            "cargo:warning=parol build time: {} milliseconds",
            elapsed_time.as_millis()
        );
    }
}
