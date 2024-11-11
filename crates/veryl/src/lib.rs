use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub mod cmd_build;
pub mod cmd_check;
pub mod cmd_clean;
pub mod cmd_doc;
pub mod cmd_dump;
pub mod cmd_fmt;
pub mod cmd_init;
pub mod cmd_metadata;
pub mod cmd_new;
pub mod cmd_publish;
pub mod cmd_test;
pub mod cmd_update;
pub mod doc;
pub mod runner;

// ---------------------------------------------------------------------------------------------------------------------
// Opt
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Opt {
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
    pub command: Commands,
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
pub enum Commands {
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
