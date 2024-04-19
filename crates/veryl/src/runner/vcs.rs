use crate::runner::Runner;
use futures::prelude::*;
use log::{error, info};
use miette::{IntoDiagnostic, Result};
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::runtime::Runtime;
use tokio_util::codec::{FramedRead, LinesCodec};
use veryl_metadata::Metadata;
use veryl_parser::resource_table::StrId;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    Idle,
    CompileWarning,
    CompileError,
    SimulateInfo,
    SimulateWarning,
    SimulateError,
    SimulateFatal,
}

pub struct Vcs {
    state: State,
    success: bool,
}

impl Vcs {
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
                if line.starts_with("Info: \"") {
                    self.state = State::SimulateInfo;
                } else if line.starts_with("Warning: \"") {
                    self.state = State::SimulateWarning;
                } else if line.starts_with("Error: \"") {
                    self.state = State::SimulateError;
                } else if line.starts_with("Fatal: \"") {
                    self.state = State::SimulateFatal;
                } else if line.starts_with("Warning-") {
                    self.state = State::CompileWarning;
                    self.warning(line);
                } else if line.starts_with("Error-") {
                    self.state = State::CompileError;
                    self.error(line);
                }
            }
            State::SimulateInfo => {
                self.info(line);
                self.state = State::Idle;
            }
            State::SimulateWarning => {
                self.warning(line);
                self.state = State::Idle;
            }
            State::SimulateError => {
                self.error(line);
                self.state = State::Idle;
            }
            State::SimulateFatal => {
                self.fatal(line);
                self.state = State::Idle;
            }
            State::CompileWarning => {
                if line.is_empty() {
                    self.state = State::Idle;
                } else {
                    self.warning(line);
                }
            }
            State::CompileError => {
                if line.is_empty() {
                    self.state = State::Idle;
                } else {
                    self.error(line);
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
        Ok(())
    }
}

impl Runner for Vcs {
    fn run(&mut self, metadata: &Metadata, test: StrId) -> Result<bool> {
        self.success = true;

        let temp_dir = tempfile::tempdir().into_diagnostic()?;

        info!("Compiling test ({})", test);

        let define = format!("+define+__veryl_test_{}_{}__", metadata.project.name, test);

        let rt = Runtime::new().unwrap();

        rt.block_on(async {
            let compile = Command::new("vcs")
                .arg("-sverilog")
                .arg("-f")
                .arg(metadata.filelist_path())
                .arg(&define)
                .args(&metadata.test.vcs.compile_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .into_diagnostic()?;

            self.parse(compile).await
        })?;

        if !self.success {
            error!("Failed compile ({})", test);
            return Ok(false);
        }

        info!("Executing test ({})", test);

        rt.block_on(async {
            let simulate = Command::new("./simv")
                .args(&metadata.test.vcs.simulate_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .into_diagnostic()?;

            self.parse(simulate).await
        })?;

        if self.success {
            info!("Succeeded test ({})", test);
            Ok(true)
        } else {
            error!("Failed test ({})", test);
            Ok(false)
        }
    }

    fn name(&self) -> &'static str {
        "VCS"
    }

    fn failure(&mut self) {
        self.success = false;
    }
}
