// The mock external binaries are POSIX shell scripts, so these CLI help tests run on Linux CI.
#![cfg(unix)]

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

#[cfg(not(target_os = "macos"))]
use std::os::unix::ffi::OsStringExt;

fn veryl() -> Command {
    Command::new(env!("CARGO_BIN_EXE_veryl"))
}

fn write_executable(dir: &Path, name: &str) {
    write_executable_with_body(dir, OsStr::new(name), "#!/bin/sh\nexit 0\n");
}

fn write_executable_with_body(dir: &Path, name: &OsStr, body: &str) {
    let path = dir.join(Path::new(name));
    std::fs::write(&path, body).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
}

fn run_with_path(work_dir: &Path, path_dirs: &[&Path], args: &[&str]) -> Output {
    let path = std::env::join_paths(path_dirs.iter().map(|path| path.to_path_buf())).unwrap();

    veryl()
        .current_dir(work_dir)
        .env("PATH", path)
        .args(args)
        .output()
        .unwrap()
}

fn run_success(work_dir: &Path, path_dirs: &[&Path], args: &[&str]) -> String {
    let output = run_with_path(work_dir, path_dirs, args);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let command = format!("veryl {}", args.join(" "));

    assert!(
        output.status.success(),
        "expected `{command}` to succeed; status: {:?}\nstderr:\n{stderr}\nstdout:\n{stdout}",
        output.status.code()
    );
    assert!(
        stderr.is_empty(),
        "expected `{command}` to keep stderr empty; stderr:\n{stderr}"
    );

    stdout.into_owned()
}

fn root_help(work_dir: &Path, path_dirs: &[&Path], flag: &str) -> String {
    run_success(work_dir, path_dirs, &[flag])
}

fn command_list(work_dir: &Path, path_dirs: &[&Path]) -> String {
    run_success(work_dir, path_dirs, &["--list"])
}

fn command_line_count(output: &str, name: &str) -> usize {
    output
        .lines()
        .filter(|line| line.split_whitespace().next() == Some(name))
        .count()
}

fn command_line<'a>(output: &'a str, name: &str) -> &'a str {
    output
        .lines()
        .find(|line| line.split_whitespace().next() == Some(name))
        .unwrap_or("")
}

fn assert_output_lists_commands(label: &str, output: &str, expected: &[&str]) {
    for name in expected {
        assert_eq!(
            command_line_count(output, name),
            1,
            "expected `{label}` to list command `{name}` exactly once; output:\n{output}"
        );
    }
}

fn assert_output_omits_commands(label: &str, output: &str, omitted: &[&str]) {
    for name in omitted {
        assert!(
            command_line_count(output, name) == 0,
            "expected `{label}` to omit command `{name}`; output:\n{output}"
        );
    }
}

fn assert_required_command_usage(output: &str) {
    assert!(
        output.contains("Usage: veryl [OPTIONS] <COMMAND>"),
        "expected usage to keep required subcommand marker `<COMMAND>`; output:\n{output}"
    );
    assert!(
        !output.contains("Usage: veryl [OPTIONS] [COMMAND]"),
        "expected usage not to show optional subcommand marker `[COMMAND]`; output:\n{output}"
    );
}

fn assert_list_hint(output: &str) {
    assert!(
        output.contains("--list") && output.contains("See all commands"),
        "expected root help to show the `--list` discovery hint; output:\n{output}"
    );
}

#[test]
fn root_help_omits_external_commands_and_points_to_list() {
    // Given: executable external Veryl subcommands are available on PATH.
    let temp = tempfile::tempdir().unwrap();
    write_executable(temp.path(), "veryl-import");
    write_executable(temp.path(), "veryl-flist");

    // When: root help is requested through both help flags and exact `help`.
    let short_help = root_help(temp.path(), &[temp.path()], "-h");
    let long_help = root_help(temp.path(), &[temp.path()], "--help");
    let help_command = run_success(temp.path(), &[temp.path()], &["help"]);

    // Then: root help stays cheap, keeps required-command usage, and advertises explicit discovery.
    for (label, output) in [
        ("veryl -h", short_help.as_str()),
        ("veryl --help", long_help.as_str()),
        ("veryl help", help_command.as_str()),
    ] {
        assert_output_omits_commands(label, output, &["flist", "import"]);
        assert_required_command_usage(output);
        assert_list_hint(output);
    }
}

#[test]
#[cfg(not(target_os = "macos"))]
fn root_help_ignores_non_utf8_external_suffixes() {
    // Given: PATH contains a valid external command and a veryl-prefixed executable with a non-UTF-8 suffix.
    let temp = tempfile::tempdir().unwrap();
    write_executable(temp.path(), "veryl-flist");
    let non_utf8_name = std::ffi::OsString::from_vec(vec![
        b'v', b'e', b'r', b'y', b'l', b'-', b'i', b'm', 0xff, b'o', b'r', b't',
    ]);
    write_executable_with_body(
        temp.path(),
        non_utf8_name.as_os_str(),
        "#!/bin/sh\nexit 0\n",
    );

    // When: root help is requested.
    let help = root_help(temp.path(), &[temp.path()], "-h");

    // Then: root help omits both valid and malformed external suffixes.
    assert_output_omits_commands("-h", &help, &["flist", "im\u{fffd}ort"]);
    assert_list_hint(&help);
}

#[test]
fn bare_cli_reports_missing_required_subcommand_usage() {
    // Given: PATH is controlled and contains no command candidates.
    let temp = tempfile::tempdir().unwrap();

    // When: the CLI is invoked without a subcommand.
    let output = veryl()
        .current_dir(temp.path())
        .env("PATH", temp.path())
        .output()
        .unwrap();

    // Then: it fails as a missing-subcommand error and keeps `<COMMAND>` in usage.
    assert!(
        !output.status.success(),
        "expected bare `veryl` to fail; status: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires a subcommand") || stderr.contains("subcommand"),
        "expected missing-subcommand style error; stderr:\n{stderr}"
    );
    assert_required_command_usage(&stderr);
}

#[test]
fn root_help_intercept_requires_exact_help_flag() {
    // Given: an executable external Veryl subcommand is available on PATH.
    let temp = tempfile::tempdir().unwrap();
    write_executable(temp.path(), "veryl-import");

    // When: `-h` is accompanied by another argument.
    let path = std::env::join_paths([temp.path()]).unwrap();
    let output = veryl()
        .current_dir(temp.path())
        .env("PATH", path)
        .args(["-h", "extra"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Then: normal clap handling answers, not the augmented exact-root-help path.
    assert!(
        output.status.success(),
        "expected clap help to succeed; status: {:?}",
        output.status.code()
    );
    assert_output_omits_commands("-h extra", &stdout, &["import"]);
    assert_required_command_usage(&stdout);
}

#[test]
fn list_reports_external_commands_from_path() {
    // Given: executable external Veryl subcommands are available on PATH.
    let temp = tempfile::tempdir().unwrap();
    write_executable(temp.path(), "veryl-import");
    write_executable(temp.path(), "veryl-flist");

    // When: the explicit command list is requested.
    let list = command_list(temp.path(), &[temp.path()]);

    // Then: external subcommands are listed only on the explicit discovery surface.
    assert_output_lists_commands("veryl --list", &list, &["flist", "import"]);
}

#[test]
fn list_uses_info_description_for_surviving_external_commands_only() {
    // Given: external commands provide valid and invalid `--info` responses.
    let temp = tempfile::tempdir().unwrap();
    write_executable_with_body(
        temp.path(),
        OsStr::new("veryl-import"),
        "#!/bin/sh\nif [ \"$1\" = \"--info\" ]; then printf 'Import source files\\n'; exit 0; fi\nexit 0\n",
    );
    write_executable_with_body(
        temp.path(),
        OsStr::new("veryl-flist"),
        "#!/bin/sh\nif [ \"$1\" = \"--info\" ]; then printf 'first line\\nsecond line\\n'; exit 0; fi\nexit 0\n",
    );
    write_executable_with_body(
        temp.path(),
        OsStr::new("veryl-build"),
        "#!/bin/sh\nif [ \"$1\" = \"--info\" ]; then printf 'External build should not be probed\\n'; exit 0; fi\nexit 0\n",
    );

    // When: the explicit command list is requested.
    let list = command_list(temp.path(), &[temp.path()]);

    // Then: final external entries use valid summaries, invalid summaries fall back, and built-in collisions are not probed.
    assert!(
        command_line(&list, "import").contains("Import source files"),
        "expected valid `--info` output to describe import; output:\n{list}"
    );
    assert!(
        command_line(&list, "flist").contains("External Veryl subcommand from PATH"),
        "expected invalid multiline `--info` output to fall back; output:\n{list}"
    );
    assert!(
        !list.contains("External build should not be probed"),
        "expected overwritten built-in collision not to run `--info`; output:\n{list}"
    );
}

#[test]
fn list_uses_builtin_rows_for_builtin_help_and_official_suffix_collisions() {
    // Given: PATH contains external-looking binaries that collide with built-ins, help, and `ls`.
    let temp = tempfile::tempdir().unwrap();
    write_executable_with_body(
        temp.path(),
        OsStr::new("veryl-build"),
        "#!/bin/sh\nprintf 'PATH veryl-build should not be listed\\n'\n",
    );
    write_executable_with_body(
        temp.path(),
        OsStr::new("veryl-check"),
        "#!/bin/sh\nprintf 'PATH veryl-check should not be listed\\n'\n",
    );
    write_executable_with_body(
        temp.path(),
        OsStr::new("veryl-help"),
        "#!/bin/sh\nprintf 'PATH veryl-help should not be listed\\n'\n",
    );
    write_executable_with_body(
        temp.path(),
        OsStr::new("veryl-ls"),
        "#!/bin/sh\nprintf 'PATH veryl-ls should not be listed\\n'\n",
    );
    write_executable(temp.path(), "veryl-import");

    // When: the explicit command list is requested.
    let list = command_list(temp.path(), &[temp.path()]);

    // Then: built-ins and help win through list merge order, and `ls` is excluded centrally.
    assert_output_lists_commands("veryl --list", &list, &["build", "check", "help", "import"]);
    assert_output_omits_commands("veryl --list", &list, &["ls"]);
    for name in ["build", "check", "help"] {
        let line = command_line(&list, name);
        assert!(
            !line.contains("External"),
            "expected `{name}` to be rendered as a built-in list row; line:\n{line}\nfull list:\n{list}"
        );
    }
    for forbidden in [
        "PATH veryl-build should not be listed",
        "PATH veryl-check should not be listed",
        "PATH veryl-help should not be listed",
        "PATH veryl-ls should not be listed",
    ] {
        assert!(
            !list.contains(forbidden),
            "expected list output not to contain external collision text `{forbidden}`; output:\n{list}"
        );
    }
}
