use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=tests/fixtures");

    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("translate_cases.rs");
    let mut out = fs::File::create(out_path).unwrap();

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<String> = fs::read_dir(&fixtures)
        .unwrap()
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension()? == "sv" {
                Some(path.file_stem()?.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();

    for name in names {
        let ident = name.replace('-', "_");
        writeln!(out, "#[test]").unwrap();
        writeln!(out, "fn {ident}() {{").unwrap();
        writeln!(out, "    check_fixture(\"{name}\");").unwrap();
        writeln!(out, "}}").unwrap();
    }
}
