mod diff;
mod keywords;
mod templates;

use anyhow::Result;
use clap::{Parser, Subcommand};
use keywords::Keywords;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use templates::{Ace, Highlightjs, Rouge, Sublime, Template, Vim, Vscode};

#[derive(Parser)]
struct Opt {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build,
    Check,
}

fn main() -> Result<ExitCode> {
    let opt = Opt::parse();

    let mut root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root_dir.pop();
    root_dir.pop();

    let keywords = Keywords::load(&root_dir);
    let templates: Vec<Box<dyn Template>> = vec![
        Box::new(Ace),
        Box::new(Highlightjs),
        Box::new(Rouge),
        Box::new(Sublime),
        Box::new(Vim),
        Box::new(Vscode),
    ];

    let mut pass = true;
    for tmpl in templates {
        let path = root_dir.join(tmpl.path());
        let new = tmpl.apply(&keywords);
        let org = fs::read_to_string(&path).unwrap();

        match opt.command {
            Commands::Build => {
                let mut file = File::create(path).unwrap();
                let _ = file.write_all(new.as_bytes());
            }
            Commands::Check => {
                if new != org {
                    diff::print_diff(&path, &org, &new);
                    pass = false;
                }
            }
        }
    }

    if matches!(opt.command, Commands::Build) {
        build_highlightjs(&root_dir)?;
    }

    if pass {
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\n\nhighlightgen check failed.");
        println!("Please refer `support/highlightgen/README.md`");
        Ok(ExitCode::FAILURE)
    }
}

#[cfg(not(windows))]
const COMMAND_EXT: &str = "";

#[cfg(windows)]
const COMMAND_EXT: &str = ".exe";

fn build_highlightjs(root_dir: &Path) -> Result<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    let work_dir = temp_dir.path().join("highlight.js");
    let highlightjs_dir = root_dir.join("support/highlightjs");
    let symlink_target = work_dir.join("extra/highlightjs-veryl");
    let generated_file = work_dir.join("build/highlight.min.js");
    let target_file = root_dir.join("doc/book/theme/highlight.js");

    let _ = Command::new(format!("git{COMMAND_EXT}"))
        .arg("clone")
        .arg("https://github.com/highlightjs/highlight.js")
        .arg("-b")
        .arg("11.11.1")
        .current_dir(temp_dir.path())
        .output()?;

    symlink(&highlightjs_dir, &symlink_target)?;

    let _ = Command::new(format!("npm{COMMAND_EXT}"))
        .arg("install")
        .current_dir(&work_dir)
        .output()?;

    let _ = Command::new(format!("node{COMMAND_EXT}"))
        .arg("./tools/build.js")
        .arg("-t")
        .arg("cdn")
        .arg("veryl")
        .arg("verilog")
        .arg("ini")
        .arg("yaml")
        .current_dir(&work_dir)
        .output()?;

    fs::copy(&generated_file, &target_file)?;

    Ok(())
}

#[cfg(not(windows))]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst)?;
    Ok(())
}

#[cfg(windows)]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)?;
    Ok(())
}
