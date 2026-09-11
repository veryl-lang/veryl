use rust_embed::Embed;
use std::fs;
use std::path::{Path, PathBuf};
use veryl_path::{PathError, PathSet, ignore_already_exists};

include!(concat!(env!("OUT_DIR"), "/std_hash.rs"));

#[derive(Embed)]
#[folder = "./veryl/src"]
#[include = "*.veryl"]
struct Asset;

fn std_dir() -> PathBuf {
    veryl_path::cache_path().join("std").join(STD_HASH)
}

pub fn expand() -> Result<(), PathError> {
    expand_into(&std_dir())
}

fn expand_into(dir: &Path) -> Result<(), PathError> {
    // The directory is created before the first file, not after the last.
    let expanded = dir.join("expanded");

    if expanded.exists() {
        return Ok(());
    }

    ignore_already_exists(fs::create_dir_all(dir))?;

    let lock = veryl_path::lock_dir(dir)?;

    if !expanded.exists() {
        for file in Asset::iter() {
            let path = dir.join(file.as_ref());

            // The hashed directory name makes an existing file correct,
            // and a veryl too old for the marker reads it without the lock.
            if path.exists() {
                continue;
            }

            let content = Asset::get(file.as_ref()).unwrap();
            let parent = path.parent().unwrap();
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }

            fs::write(&path, content.data.as_ref())?;
        }

        fs::write(&expanded, [])?;
    }

    veryl_path::unlock_dir(lock)?;

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
            example: false,
        });
    }

    Ok(ret)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_asset() -> String {
        Asset::iter().next().unwrap().to_string()
    }

    #[test]
    fn an_existing_directory_without_the_marker_is_still_expanded() {
        let dir = tempfile::tempdir().unwrap();

        expand_into(dir.path()).unwrap();

        assert!(dir.path().join("expanded").exists());
        assert!(dir.path().join(first_asset()).exists());
    }

    #[test]
    fn a_tree_missing_a_file_is_completed() {
        let dir = tempfile::tempdir().unwrap();
        expand_into(dir.path()).unwrap();

        // The state a crashed expansion leaves behind.
        let dropped = dir.path().join(first_asset());
        fs::remove_file(&dropped).unwrap();
        fs::remove_file(dir.path().join("expanded")).unwrap();

        expand_into(dir.path()).unwrap();

        assert!(dropped.exists());
    }

    #[test]
    fn the_marker_short_circuits_the_expansion() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("expanded"), []).unwrap();

        expand_into(dir.path()).unwrap();

        assert!(!dir.path().join(first_asset()).exists());
    }

    #[test]
    fn a_file_already_in_place_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join(first_asset());
        fs::create_dir_all(kept.parent().unwrap()).unwrap();
        fs::write(&kept, b"kept").unwrap();

        expand_into(dir.path()).unwrap();

        assert_eq!(fs::read(&kept).unwrap(), b"kept");
    }
}
