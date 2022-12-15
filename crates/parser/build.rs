use parol::build::Builder;
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use walkdir::WalkDir;

fn main() {
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

    println!("cargo:rerun-if-changed=../../testcases");

    let out_dir = env::var("OUT_DIR").unwrap();
    let out_test = Path::new(&out_dir).join("test.rs");
    let mut out_test = File::create(&out_test).unwrap();

    for entry in WalkDir::new("../../testcases") {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            let file = entry.path().file_stem().unwrap().to_string_lossy();
            let _ = write!(out_test, "#[test]\n");
            let _ = write!(out_test, "fn test_{}() {{\n", file);
            let _ = write!(out_test, "    test(\"{}\");\n", file);
            let _ = write!(out_test, "}}\n");
        }
    }
}
