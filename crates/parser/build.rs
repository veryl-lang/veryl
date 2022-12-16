use parol::build::Builder;
use std::time::Instant;

fn main() {
    let now = Instant::now();

    // CLI equivalent is:
    // parol -f ./veryl.par -e ./veryl-exp.par -p ./src/veryl_parser.rs -a ./src/veryl_grammar_trait.rs -t VerylGrammar -m veryl_grammar -g
    Builder::with_explicit_output_dir("src/generated")
        .grammar_file("veryl.par")
        .expanded_grammar_output_file("veryl-exp.par")
        .parser_output_file("veryl_parser.rs")
        .actions_output_file("veryl_grammar_trait.rs")
        .enable_auto_generation()
        .user_type_name("VerylGrammar")
        .user_trait_module_name("veryl_grammar")
        .generate_parser()
        .unwrap();

    let elapsed_time = now.elapsed();
    println!(
        "cargo:warning=parol build time: {} milliseconds",
        elapsed_time.as_millis()
    );
}
