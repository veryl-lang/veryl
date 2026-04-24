use clap::Parser;
use console::Style;
use fern::Dispatch;
use log::{Level, LevelFilter, info, warn};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use veryl_metadata::{Metadata, NewlineStyle};
use veryl_translator::translate_str;

/// Translate SystemVerilog files into Veryl source.
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Opt {
    /// Input SystemVerilog files. Each `foo.sv` produces a sibling `foo.veryl`.
    files: Vec<PathBuf>,

    /// Write the result to stdout instead of writing files.
    #[arg(long)]
    stdout: bool,

    /// Fail if any unsupported constructs are encountered.
    #[arg(long)]
    strict: bool,

    /// Skip the Veryl formatter pass and emit the raw translator output.
    #[arg(long = "no-format")]
    no_format: bool,

    /// No output printed to stdout.
    #[arg(long)]
    quiet: bool,

    /// Use verbose output.
    #[arg(long)]
    verbose: bool,

    /// Use trace output.
    #[arg(long)]
    trace: bool,
}

fn main() -> Result<ExitCode> {
    let opt = Opt::parse();

    let level = if opt.trace {
        LevelFilter::Trace
    } else if opt.verbose {
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
        .level_for("veryl_parser", LevelFilter::Warn)
        .level_for("parol_runtime", LevelFilter::Warn)
        .level_for("scnr", LevelFilter::Warn)
        .chain(std::io::stderr())
        .apply()
        .into_diagnostic()?;

    if opt.files.is_empty() {
        return Err(miette::miette!(
            "no input files; pass one or more `.sv` paths"
        ));
    }

    let newline_style = match Metadata::search_from_current() {
        Ok(p) => Metadata::load(p)
            .map(|m| m.format.newline_style)
            .unwrap_or(NewlineStyle::Auto),
        Err(_) => NewlineStyle::Auto,
    };

    let mut all_pass = true;
    for input in &opt.files {
        all_pass &= translate_one(&opt, input, newline_style)?;
    }

    if all_pass {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

fn translate_one(opt: &Opt, input: &Path, newline_style: NewlineStyle) -> Result<bool> {
    info!("Translating ({})", input.to_string_lossy());

    let src = fs::read_to_string(input)
        .into_diagnostic()
        .wrap_err("failed to read input")?;

    let out = translate_str(&src, input, !opt.no_format, newline_style)
        .map_err(|e| miette::miette!("{e}"))?;

    if !out.unsupported.is_empty() {
        for r in &out.unsupported {
            warn!("unsupported {} at line {}", r.kind, r.line);
        }
        warn!(
            "{}: {} unsupported construct(s)",
            input.to_string_lossy(),
            out.unsupported.len()
        );
    }

    if opt.stdout {
        print!("{}", out.veryl);
    } else {
        let dest = sibling_veryl_path(input)?;
        fs::write(&dest, out.veryl.as_bytes())
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to write {}", dest.to_string_lossy()))?;
        info!("Wrote ({})", dest.to_string_lossy());
    }

    if opt.strict && !out.unsupported.is_empty() {
        Ok(false)
    } else {
        Ok(true)
    }
}

fn sibling_veryl_path(input: &Path) -> Result<PathBuf> {
    let mut dest = input.to_path_buf();
    if !dest.set_extension("veryl") {
        return Err(miette::miette!(
            "cannot derive output path for {}",
            input.to_string_lossy()
        ));
    }
    Ok(dest)
}
