use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use veryl_metadata::Metadata;
use veryl_parser::miette::Result;

mod cmd_build;
mod cmd_check;
mod cmd_dump;
mod cmd_fmt;
mod cmd_init;
mod cmd_metadata;
mod cmd_new;
mod utils;

// ---------------------------------------------------------------------------------------------------------------------
// Opt
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Opt {
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
    Metadata(OptMetadata),
    Dump(OptDump),
}

/// Create a new package
#[derive(Args)]
pub struct OptNew {
    pub path: PathBuf,

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

/// Create a new package in an existing directory
#[derive(Args)]
pub struct OptInit {
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

/// Format the current package
#[derive(Args)]
pub struct OptFmt {
    /// Target files
    pub files: Vec<PathBuf>,

    /// Run fmt in check mode
    #[arg(long)]
    pub check: bool,

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

/// Analyze the current package
#[derive(Args)]
pub struct OptCheck {
    /// Target files
    pub files: Vec<PathBuf>,

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

/// Build the target codes corresponding to the current package
#[derive(Args)]
pub struct OptBuild {
    /// Target files
    pub files: Vec<PathBuf>,

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

/// Dump metadata of the current packege
#[derive(Args)]
pub struct OptMetadata {
    /// output format
    #[arg(long, value_enum, default_value_t)]
    pub format: Format,

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
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

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

// ---------------------------------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------------------------------

fn main() -> Result<ExitCode> {
    env_logger::init();
    let opt = Opt::parse();

    let metadata = match opt.command {
        Commands::New(_) | Commands::Init(_) => {
            // dummy metadata
            let metadata = utils::create_default_toml("");
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
        Commands::Fmt(x) => cmd_fmt::CmdFmt::new(x).exec(&metadata)?,
        Commands::Check(x) => cmd_check::CmdCheck::new(x).exec(&metadata)?,
        Commands::Build(x) => {
            let opt_check = OptCheck {
                files: x.files.clone(),
                quiet: x.quiet,
                verbose: x.verbose,
            };
            cmd_check::CmdCheck::new(opt_check).exec(&metadata)?;
            cmd_build::CmdBuild::new(x).exec(&metadata)?
        }
        Commands::Metadata(x) => cmd_metadata::CmdMetadata::new(x).exec(&metadata)?,
        Commands::Dump(x) => cmd_dump::CmdDump::new(x).exec(&metadata)?,
    };
    if ret {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
