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

    // gix is not available on wasm; combine the feature check with a target check.
    println!("cargo:rustc-check-cfg=cfg(gitoxide_enabled)");
    let feature_on = env::var_os("CARGO_FEATURE_GIT_GITOXIDE").is_some();
    let target_family = env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let is_wasm = target_family.split(',').any(|s| s == "wasm");
    if feature_on && !is_wasm {
        println!("cargo:rustc-cfg=gitoxide_enabled");
    }
}
