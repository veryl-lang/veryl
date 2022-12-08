use parol::build::Builder;

fn main() {
    // CLI equivalent is:
    // parol -f ./veryl.par -e ./veryl-exp.par -p ./src/veryl_parser.rs -a ./src/veryl_grammar_trait.rs -t VerylGrammar -m veryl_grammar -g
    Builder::with_explicit_output_dir("src")
        .grammar_file("veryl.par")
        .expanded_grammar_output_file("../veryl-exp.par")
        .parser_output_file("veryl_parser.rs")
        .actions_output_file("veryl_grammar_trait.rs")
        .enable_auto_generation()
        .user_type_name("VerylGrammar")
        .user_trait_module_name("veryl_grammar")
        .generate_parser()
        .unwrap();
}
