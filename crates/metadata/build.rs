use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo::rerun-if-env-changed=VERSION");

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("veryl_version.rs");
    fs::write(
        &dest_path,
        format!(
            "pub const VERYL_VERSION: &str = \"{}\";",
            env::var("VERSION").unwrap_or(env::var("CARGO_PKG_VERSION").unwrap())
        ),
    )
    .unwrap();
}
