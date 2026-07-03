use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use clap::Command as ClapCommand;
use miette::{IntoDiagnostic, Result};

use super::{is_executable_file, is_official_non_subcommand_suffix};

const MAX_INFO_DESCRIPTION_CHARS: usize = 160;
const MAX_INFO_DESCRIPTION_BYTES: usize = MAX_INFO_DESCRIPTION_CHARS * 4 + 2;
const INFO_TIMEOUT: Duration = Duration::from_millis(500);
const INFO_KILL_GRACE: Duration = Duration::from_millis(200);
const EXTERNAL_FALLBACK_DESCRIPTION: &str = "External Veryl subcommand from PATH";
const HELP_DESCRIPTION: &str = "Print this message or the help of the given subcommand(s)";

enum CommandInfo {
    BuiltIn { description: String },
    External { binary_name: String, path: PathBuf },
}

pub fn print_command_list(command: ClapCommand) -> Result<()> {
    let list = render_command_list(command);
    std::io::stdout()
        .write_all(list.as_bytes())
        .into_diagnostic()?;
    Ok(())
}

fn render_command_list(command: ClapCommand) -> String {
    let mut commands = discover_external_commands();
    insert_builtin_commands(&mut commands, command);
    commands.insert(
        "help".to_owned(),
        CommandInfo::BuiltIn {
            description: HELP_DESCRIPTION.to_owned(),
        },
    );

    let mut output = String::from("Available Commands:\n");
    for (name, info) in commands {
        let description = match info {
            CommandInfo::BuiltIn { description } => description,
            CommandInfo::External { binary_name, path } => probe_info_description(&path)
                .unwrap_or_else(|| format!("{EXTERNAL_FALLBACK_DESCRIPTION} ({binary_name})")),
        };
        output.push_str(&format!("  {name:<12} {description}\n"));
    }
    output
}

fn insert_builtin_commands(commands: &mut BTreeMap<String, CommandInfo>, command: ClapCommand) {
    for subcommand in command.get_subcommands() {
        if subcommand.is_hide_set() {
            continue;
        }

        commands.insert(
            subcommand.get_name().to_owned(),
            CommandInfo::BuiltIn {
                description: subcommand
                    .get_about()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            },
        );
    }
}

fn discover_external_commands() -> BTreeMap<String, CommandInfo> {
    let Some(path) = std::env::var_os("PATH") else {
        return BTreeMap::new();
    };

    discover_from_path_entries(std::env::split_paths(&path))
}

fn discover_from_path_entries<P>(
    path_entries: impl IntoIterator<Item = P>,
) -> BTreeMap<String, CommandInfo>
where
    P: AsRef<Path>,
{
    let mut discovered = BTreeMap::new();
    for dir in path_entries {
        let Ok(entries) = fs::read_dir(dir.as_ref()) else {
            continue;
        };

        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if !is_executable_file(&path) {
                continue;
            }

            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(suffix) = file_name.strip_prefix("veryl-") else {
                continue;
            };
            if valid_external_suffix(suffix) {
                discovered
                    .entry(suffix.to_owned())
                    .or_insert_with(|| CommandInfo::External {
                        binary_name: file_name.to_owned(),
                        path: path.clone(),
                    });
            }
        }
    }

    discovered
}

fn valid_external_suffix(suffix: &str) -> bool {
    !suffix.is_empty()
        && !suffix.contains('/')
        && !suffix.contains('\\')
        && !suffix.contains(std::path::MAIN_SEPARATOR)
        && !is_official_non_subcommand_suffix(suffix)
}

fn probe_info_description(path: &Path) -> Option<String> {
    let mut command = ProcessCommand::new(path);
    command
        .arg("--info")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_info_probe_command(&mut command);

    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let limit = u64::try_from(MAX_INFO_DESCRIPTION_BYTES + 1).unwrap_or(u64::MAX);
        let stdout = match stdout.take(limit).read_to_end(&mut bytes) {
            Ok(_) if bytes.len() <= MAX_INFO_DESCRIPTION_BYTES => String::from_utf8(bytes).ok(),
            Ok(_) | Err(_) => None,
        };
        let _ = stdout_tx.send(stdout);
    });

    let started = Instant::now();
    let mut child_exited_successfully = false;
    let mut stdout = None;
    loop {
        if stdout.is_none() {
            match stdout_rx.try_recv() {
                Ok(Some(output)) => stdout = Some(output),
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                    terminate_info_probe(&mut child);
                    return None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        if child_exited_successfully && let Some(stdout) = stdout {
            return parse_info_description(&stdout);
        }

        if !child_exited_successfully {
            let Some(status) = child.try_wait().ok()? else {
                if started.elapsed() >= INFO_TIMEOUT {
                    terminate_info_probe(&mut child);
                    return None;
                }

                std::thread::sleep(Duration::from_millis(10));
                continue;
            };

            if !status.success() {
                terminate_info_probe(&mut child);
                return None;
            }

            child_exited_successfully = true;
        }

        if started.elapsed() >= INFO_TIMEOUT {
            terminate_info_probe(&mut child);
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn configure_info_probe_command(command: &mut ProcessCommand) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

fn terminate_info_probe(child: &mut Child) {
    #[cfg(unix)]
    terminate_info_probe_group(child);

    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(unix)]
fn terminate_info_probe_group(child: &mut Child) {
    let process_group = child.id();
    if let Ok(process_group) = i32::try_from(process_group) {
        kill_process_group(process_group, libc::SIGKILL);
        let _ = child.wait();

        let started = Instant::now();
        while process_group_exists(process_group) && started.elapsed() < INFO_KILL_GRACE {
            std::thread::sleep(Duration::from_millis(10));
        }
    } else {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(unix)]
fn kill_process_group(process_group: i32, signal: i32) {
    // SAFETY: Category 8 - FFI boundary. libc::kill is called with a negative
    // process-group id created by CommandExt::process_group(0) for this probe.
    // The call does not pass Rust references or memory across the FFI boundary.
    unsafe {
        libc::kill(-process_group, signal);
    }
}

#[cfg(unix)]
fn process_group_exists(process_group: i32) -> bool {
    // SAFETY: Category 8 - FFI boundary. Signal 0 performs only an OS existence
    // check for the process group id; no Rust memory or aliasing invariants cross
    // the FFI boundary.
    unsafe { libc::kill(-process_group, 0) == 0 }
}

fn parse_info_description(stdout: &str) -> Option<String> {
    let line = stdout
        .strip_suffix("\r\n")
        .or_else(|| stdout.strip_suffix('\n'))
        .unwrap_or(stdout);

    if line.contains('\n') || line.contains('\r') || line.chars().any(char::is_control) {
        return None;
    }

    let line = line.trim();
    if line.is_empty() || line.chars().count() > MAX_INFO_DESCRIPTION_CHARS {
        return None;
    }

    Some(line.to_owned())
}

#[cfg(test)]
mod tests;
