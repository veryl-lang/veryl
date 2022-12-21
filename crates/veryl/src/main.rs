use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use veryl_parser::miette::Result;

mod cmd_check;
mod cmd_emit;
mod cmd_fmt;
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
    Fmt(Fmt),
    Check(Check),
    Emit(Emit),
}

/// Format the current package
#[derive(Args)]
pub struct Fmt {
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
pub struct Check {
    /// Target files
    pub files: Vec<PathBuf>,

    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

/// Emit the target codes corresponding to the current package
#[derive(Args)]
pub struct Emit {
    /// Target files
    pub files: Vec<PathBuf>,

    /// Directory for all generated artifacts
    #[arg(long)]
    pub target_directory: Option<PathBuf>,

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
    let ret = match opt.command {
        Commands::Fmt(x) => cmd_fmt::CmdFmt::new(x).exec()?,
        Commands::Check(x) => cmd_check::CmdCheck::new(x).exec()?,
        Commands::Emit(x) => cmd_emit::CmdEmit::new(x).exec()?,
    };
    if ret {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
