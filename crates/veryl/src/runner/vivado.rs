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
    SimulateInfo,
    SimulateWarning,
    SimulateError,
    SimulateFatal,
}

pub struct Vivado {
    state: State,
    success: bool,
}

fn remap_msg(line: &str) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?<path>[^: \[\]]+):(?<line>[0-9]+)(?::(?<column>[0-9]+))?").unwrap()
    });

    remap_msg_by_regex(line, &RE)
}

impl Vivado {
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
                if line.starts_with("Info: ") {
                    self.state = State::SimulateInfo;
                    self.info(line.strip_prefix("Info: ").unwrap());
                } else if line.starts_with("Warning: ") {
                    self.state = State::SimulateWarning;
                    self.warning(line.strip_prefix("Warning: ").unwrap());
                } else if line.starts_with("Error: ") {
                    self.state = State::SimulateError;
                    self.error(line.strip_prefix("Error: ").unwrap());
                } else if line.starts_with("Fatal: ") {
                    self.state = State::SimulateFatal;
                    self.fatal(line.strip_prefix("Fatal: ").unwrap());
                } else if line.starts_with("WARNING:") {
                    self.warning(&remap_msg(line));
                } else if line.starts_with("ERROR:") {
                    self.error(&remap_msg(line));
                }
            }
            State::SimulateInfo => {
                self.state = State::Idle;
            }
            State::SimulateWarning => {
                self.state = State::Idle;
            }
            State::SimulateError => {
                self.state = State::Idle;
            }
            State::SimulateFatal => {
                self.state = State::Idle;
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

impl Default for Vivado {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for Vivado {
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

        let mut defines = vec![
            "-d".to_string(),
            format!("__veryl_test_{}_{}__", metadata.project.name, test),
        ];

        if wave {
            defines.push("-d".to_string());
            defines.push(format!(
                "__veryl_wavedump_{}_{}__",
                metadata.project.name, test
            ));
        }

        let rt = Runtime::new().unwrap();

        rt.block_on(async {
            let compile = Command::new("xvlog")
                .arg("--sv")
                .arg("-f")
                .arg(metadata.filelist_path())
                .args(&defines)
                .args(&metadata.test.vivado.compile_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run \"xvlog\"")?;

            self.parse(compile).await
        })?;

        if !self.success {
            error!("Failed compile ({})", test);
            return Ok(false);
        }

        info!("Elaborating test ({})", test);

        let opt = if wave { vec!["-debug", "all"] } else { vec![] };

        let mut top = vec![test.to_string()];
        if wave {
            top.push("__veryl_wavedump".to_string());
        }

        rt.block_on(async {
            let elaborate = Command::new("xelab")
                .args(top)
                .args(opt)
                .arg("-s")
                .arg("simv")
                .args(&metadata.test.vivado.elaborate_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run \"xelab\"")?;

            self.parse(elaborate).await
        })?;

        if !self.success {
            error!("Failed elaborate ({})", test);
            return Ok(false);
        }

        info!("Executing test ({})", test);

        rt.block_on(async {
            let simulate = Command::new("xsim")
                .arg("simv")
                .arg("--runall")
                .args(&metadata.test.vivado.simulate_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run \"xsim\"")?;

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
        "Vivado"
    }

    fn failure(&mut self) {
        self.success = false;
    }
}
