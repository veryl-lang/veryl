use crate::runner::{copy_wave, remap_msg_by_regex, Runner};
use futures::prelude::*;
use log::{error, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::runtime::Runtime;
use tokio_util::codec::{FramedRead, LinesCodec};
use veryl_metadata::Metadata;
use veryl_parser::resource_table::{PathId, StrId};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    Idle,
    CompileWarning,
    CompileError,
}

pub struct Verilator {
    state: State,
    success: bool,
}

fn parse_msg(line: &str) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^:]*:[^:]*:[^:]*:[^:]*: (.*)").unwrap());

    if let Some(caps) = RE.captures(line) {
        caps[1].to_string()
    } else {
        String::new()
    }
}

fn remap_msg(line: &str) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?<path>[^: ]+):(?<line>[0-9]+)(?::(?<column>[0-9]+))?").unwrap()
    });

    remap_msg_by_regex(line, &RE)
}

impl Verilator {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            success: true,
        }
    }

    pub fn runner(self) -> Box<dyn Runner> {
        Box::new(self) as Box<dyn Runner>
    }

    fn parse_line(&mut self, line: &str) {
        self.debug(line);

        match self.state {
            State::Idle => {
                if line.starts_with("[0] -Info: ") {
                    self.info(&parse_msg(line));
                } else if line.starts_with("[0] %Warning: ") {
                    self.warning(&parse_msg(line));
                } else if line.starts_with("[0] %Error: ") {
                    self.error(&parse_msg(line));
                } else if line.starts_with("[0] %Fatal: ") {
                    self.fatal(&parse_msg(line));
                } else if line.starts_with("%Warning:") {
                    self.state = State::CompileWarning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("%Error:") {
                    self.state = State::CompileError;
                    self.error(&remap_msg(line));
                }
            }
            State::CompileWarning => {
                if line.starts_with(' ') {
                    self.warning(&remap_msg(line));
                } else if line.starts_with("%Warning") {
                    self.state = State::CompileWarning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("%Error") {
                    self.state = State::CompileError;
                    self.error(&remap_msg(line));
                } else {
                    self.state = State::Idle;
                }
            }
            State::CompileError => {
                if line.starts_with(' ') {
                    self.error(&remap_msg(line));
                } else if line.starts_with("%Warning") {
                    self.state = State::CompileWarning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("%Error") {
                    self.state = State::CompileError;
                    self.error(&remap_msg(line));
                } else {
                    self.state = State::Idle;
                }
            }
        }
    }

    async fn parse(&mut self, mut child: Child) -> Result<()> {
        let stdout = child.stdout.take().unwrap();
        let mut reader = FramedRead::new(stdout, LinesCodec::new());
        while let Some(line) = reader.next().await {
            let line = line.into_diagnostic()?;
            self.parse_line(&line);
        }

        let stderr = child.stderr.take().unwrap();
        let mut reader = FramedRead::new(stderr, LinesCodec::new());
        while let Some(line) = reader.next().await {
            let line = line.into_diagnostic()?;
            self.parse_line(&line);
        }
        Ok(())
    }
}

impl Default for Verilator {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for Verilator {
    fn run(
        &mut self,
        metadata: &Metadata,
        test: StrId,
        _top: Option<StrId>,
        path: PathId,
        wave: bool,
    ) -> Result<bool> {
        self.success = true;

        let temp_dir = tempfile::tempdir().into_diagnostic()?;

        info!("Compiling test ({})", test);

        let mut defines = vec![format!(
            "+define+__veryl_test_{}_{}__",
            metadata.project.name, test
        )];

        if wave {
            defines.push(format!(
                "+define+__veryl_wavedump_{}_{}__",
                metadata.project.name, test
            ));
        }

        let mut opt = vec!["--assert", "--binary", "-Wno-MULTITOP"];

        if wave {
            opt.push("--trace");
        }

        let rt = Runtime::new().unwrap();

        rt.block_on(async {
            let compile = Command::new("verilator")
                .args(&opt)
                .arg("-f")
                .arg(metadata.filelist_path())
                .arg("-o")
                .arg("simv")
                .args(&defines)
                .args(&metadata.test.verilator.compile_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run \"verilator\"")?;

            self.parse(compile).await
        })?;

        if !self.success {
            error!("Failed compile ({})", test);
            return Ok(false);
        }

        info!("Executing test ({})", test);

        rt.block_on(async {
            let simulate = Command::new("./obj_dir/simv")
                .args(&metadata.test.verilator.simulate_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run simulation binary")?;

            self.parse(simulate).await
        })?;

        if wave {
            copy_wave(test, path, metadata, temp_dir.path())?;
        }

        if self.success {
            info!("Succeeded test ({})", test);
            Ok(true)
        } else {
            error!("Failed test ({})", test);
            Ok(false)
        }
    }

    fn name(&self) -> &'static str {
        "Verilator"
    }

    fn failure(&mut self) {
        self.success = false;
    }
}
