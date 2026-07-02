use super::*;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
fn write_file(dir: &Path, name: &OsStr, mode: u32) {
    let path = dir.join(Path::new(name));
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn write_executable(dir: &Path, name: &OsStr) {
    write_file(dir, name, 0o755);
}

#[cfg(unix)]
fn write_non_executable(dir: &Path, name: &OsStr) {
    write_file(dir, name, 0o644);
}

#[test]
#[cfg(unix)]
fn discover_subcommands_keeps_valid_utf8_executable_suffixes_sorted() {
    // Given: duplicate valid external subcommands exist across PATH entries.
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_executable(first.path(), OsStr::new("veryl-import"));
    write_executable(second.path(), OsStr::new("veryl-import"));
    write_executable(second.path(), OsStr::new("veryl-flist"));

    // When: discovery scans those PATH entries.
    let discovered = discover_from_path_entries([first.path(), second.path()]);

    // Then: names are deduped and sorted by Rust String ordering.
    assert_eq!(names(&discovered), ["flist", "import"]);
}

#[test]
#[cfg(unix)]
fn discover_subcommands_ignores_official_non_subcommand_suffixes() {
    // Given: PATH contains an official Veryl binary that uses the `veryl-` prefix.
    let dir = tempfile::tempdir().unwrap();
    write_executable(dir.path(), OsStr::new("veryl-ls"));
    write_executable(dir.path(), OsStr::new("veryl-import"));

    // When: discovery scans that PATH entry.
    let discovered = discover_from_path_entries([dir.path()]);

    // Then: only real external subcommands remain.
    assert_eq!(names(&discovered), ["import"]);
}

#[test]
#[cfg(unix)]
fn discover_subcommands_ignores_non_executable_non_directory_and_malformed_entries() {
    // Given: PATH includes malformed candidates, a plain file entry, and one valid command.
    let dir = tempfile::tempdir().unwrap();
    let plain_file = tempfile::NamedTempFile::new().unwrap();
    write_executable(dir.path(), OsStr::new("veryl-flist"));
    write_executable(dir.path(), OsStr::new("veryl-"));
    write_executable(dir.path(), OsStr::new("veryl-bad\\name"));
    #[cfg(not(target_os = "macos"))]
    write_executable(dir.path(), OsStr::from_bytes(b"veryl-im\xffort"));
    write_non_executable(dir.path(), OsStr::new("veryl-import"));

    // When: discovery scans all entries.
    let discovered = discover_from_path_entries([dir.path(), plain_file.path()]);

    // Then: only valid executable suffixes survive.
    assert_eq!(names(&discovered), ["flist"]);
}

#[test]
#[cfg(unix)]
fn discover_subcommands_ignores_unreadable_path_entries() {
    // Given: PATH includes an unreadable directory before a readable external command directory.
    let unreadable = tempfile::tempdir().unwrap();
    let readable = tempfile::tempdir().unwrap();
    let mut permissions = fs::metadata(unreadable.path()).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(unreadable.path(), permissions).unwrap();
    write_executable(readable.path(), OsStr::new("veryl-import"));

    // When: discovery scans all entries.
    let discovered = discover_from_path_entries([unreadable.path(), readable.path()]);

    let mut permissions = fs::metadata(unreadable.path()).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(unreadable.path(), permissions).unwrap();

    // Then: unreadable PATH entries do not fail discovery.
    assert_eq!(names(&discovered), ["import"]);
}

#[test]
fn parse_info_description_accepts_short_single_line_utf8() {
    // Given: an external command prints one concise UTF-8 line.
    let stdout = "Import Veryl modules\n";

    // When: the summary is parsed.
    let description = parse_info_description(stdout);

    // Then: the trimmed line is accepted.
    assert_eq!(description.as_deref(), Some("Import Veryl modules"));
}

#[test]
fn parse_info_description_rejects_invalid_output() {
    let long = "x".repeat(MAX_INFO_DESCRIPTION_CHARS + 1);
    for stdout in ["", "\n", "first\nsecond\n", "bad\tcontrol\n", &long] {
        // Given: invalid stdout for the one-line `--info` protocol.
        // When: the summary is parsed.
        let description = parse_info_description(stdout);

        // Then: the caller will fall back to the generic external description.
        assert!(description.is_none(), "accepted invalid stdout: {stdout:?}");
    }
}

#[test]
#[cfg(unix)]
fn probe_info_description_rejects_hanging_commands() {
    // Given: an external command hangs while answering `--info`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("veryl-slow");
    fs::write(
        &path,
        "#!/bin/sh\nif [ \"$1\" = \"--info\" ]; then sleep 2; fi\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();

    // When: the list-summary probe runs.
    let description = probe_info_description(&path);

    // Then: the probe times out and the caller will use fallback text.
    assert!(description.is_none());
}

#[test]
#[cfg(unix)]
fn probe_info_description_rejects_oversized_output_without_waiting_for_process_exit() {
    // Given: an external command emits one byte past the accepted summary limit and keeps stdout open.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("veryl-verbose");
    let oversized_bytes = MAX_INFO_DESCRIPTION_BYTES + 1;
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--info\" ]; then printf '%*s' {oversized_bytes} '' | tr ' ' x; sleep 2; fi\n",
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();

    // When: the list-summary probe runs.
    let started = Instant::now();
    let description = probe_info_description(&path);
    let elapsed = started.elapsed();

    // Then: oversized output falls back immediately after the bounded read, not after the deadline.
    assert!(description.is_none());
    assert!(
        elapsed < INFO_TIMEOUT / 2,
        "expected oversized output to fall back before waiting for process exit; elapsed: {elapsed:?}"
    );
}

#[test]
#[cfg(unix)]
fn probe_info_description_times_out_when_descendant_inherits_stdout() {
    // Given: the direct `--info` process exits but a descendant keeps stdout open.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("veryl-zhold");
    let pid_file = dir.path().join("zhold.pid");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--info\" ]; then bash -c 'exec -a veryl-zhold-inherited-stdout-regression sleep 60' & printf '%s\\n' \"$!\" > {}; printf 'held stdout\\n'; exit 0; fi\n",
            pid_file.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();

    // When: the list-summary probe runs.
    let description = probe_info_description(&path);

    let pid = fs::read_to_string(&pid_file).unwrap();
    let descendant_alive = process_is_alive(pid.trim());
    if descendant_alive {
        let _ = Command::new("kill").arg("-TERM").arg(pid.trim()).status();
    }

    // Then: inherited stdout does not block past the probe deadline and the process tree is cleaned.
    assert!(description.is_none());
    assert!(
        !descendant_alive,
        "expected inherited-stdout descendant pid {} to be dead before test cleanup",
        pid.trim()
    );
}

fn names(discovered: &BTreeMap<String, CommandInfo>) -> Vec<&str> {
    discovered.keys().map(String::as_str).collect()
}

#[cfg(unix)]
fn process_is_alive(pid: &str) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
