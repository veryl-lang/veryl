use clap::{Args, Parser, Subcommand};
use miette::Result;
use std::path::PathBuf;
use std::process::ExitCode;

mod format;
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

#[derive(Args)]
pub struct Check {
    /// No output printed to stdout
    #[arg(long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Args)]
struct Emit {
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
        Commands::Fmt(x) => format::format(&x)?,
        Commands::Check(_) => todo!(),
        Commands::Emit(_) => todo!(),
    };
    if ret {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
