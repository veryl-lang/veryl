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

    let par_file = PathBuf::from("../parser/veryl.par");
    let exp_file = PathBuf::from("src/keyword.rs");

    let par_modified = fs::metadata(&par_file).unwrap().modified().unwrap();
    let exp_modified = fs::metadata(&exp_file).unwrap().modified().unwrap();

    if par_modified > exp_modified {
        let text = fs::read_to_string(&par_file).unwrap();
        let mut keywords = "pub const KEYWORDS: &[&str] = &[\n".to_string();
        for line in text.lines() {
            if line.contains("(?-u:\\b)") {
                let keyword = line.split_ascii_whitespace().nth(2).unwrap();
                let keyword = keyword.replace("/(?-u:\\b)", "");
                let keyword = keyword.replace("(?-u:\\b)/", "");
                keywords.push_str(&format!("    \"{keyword}\",\n"));
            }
        }
        keywords.push_str("];\n");
        fs::write(&exp_file, keywords).unwrap();
    }
}
