//! The one real-cargo integration test for the dlopen path: every logic
//! test drives components through the static registry; this covers the
//! library build + load + ABI handshake itself.

#![cfg(not(target_family = "wasm"))]

use std::path::PathBuf;
use std::process::Command;
use veryl_simulator::component::host::{ExternalInstance, HostContext, PortDir, PortRole};
use veryl_simulator::component::loader::lookup_component;

#[test]
fn dlopen_component_roundtrip() {
    // Skip on environments without a cargo toolchain.
    if Command::new("cargo").arg("--version").output().is_err() {
        eprintln!("skipping: cargo not available");
        return;
    }

    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test_component");
    let target_dir =
        std::env::temp_dir().join(format!("veryl_dlopen_fixture_{}", std::process::id()));
    let output = Command::new("cargo")
        .args(["build", "--release", "--message-format=json"])
        .current_dir(fixture)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("failed to run cargo");
    assert!(
        output.status.success(),
        "fixture build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut cdylib: Option<PathBuf> = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if msg["reason"] != "compiler-artifact" {
            continue;
        }
        let is_cdylib = msg["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|k| k == "cdylib"));
        if !is_cdylib {
            continue;
        }
        if let Some(files) = msg["filenames"].as_array() {
            for file in files {
                if let Some(path) = file.as_str()
                    && (path.ends_with(".so") || path.ends_with(".dylib") || path.ends_with(".dll"))
                {
                    cdylib = Some(PathBuf::from(path));
                }
            }
        }
    }
    let cdylib = cdylib.expect("fixture produced no cdylib");

    let vtable = lookup_component(Some(&cdylib), "fixture_counter").unwrap();

    // Unknown type in a real library is reported, not a crash, and the
    // error lists what the library actually exports.
    let err = lookup_component(Some(&cdylib), "no_such_type").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("the library exports"), "{msg}");
    assert!(msg.contains("`fixture_counter`"), "{msg}");

    let mut host = HostContext::new();
    host.add_port("out", PortDir::Output, 8);
    host.add_port_role("clk", PortDir::Input, PortRole::Clock, 1);
    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();
    for expected in 1..=3u64 {
        assert_eq!(instance.on_clock(&mut host), 0);
        assert_eq!(host.output_u64("out"), expected);
    }
    drop(instance);

    let _ = std::fs::remove_dir_all(&target_dir);
}
