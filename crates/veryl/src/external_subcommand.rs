mod help;

pub use help::print_command_list;

use miette::{IntoDiagnostic, Result, bail};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) const OFFICIAL_NON_SUBCOMMAND_SUFFIXES: &[&str] = &["ls"];

pub(crate) fn is_official_non_subcommand_suffix(name: &str) -> bool {
    OFFICIAL_NON_SUBCOMMAND_SUFFIXES.contains(&name)
}

pub fn dispatch(args: Vec<OsString>) -> Result<ExitCode> {
    let Some((name, forwarded_args)) = args.split_first() else {
        bail!("external subcommand name is empty");
    };

    let binary_name = binary_name(name)?;
    let Some(binary_path) = find_on_path(&binary_name) else {
        bail!(
            "external subcommand `{}` was not found on PATH; install `{}` directly or make sure a `veryl-{}` executable is available on PATH",
            name.to_string_lossy(),
            binary_name.to_string_lossy(),
            name.to_string_lossy(),
        );
    };

    let status = Command::new(binary_path)
        .args(forwarded_args)
        .status()
        .into_diagnostic()?;
    if let Some(code) = status.code() {
        Ok(exit_code_from_i32(code))
    } else {
        eprintln!(
            "external subcommand `{}` terminated without an exit code",
            binary_name.to_string_lossy()
        );
        Ok(ExitCode::FAILURE)
    }
}

fn binary_name(name: &OsStr) -> Result<OsString> {
    let name = name.to_string_lossy();
    if name.is_empty() {
        bail!("external subcommand name is empty");
    }
    if name.contains('/') || name.contains('\\') || name.contains(std::path::MAIN_SEPARATOR) {
        bail!("external subcommand `{name}` must not contain path separators");
    }
    if is_official_non_subcommand_suffix(&name) {
        bail!("`veryl-{name}` is an official Veryl binary, not an external subcommand");
    }

    Ok(OsString::from(format!("veryl-{name}")))
}

fn find_on_path(binary_name: &OsStr) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(binary_name))
            .find(|path| is_executable_file(path))
    })
}

pub(super) fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn exit_code_from_i32(code: i32) -> ExitCode {
    match u8::try_from(code) {
        Ok(code) => ExitCode::from(code),
        Err(_) => ExitCode::FAILURE,
    }
}
