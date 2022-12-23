use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use veryl_metadata::Metadata;
use veryl_parser::miette::Result;

mod cmd_build;
mod cmd_check;
mod cmd_fmt;
mod cmd_metadata;
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
    Fmt(OptFmt),
    Check(OptCheck),
    Build(OptBuild),
    Metadata(OptMetadata),
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

    let metadata_path = Metadata::search_from_current()?;
    let metadata = Metadata::load(metadata_path)?;

    let ret = match opt.command {
        Commands::Fmt(x) => cmd_fmt::CmdFmt::new(x).exec(&metadata)?,
        Commands::Check(x) => cmd_check::CmdCheck::new(x).exec(&metadata)?,
        Commands::Build(x) => cmd_build::CmdBuild::new(x).exec(&metadata)?,
        Commands::Metadata(x) => cmd_metadata::CmdMetadata::new(x).exec(&metadata)?,
    };
    if ret {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
