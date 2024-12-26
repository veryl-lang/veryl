use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Skip in GitHub Actions
    if let Ok(x) = env::var("GITHUB_ACTIONS") {
        if x == "true" {
            return;
        }
    }

    println!("cargo::rerun-if-changed=../parser/veryl.par");

    let par_file = PathBuf::from("../parser/veryl.par");
    let exp_file = PathBuf::from("src/keyword.rs");

    let text = fs::read_to_string(par_file).unwrap();
    let mut keywords = "pub const KEYWORDS: &[&str] = &[\n".to_string();
    let mut in_keyword = false;
    for line in text.lines() {
        if line == "// -- keyword end --" {
            in_keyword = false;
        }
        if in_keyword {
            let keyword = line.split('/').nth(1).unwrap();
            keywords.push_str(&format!("    \"{keyword}\",\n"));
        }
        if line == "// -- keyword begin --" {
            in_keyword = true;
        }
    }
    keywords.push_str("];\n");
    fs::write(exp_file, keywords).unwrap();
}
