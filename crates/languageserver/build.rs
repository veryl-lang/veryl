use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Skip in GitHub Actions
    if let Ok(x) = env::var("GITHUB_ACTIONS")
        && x == "true"
    {
        return;
    }

    println!("cargo::rerun-if-changed=../parser/veryl.par");

    let par_file = PathBuf::from("../parser/veryl.par");
    let exp_file = PathBuf::from("src/keyword.rs");

    let text = fs::read_to_string(par_file).unwrap();
    let mut keywords = "pub const KEYWORDS: &[&str] = &[\n".to_string();
    for line in text.lines() {
        if line.contains("Keyword:") {
            let keyword = line.split("'").nth(1).unwrap();
            keywords.push_str(&format!("    \"{keyword}\",\n"));
        }
    }
    keywords.push_str("];\n");
    fs::write(exp_file, keywords).unwrap();
}
