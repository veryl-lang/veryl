use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::aot::Shell;
use console::Style;
use fern::Dispatch;
use log::debug;
use log::{Level, LevelFilter};
use miette::{IntoDiagnostic, Result};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Instant;
use veryl_metadata::Metadata;

mod cmd_build;
mod cmd_check;
mod cmd_clean;
mod cmd_doc;
mod cmd_dump;
mod cmd_fmt;
mod cmd_init;
mod cmd_metadata;
mod cmd_new;
mod cmd_publish;
mod cmd_test;
mod cmd_update;
mod doc;
mod runner;

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

    /// Generate tab-completion
    #[arg(long, global = true, hide = true)]
    pub completion: Option<CompletionShell>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
}

#[derive(Subcommand)]
enum Commands {
    New(OptNew),
    Init(OptInit),
    Fmt(OptFmt),
    Check(OptCheck),
    Build(OptBuild),
    Clean(OptClean),
    Update(OptUpdate),
    Publish(OptPublish),
    Doc(OptDoc),
    Metadata(OptMetadata),
    Dump(OptDump),
    Test(OptTest),
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

/// Clean-up the current project
#[derive(Args)]
pub struct OptClean {}

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

/// Execute tests
#[derive(Args)]
pub struct OptTest {
    /// Target files
    pub files: Vec<PathBuf>,

    /// Simulator
    #[arg(long, value_enum)]
    pub sim: Option<SimType>,

    /// Dump waveform
    #[arg(long)]
    pub wave: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SimType {
    /// Verilator
    Verilator,
    /// Synopsys VCS
    Vcs,
    /// AMD Vivado Simulator
    Vivado,
}

impl From<SimType> for veryl_metadata::SimType {
    fn from(x: SimType) -> Self {
        match x {
            SimType::Verilator => veryl_metadata::SimType::Verilator,
            SimType::Vcs => veryl_metadata::SimType::Vcs,
            SimType::Vivado => veryl_metadata::SimType::Vivado,
        }
    }
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

    /// output assign list
    #[arg(long)]
    pub assign_list: bool,

    /// output namespace table
    #[arg(long)]
    pub namespace_table: bool,

    /// output type dag
    #[arg(long)]
    pub type_dag: bool,

    /// output attribute table
    #[arg(long)]
    pub attribute_table: bool,

    /// output unsafe table
    #[arg(long)]
    pub unsafe_table: bool,
}

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
        Commands::Build(x) => cmd_build::CmdBuild::new(x).exec(&mut metadata)?,
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
