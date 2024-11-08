use rust_embed::Embed;
use std::fs;
use std::path::{Path, PathBuf};
use veryl_path::{PathError, PathSet};

include!(concat!(env!("OUT_DIR"), "/std_hash.rs"));

#[derive(Embed)]
#[folder = "./veryl/src"]
#[include = "*.veryl"]
struct Asset;

fn std_dir() -> PathBuf {
    veryl_path::cache_path().join("std").join(STD_HASH)
}

pub fn expand() -> Result<(), PathError> {
    let std_dir = std_dir();

    if !std_dir.exists() {
        fs::create_dir_all(&std_dir)?;

        let lock = veryl_path::lock_dir(&std_dir)?;

        for file in Asset::iter() {
            let content = Asset::get(file.as_ref()).unwrap();
            let path = std_dir.join(file.as_ref());

            let parent = path.parent().unwrap();
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }

            fs::write(&path, content.data.as_ref())?;
        }

        veryl_path::unlock_dir(lock)?;
    }

    Ok(())
}

pub fn paths(base_dst: &Path) -> Result<Vec<PathSet>, PathError> {
    let mut ret = Vec::new();
    let std_dir = std_dir().canonicalize().unwrap();

    for src in &veryl_path::gather_files_with_extension(&std_dir, "veryl", false)? {
        let rel = src.strip_prefix(&std_dir)?;
        let mut dst = base_dst.join("std");
        dst.push(rel);
        dst.set_extension("sv");
        let mut map = dst.to_path_buf();
        map.set_extension("sv.map");
        ret.push(PathSet {
            prj: "$std".to_string(),
            src: src.to_path_buf(),
            dst,
            map,
        });
    }

    Ok(ret)
}
