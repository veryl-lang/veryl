//! Snapshot tests: each `*.sv` fixture in `tests/fixtures/` is translated and
//! compared against its sibling `*.veryl` expected output. Test functions are
//! generated at build time by `build.rs`.

use std::fs;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn check_fixture(name: &str) {
    let dir = fixtures_dir();
    let sv_path = dir.join(format!("{name}.sv"));
    let expected_path = dir.join(format!("{name}.veryl"));

    let src = fs::read_to_string(&sv_path).expect("read input");
    let expected = fs::read_to_string(&expected_path).expect("read expected");

    let out =
        veryl_translator::translate_str(&src, &sv_path, false, veryl_metadata::NewlineStyle::Auto)
            .expect("translate");
    assert!(
        out.unsupported.is_empty(),
        "{name}: unexpected unsupported reports: {:?}",
        out.unsupported
    );
    assert_eq!(out.veryl, expected, "{name}: output mismatch");
}

include!(concat!(env!("OUT_DIR"), "/translate_cases.rs"));
