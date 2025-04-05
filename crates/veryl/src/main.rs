use clap::{CommandFactory, Parser};
use clap_complete::aot::Shell;
use console::Style;
use fern::Dispatch;
use log::debug;
use log::{Level, LevelFilter};
use miette::{IntoDiagnostic, Result};
use std::process::ExitCode;
use std::str::FromStr;
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
                    12_usize.saturating_sub(
                        format!("{message}")
                            .split_ascii_whitespace()
                            .next()
                            .unwrap()
                            .len()
                    )
                ),
                message
            ))
        })
        .level(level)
        .level_for("parol_runtime", LevelFilter::Warn)
        .chain(std::io::stderr())
        .apply()
        .into_diagnostic()?;

    let (mut metadata, dot_build_lock) = match opt.command {
        Commands::New(_) | Commands::Init(_) => {
            // dummy metadata
            let metadata = Metadata::create_default_toml("dummy").unwrap();
            (Metadata::from_str(&metadata)?, None)
        }
        _ => {
            let metadata_path = Metadata::search_from_current()?;
            let metadata = Metadata::load(metadata_path)?;

            let dot_build = metadata.project_dot_build_path();
            let dot_build_lock = veryl_path::lock_dir(&dot_build)?;
            (metadata, Some(dot_build_lock))
        }
    };

    let mut stopwatch = StopWatch::new();

    let ret = match opt.command {
        Commands::New(x) => cmd_new::CmdNew::new(x).exec()?,
        Commands::Init(x) => cmd_init::CmdInit::new(x).exec()?,
        Commands::Fmt(x) => cmd_fmt::CmdFmt::new(x).exec(&mut metadata, opt.quiet)?,
        Commands::Check(x) => cmd_check::CmdCheck::new(x).exec(&mut metadata)?,
        Commands::Build(x) => {
            let ret = cmd_build::CmdBuild::new(x).exec(&mut metadata, false, opt.quiet);
            metadata.save_build_info()?;
            ret?
        }
        Commands::Clean(x) => cmd_clean::CmdClean::new(x).exec(&mut metadata)?,
        Commands::Update(x) => cmd_update::CmdUpdate::new(x).exec(&mut metadata)?,
        Commands::Publish(x) => cmd_publish::CmdPublish::new(x).exec(&mut metadata)?,
        Commands::Migrate(x) => cmd_migrate::CmdMigrate::new(x).exec(&mut metadata, opt.quiet)?,
        Commands::Doc(x) => cmd_doc::CmdDoc::new(x).exec(&mut metadata)?,
        Commands::Metadata(x) => cmd_metadata::CmdMetadata::new(x).exec(&metadata)?,
        Commands::Dump(x) => cmd_dump::CmdDump::new(x).exec(&mut metadata)?,
        Commands::Test(x) => cmd_test::CmdTest::new(x).exec(&mut metadata)?,
    };

    if let Some(dot_build_lock) = dot_build_lock {
        veryl_path::unlock_dir(dot_build_lock)?;
    }

    debug!("Elapsed time ({} milliseconds)", stopwatch.lap());

    if ret {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
