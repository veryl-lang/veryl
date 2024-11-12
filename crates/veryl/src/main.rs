use clap::{CommandFactory, Parser};
use clap_complete::aot::Shell;
use console::Style;
use fern::Dispatch;
use log::debug;
use log::{Level, LevelFilter};
use miette::{IntoDiagnostic, Result};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Instant;
use veryl_metadata::Metadata;

use veryl::*;

// ---------------------------------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------------------------------

fn main() -> Result<ExitCode> {
    let opt = Opt::parse();

    if let Some(shell) = opt.completion {
        let shell = match shell {
            CompletionShell::Bash => Shell::Bash,
            CompletionShell::Elvish => Shell::Elvish,
            CompletionShell::Fish => Shell::Fish,
            CompletionShell::PowerShell => Shell::PowerShell,
            CompletionShell::Zsh => Shell::Zsh,
        };
        clap_complete::generate(shell, &mut Opt::command(), "veryl", &mut std::io::stdout());
        return Ok(ExitCode::SUCCESS);
    }

    let level = if opt.verbose {
        LevelFilter::Debug
    } else if opt.quiet {
        LevelFilter::Warn
    } else {
        LevelFilter::Info
    };

    Dispatch::new()
        .format(|out, message, record| {
            let style = match record.level() {
                Level::Error => Style::new().red().bright(),
                Level::Warn => Style::new().yellow().bright(),
                Level::Info => Style::new().green().bright(),
                Level::Debug => Style::new().cyan().bright(),
                Level::Trace => Style::new().magenta().bright(),
            };
            out.finish(format_args!(
                "{} {}{}",
                style.apply_to(format!("[{:<5}]", record.level())),
                " ".repeat(
                    12 - format!("{message}")
                        .split_ascii_whitespace()
                        .next()
                        .unwrap()
                        .len()
                ),
                message
            ))
        })
        .level(level)
        .level_for("parol_runtime", LevelFilter::Warn)
        .chain(std::io::stderr())
        .apply()
        .into_diagnostic()?;

    let mut metadata = match opt.command {
        Commands::New(_) | Commands::Init(_) => {
            // dummy metadata
            let metadata = Metadata::create_default_toml("dummy").unwrap();
            Metadata::from_str(&metadata)?
        }
        _ => {
            let metadata_path = Metadata::search_from_current()?;
            Metadata::load(metadata_path)?
        }
    };

    let now = Instant::now();

    let ret = match opt.command {
        Commands::New(x) => cmd_new::CmdNew::new(x).exec()?,
        Commands::Init(x) => cmd_init::CmdInit::new(x).exec()?,
        Commands::Fmt(x) => cmd_fmt::CmdFmt::new(x).exec(&mut metadata)?,
        Commands::Check(x) => cmd_check::CmdCheck::new(x).exec(&mut metadata)?,
        Commands::Build(x) => cmd_build::CmdBuild::new(x).exec(&mut metadata, false)?,
        Commands::Clean(x) => cmd_clean::CmdClean::new(x).exec(&mut metadata)?,
        Commands::Update(x) => cmd_update::CmdUpdate::new(x).exec(&mut metadata)?,
        Commands::Publish(x) => cmd_publish::CmdPublish::new(x).exec(&mut metadata)?,
        Commands::Doc(x) => cmd_doc::CmdDoc::new(x).exec(&mut metadata)?,
        Commands::Metadata(x) => cmd_metadata::CmdMetadata::new(x).exec(&metadata)?,
        Commands::Dump(x) => cmd_dump::CmdDump::new(x).exec(&mut metadata)?,
        Commands::Test(x) => cmd_test::CmdTest::new(x).exec(&mut metadata)?,
    };

    let elapsed_time = now.elapsed();
    debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

    if ret {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
