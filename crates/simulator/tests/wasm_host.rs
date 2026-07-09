//! Functional tests of the wasm transport: the fixture is built for
//! wasm32-unknown-unknown with the real cargo and driven through
//! `ExternalInstance`, mirroring what the dlopen test covers natively —
//! ports, params (including the retry protocol of `param_get`), methods
//! with arguments and returns, file I/O, traces, and trap conversion.

#![cfg(not(target_family = "wasm"))]

use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;
use veryl_simulator::component::host::{
    ExternalInstance, HostContext, HostValue, PortDir, PortRole,
};
use veryl_simulator::component::loader::{
    ComponentBackend, ComponentError, lookup_component_backend,
};

/// Builds the fixture once for every test in this file; `None` when the
/// toolchain or the wasm32 std is unavailable.
static FIXTURE_WASM: LazyLock<Option<PathBuf>> = LazyLock::new(build_fixture_wasm);

fn build_fixture_wasm() -> Option<PathBuf> {
    if Command::new("cargo").arg("--version").output().is_err() {
        eprintln!("skipping: cargo not available");
        return None;
    }
    let libdir = Command::new("rustc")
        .args([
            "--print",
            "target-libdir",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output();
    let has_wasm_std = libdir.is_ok_and(|out| {
        out.status.success() && PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()).exists()
    });
    if !has_wasm_std {
        eprintln!("skipping: wasm32-unknown-unknown std not installed");
        return None;
    }

    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test_component");
    // Shared fixture build cache; see wasm_guest.rs.
    let target_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm-fixture");
    let output = Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
        .current_dir(fixture)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("failed to run cargo");
    assert!(
        output.status.success(),
        "fixture wasm build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(
        target_dir
            .join("wasm32-unknown-unknown")
            .join("release")
            .join("test_component.wasm"),
    )
}

fn backend(type_name: &str) -> Option<ComponentBackend> {
    let path = FIXTURE_WASM.as_ref()?;
    Some(lookup_component_backend(Some(path), type_name).unwrap())
}

macro_rules! require_fixture {
    ($name:literal) => {
        match backend($name) {
            Some(backend) => backend,
            None => return,
        }
    };
}

#[test]
fn clocked_roundtrip_and_finish() {
    let backend = require_fixture!("fixture_counter");
    assert_eq!(backend.kind(), veryl_component_sys::VRL_KIND_CLOCKED);

    let mut host = HostContext::new();
    host.add_port("out", PortDir::Output, 64);
    host.add_port_role("clk", PortDir::Input, PortRole::Clock, 1);
    host.add_param("limit", HostValue::bits_u64(3, 32));
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();
    assert_eq!(instance.kind(), veryl_component_sys::VRL_KIND_CLOCKED);

    for expected in 1..=3u64 {
        assert_eq!(instance.on_clock(&mut host), 0);
        assert_eq!(host.output_u64("out"), expected);
    }
    assert!(host.finish_requested());
    assert!(!host.failed());
    assert_eq!(instance.on_finish(&mut host), 0);
}

#[test]
fn optional_param_absent() {
    // No `limit` parameter: `param_get` answers -1 and the component runs
    // without finishing.
    let backend = require_fixture!("fixture_counter");
    let mut host = HostContext::new();
    host.add_port("out", PortDir::Output, 64);
    host.add_port_role("clk", PortDir::Input, PortRole::Clock, 1);
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();
    for _ in 0..5 {
        assert_eq!(instance.on_clock(&mut host), 0);
    }
    assert!(!host.finish_requested());
}

#[test]
fn wide_ports_cross_the_boundary() {
    let backend = require_fixture!("fixture_mirror");
    let mut host = HostContext::new();
    host.add_port_role("clk", PortDir::Input, PortRole::Clock, 1);
    let d_idx = host.add_port("d", PortDir::Input, 128);
    host.add_port("q", PortDir::Output, 128);
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();

    host.set_input(d_idx, &[0xdead_beef_cafe_f00d, 0x0123_4567_89ab_cdef]);
    assert_eq!(instance.on_clock(&mut host), 0);
    assert_eq!(
        host.output_words("q"),
        &[0xdead_beef_cafe_f00d, 0x0123_4567_89ab_cdef]
    );
}

#[test]
fn methods_params_and_logs() {
    let backend = require_fixture!("fixture_echo");
    let mut host = HostContext::new();
    host.add_param("scale", HostValue::bits_u64(3, 32));
    // Long enough to force the param_get retry with a grown buffer.
    let tag = "a".repeat(100);
    host.add_param("tag", HostValue::Str(tag));
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();

    assert_eq!(
        instance.call_method(&mut host, "set", &[HostValue::bits_u64(14, 64)]),
        Some(HostValue::Unit)
    );
    assert_eq!(
        instance.call_method(&mut host, "get", &[]),
        Some(HostValue::Bits {
            words: vec![42],
            width: 64
        })
    );
    assert_eq!(host.logs(), ["get -> 42"]);

    // String argument marshalling.
    let ret = instance
        .call_method(&mut host, "name_len", &[HostValue::Str("veryl".into())])
        .unwrap();
    assert_eq!(ret.as_u64(), Some(5));

    // String parameter resolved during create.
    let ret = instance.call_method(&mut host, "tag_len", &[]).unwrap();
    assert_eq!(ret.as_u64(), Some(100));

    // Unknown method: reported through fail, rc != 0.
    assert_eq!(instance.call_method(&mut host, "nope", &[]), None);
    assert!(host.failures().iter().any(|m| m.contains("unknown method")));
}

#[test]
fn panic_becomes_trap_with_message() {
    let backend = require_fixture!("fixture_panicker");
    let mut host = HostContext::new();
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();

    assert_eq!(instance.call_method(&mut host, "boom", &[]), None);
    let failures = host.take_failures();
    assert!(
        failures.iter().any(|m| m.contains("kaboom")),
        "panic message lost: {failures:?}"
    );
    assert!(
        failures.iter().any(|m| m.contains("component trapped")),
        "trap not reported: {failures:?}"
    );

    // A plain Err still crosses as fail + rc, not a trap.
    let mut host = HostContext::new();
    let backend = require_fixture!("fixture_panicker");
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();
    assert_eq!(instance.call_method(&mut host, "err", &[]), None);
    let failures = host.take_failures();
    assert!(failures.iter().any(|m| m.contains("deliberate error")));
    assert!(!failures.iter().any(|m| m.contains("trapped")));
}

#[test]
fn file_io_roundtrip() {
    let backend = require_fixture!("fixture_filer");
    let dir = std::env::temp_dir().join(format!("veryl_wasm_files_{}", std::process::id()));
    let mut host = HostContext::new();
    host.write_base = Some(dir.clone());
    host.read_base = Some(dir.clone());
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();

    let args = [
        HostValue::Str("note.txt".into()),
        HostValue::Str("hello wasm".into()),
    ];
    assert_eq!(
        instance.call_method(&mut host, "write_note", &args),
        Some(HostValue::Unit)
    );
    let ret = instance
        .call_method(&mut host, "note_len", &[HostValue::Str("note.txt".into())])
        .unwrap();
    assert_eq!(ret.as_u64(), Some("hello wasm".len() as u64));
    assert!(!host.failed());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trace_vars_reach_the_host() {
    let backend = require_fixture!("fixture_tracer");
    let mut host = HostContext::new();
    host.add_port_role("clk", PortDir::Input, PortRole::Clock, 1);
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();

    assert_eq!(host.trace_vars.len(), 1);
    assert_eq!(host.trace_vars[0].name, "cycle");
    assert_eq!(host.trace_vars[0].width, 32);

    host.cycle = 7;
    assert_eq!(instance.on_clock(&mut host), 0);
    assert_eq!(host.trace_vars[0].words, [7]);
}

#[test]
fn file_io_denied_without_requires() {
    // The type's manifest declares no `requires(file)`: the file service
    // is stubbed to -1 with an explicit failure.
    let backend = require_fixture!("fixture_sneaky_filer");
    let mut host = HostContext::new();
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();

    assert_eq!(
        instance.call_method(&mut host, "write_note", &[HostValue::Str("x.txt".into())]),
        None
    );
    assert!(
        host.failures()
            .iter()
            .any(|m| m.contains("does not declare `requires(file)`")),
        "missing enforcement message: {:?}",
        host.failures()
    );
    assert!(host.touched_files.is_empty());
}

#[test]
fn native_component_is_rejected_at_load() {
    let Some(path) = FIXTURE_WASM.as_ref() else {
        return;
    };
    let err = lookup_component_backend(Some(path), "fixture_native_decl").unwrap_err();
    assert!(
        matches!(err, ComponentError::WasmNativeComponent { .. }),
        "{err}"
    );
    assert!(err.to_string().contains("requires(native)"), "{err}");
}

#[test]
fn memory_limit_stops_runaway_allocation() {
    let backend = require_fixture!("fixture_hog");
    let mut host = HostContext::new();
    let mut instance = ExternalInstance::create(backend, &mut host).unwrap();

    // Under the limit: succeeds.
    let ret = instance
        .call_method(&mut host, "hog", &[HostValue::bits_u64(1 << 20, 64)])
        .unwrap();
    assert_eq!(ret.as_u64(), Some(1 << 20));

    // Past the 256 MiB store limit: the grow is denied and the guest
    // aborts, surfacing as a trap failure instead of exhausting the host.
    assert_eq!(
        instance.call_method(&mut host, "hog", &[HostValue::bits_u64(512 << 20, 64)]),
        None
    );
    assert!(
        host.failures().iter().any(|m| m.contains("trapped")),
        "expected a trap failure: {:?}",
        host.failures()
    );
}

#[test]
fn load_errors_are_reported() {
    let Some(path) = FIXTURE_WASM.as_ref() else {
        return;
    };
    let err = lookup_component_backend(Some(path), "no_such_type").unwrap_err();
    assert!(matches!(err, ComponentError::UnknownType { .. }), "{err}");
    let msg = err.to_string();
    assert!(msg.contains("the library exports"), "{msg}");
    assert!(msg.contains("`fixture_mirror`"), "{msg}");

    let bogus = path.with_file_name("missing.wasm");
    let err = lookup_component_backend(Some(&bogus), "fixture_counter").unwrap_err();
    assert!(matches!(err, ComponentError::LibraryLoad { .. }), "{err}");
}

#[test]
fn create_failure_reports_missing_port() {
    // FixtureMirror requires ports clk/d/q; give it nothing.
    let backend = require_fixture!("fixture_mirror");
    let mut host = HostContext::new();
    let err = ExternalInstance::create(backend, &mut host).unwrap_err();
    assert!(matches!(err, ComponentError::CreateFailed { .. }), "{err}");
    assert!(
        err.to_string().contains("no clock port named `clk`"),
        "{err}"
    );
}
