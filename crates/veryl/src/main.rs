use clap::{Args, Parser, Subcommand, ValueEnum};
use console::Style;
use fern::Dispatch;
use log::{Level, LevelFilter};
use miette::{IntoDiagnostic, Result};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use veryl_metadata::Metadata;

mod cmd_build;
mod cmd_check;
mod cmd_doc;
mod cmd_dump;
mod cmd_fmt;
mod cmd_init;
mod cmd_metadata;
mod cmd_new;
mod cmd_publish;
mod cmd_update;
mod doc_builder;

// ---------------------------------------------------------------------------------------------------------------------
// Opt
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Opt {
    /// No output printed to stdout
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    New(OptNew),
    Init(OptInit),
    Fmt(OptFmt),
    Check(OptCheck),
    Build(OptBuild),
    Update(OptUpdate),
    Publish(OptPublish),
    Doc(OptDoc),
    Metadata(OptMetadata),
    Dump(OptDump),
}

/// Create a new project
#[derive(Args)]
pub struct OptNew {
    pub path: PathBuf,
}

/// Create a new project in an existing directory
#[derive(Args)]
pub struct OptInit {
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

/// Format the current project
#[derive(Args)]
pub struct OptFmt {
    /// Target files
    pub files: Vec<PathBuf>,

    /// Run fmt in check mode
    #[arg(long)]
    pub check: bool,
}

/// Analyze the current project
#[derive(Args)]
pub struct OptCheck {
    /// Target files
    pub files: Vec<PathBuf>,
}

/// Build the target codes corresponding to the current project
#[derive(Args)]
pub struct OptBuild {
    /// Target files
    pub files: Vec<PathBuf>,
}

/// Update dependencies
#[derive(Args)]
pub struct OptUpdate {}

/// Publish the current project
#[derive(Args)]
pub struct OptPublish {
    /// Bump version
    #[arg(long)]
    pub bump: Option<BumpKind>,
}

/// Build the document corresponding to the current project
#[derive(Args)]
pub struct OptDoc {
    /// Target files
    pub files: Vec<PathBuf>,
}

#[derive(Clone, Copy, Default, Debug, ValueEnum)]
pub enum BumpKind {
    /// Increment majoir version
    Major,
    /// Increment minor version
    Minor,
    /// Increment patch version
    #[default]
    Patch,
}

impl From<BumpKind> for veryl_metadata::BumpKind {
    fn from(x: BumpKind) -> Self {
        match x {
            BumpKind::Major => veryl_metadata::BumpKind::Major,
            BumpKind::Minor => veryl_metadata::BumpKind::Minor,
            BumpKind::Patch => veryl_metadata::BumpKind::Patch,
        }
    }
}

/// Dump metadata of the current packege
#[derive(Args)]
pub struct OptMetadata {
    /// output format
    #[arg(long, value_enum, default_value_t)]
    pub format: Format,
}

#[derive(Clone, Copy, Default, Debug, ValueEnum)]
pub enum Format {
    #[default]
    Pretty,
    Json,
}

/// Dump debug info
#[derive(Args)]
pub struct OptDump {
    /// Target files
    pub files: Vec<PathBuf>,

    /// output syntex tree
    #[arg(long)]
    pub syntax_tree: bool,

    /// output symbol table
    #[arg(long)]
    pub symbol_table: bool,

    /// output namespace table
    #[arg(long)]
    pub namespace_table: bool,
}

// ---------------------------------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------------------------------

fn main() -> Result<ExitCode> {
    let opt = Opt::parse();

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

    let ret = match opt.command {
        Commands::New(x) => cmd_new::CmdNew::new(x).exec()?,
        Commands::Init(x) => cmd_init::CmdInit::new(x).exec()?,
        Commands::Fmt(x) => cmd_fmt::CmdFmt::new(x).exec(&mut metadata)?,
        Commands::Check(x) => cmd_check::CmdCheck::new(x).exec(&mut metadata)?,
        Commands::Build(x) => cmd_build::CmdBuild::new(x).exec(&mut metadata)?,
        Commands::Update(x) => cmd_update::CmdUpdate::new(x).exec(&mut metadata)?,
        Commands::Publish(x) => cmd_publish::CmdPublish::new(x).exec(&mut metadata)?,
        Commands::Doc(x) => cmd_doc::CmdDoc::new(x).exec(&mut metadata)?,
        Commands::Metadata(x) => cmd_metadata::CmdMetadata::new(x).exec(&metadata)?,
        Commands::Dump(x) => cmd_dump::CmdDump::new(x).exec(&mut metadata)?,
    };
    if ret {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
