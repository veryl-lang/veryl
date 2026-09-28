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

#[test]
fn unsupported_construct_has_span_reason_and_source() {
    use miette::{Diagnostic, SourceCode};

    let src = "module top;\n  initial begin\n    x = 0;\n  end\nendmodule\n";
    let out = veryl_translator::translate_str(
        src,
        "inline.sv",
        false,
        veryl_metadata::NewlineStyle::Auto,
    )
    .expect("translate");

    let c = out
        .unsupported
        .iter()
        .find(|c| c.kind == "initial block")
        .expect("initial block should be reported as unsupported");

    // The span must point at the `initial` keyword.
    let initial_off = src.find("initial").unwrap();
    assert_eq!(c.span.offset(), initial_off);
    assert_eq!(c.span.len(), "initial".len());

    // miette plumbing: a help line (the reason) and an attached source.
    assert!(!c.reason.is_empty(), "reason should be populated");
    assert!(c.help().is_some(), "reason should surface as the help line");
    let span_src = c
        .src
        .read_span(&c.span, 0, 0)
        .expect("span must resolve against the attached source");
    assert_eq!(
        std::str::from_utf8(span_src.data()).unwrap(),
        "initial",
        "the attached source span must cover the `initial` keyword"
    );
}

#[test]
fn unsupported_span_points_into_the_input_after_preprocessing() {
    // The preprocessor drops the `define line and pads the string literal, so
    // offsets into its output are behind or ahead of the input.
    let src = "`define W 8\nmodule top;\n  logic [`W-1:0] a;\n  logic b;\n  assign b = \"ab\" == \"ab\";\n  initial begin\n    a = 0;\n  end\nendmodule\n";
    let out = veryl_translator::translate_str(
        src,
        "inline.sv",
        false,
        veryl_metadata::NewlineStyle::Auto,
    )
    .expect("translate");

    let c = out
        .unsupported
        .iter()
        .find(|c| c.kind == "initial block")
        .expect("initial block should be reported as unsupported");
    assert_eq!(c.span.offset(), src.find("initial").unwrap());
    assert_eq!(c.span.len(), "initial".len());
    assert!(out.veryl.contains("var a: logic<8>;"), "{}", out.veryl);
}

include!(concat!(env!("OUT_DIR"), "/translate_cases.rs"));
