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
pub mod cmd_migrate;
pub mod cmd_new;
pub mod cmd_publish;
pub mod cmd_synth;
pub mod cmd_test;
pub mod cmd_translate;
pub mod cmd_update;
pub mod context;
pub mod diff;
pub mod doc;
pub mod runner;
pub mod stopwatch;
pub mod utils;
pub use stopwatch::StopWatch;

// ---------------------------------------------------------------------------------------------------------------------
// Opt
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
#[clap(version(veryl_metadata::VERYL_VERSION))]
#[clap(long_version(veryl_metadata::VERYL_VERSION))]
pub struct Opt {
    /// No output printed to stdout
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Use trace output
    #[arg(long, global = true)]
    pub trace: bool,

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
    Migrate(OptMigrate),
    Doc(OptDoc),
    Metadata(OptMetadata),
    Dump(OptDump),
    Test(OptTest),
    Synth(OptSynth),
    Translate(OptTranslate),
}

/// Translate SystemVerilog files into Veryl source
#[derive(Args)]
pub struct OptTranslate {
    /// Input SystemVerilog files. Each `foo.sv` produces a sibling `foo.veryl`.
    pub files: Vec<PathBuf>,

    /// Write the result to stdout instead of writing files. Useful for piping
    /// or redirecting to a non-default path.
    #[arg(long)]
    pub stdout: bool,

    /// Fail if any unsupported constructs are encountered
    #[arg(long)]
    pub strict: bool,

    /// Skip the Veryl formatter pass and emit the raw translator output
    #[arg(long = "no-format")]
    pub no_format: bool,
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

    /// Run build in check mode
    #[arg(long)]
    pub check: bool,
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

/// Migrate breaking changes from the previous version
#[derive(Args)]
pub struct OptMigrate {
    /// Target files
    pub files: Vec<PathBuf>,

    /// Run fmt in check mode
    #[arg(long)]
    pub check: bool,
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

    /// Test name filter (substring match)
    #[arg(short = 't', long = "test")]
    pub test: Option<String>,

    /// Simulator
    #[arg(long, value_enum)]
    pub sim: Option<SimType>,

    /// Dump waveform
    #[arg(long)]
    pub wave: bool,

    /// Disable JIT compilation for native tests
    #[arg(long)]
    pub disable_jit: bool,

    /// Disable FF classification optimization (force all always_ff variables to FF)
    #[arg(long)]
    pub disable_ff_opt: bool,

    /// Run only ignored tests
    #[arg(long)]
    pub ignored: bool,

    /// Run both ignored and non-ignored tests
    #[arg(long)]
    pub include_ignored: bool,

    /// Define a name visible to `#[ifdef]` (can be specified multiple times).
    /// Merged with `[test].defines` from Veryl.toml.
    #[arg(short = 'D', long = "define", value_name = "NAME")]
    pub define: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SimType {
    /// Verilator
    Verilator,
    /// Synopsys VCS
    Vcs,
    /// Altair DSim
    Dsim,
    /// AMD Vivado Simulator
    Vivado,
}

impl From<SimType> for veryl_metadata::SimType {
    fn from(x: SimType) -> Self {
        match x {
            SimType::Dsim => veryl_metadata::SimType::Dsim,
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

    /// output namespace table
    #[arg(long)]
    pub namespace_table: bool,

    /// output type dag
    #[arg(long)]
    pub type_dag: bool,

    /// output file dag
    #[arg(long)]
    pub file_dag: bool,

    /// output attribute table
    #[arg(long)]
    pub attribute_table: bool,

    /// output unsafe table
    #[arg(long)]
    pub unsafe_table: bool,

    /// output IR
    #[arg(long)]
    pub ir: bool,
}

/// Synthesize to a simple gate-level netlist and report area / critical path.
///
/// Design-parameter knobs (`clock_freq`, `activity`) and the default `top` /
/// `timing_paths` live in the `[synth]` section of `Veryl.toml`. CLI
/// `--top` and `--timing-paths` override the toml setting when supplied.
#[derive(Args)]
pub struct OptSynth {
    /// Target files
    pub files: Vec<PathBuf>,

    /// Top module name (overrides `synth.top` in Veryl.toml; otherwise
    /// inferred from the first user module)
    #[arg(long)]
    pub top: Option<String>,

    /// Number of worst-delay endpoints to report when dumping timing
    /// (overrides `synth.timing_paths` in Veryl.toml)
    #[arg(long)]
    pub timing_paths: Option<usize>,

    /// Dump the gate-level IR (netlist of gates and flip-flops)
    #[arg(long)]
    pub dump_ir: bool,

    /// Dump the critical path trace
    #[arg(long)]
    pub dump_timing: bool,

    /// Dump the per-cell-kind area breakdown
    #[arg(long)]
    pub dump_area: bool,

    /// Dump the power estimate (leakage + dynamic breakdown)
    #[arg(long)]
    pub dump_power: bool,
}
