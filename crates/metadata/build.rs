use merkle_hash::{Algorithm, Encodable, MerkleTree};
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let tree = MerkleTree::builder("./std/src")
        .algorithm(Algorithm::Blake3)
        .hash_names(true)
        .build()
        .unwrap();
    println!(
        "cargo:warning=std hash: {}",
        tree.root.item.hash.to_hex_string()
    );

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("std_hash.rs");
    fs::write(
        &dest_path,
        format!(
            "const STD_HASH: &str = \"{}\";",
            tree.root.item.hash.to_hex_string(),
        ),
    )
    .unwrap();
    println!("cargo::rerun-if-changed=./std/src");
}
