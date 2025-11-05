use crate::runner::{Runner, copy_wave, remap_msg_by_regex};
use futures::prelude::*;
use log::{error, info, warn};
use miette::{IntoDiagnostic, Result, WrapErr};
use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::runtime::Runtime;
use tokio_util::codec::{FramedRead, LinesCodec};
use veryl_metadata::{Metadata, WaveFormFormat};
use veryl_parser::resource_table::{PathId, StrId};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Info,
    Warning,
    Error,
    Fatal,
}

pub struct Dsim {
    state: State,
    success: bool,
}

fn remap_msg(line: &str) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r###"(?<path>[^: ]+):(?<line>[0-9]+):(?<column>[0-9]+)"###).unwrap()
    });

    remap_msg_by_regex(line, &RE)
}

impl Dsim {
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
                if line.starts_with("=F:") {
                    self.state = State::Fatal;
                    self.fatal(&remap_msg(line));
                } else if line.starts_with("=E:") {
                    self.state = State::Error;
                    self.error(&remap_msg(line));
                } else if line.starts_with("=W:") {
                    self.state = State::Warning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("=N") {
                    self.state = State::Info;
                    self.info(&remap_msg(line));
                }
            }
            State::Fatal => {
                if line.starts_with("=F:") || line.is_empty() {
                    self.state = State::Fatal;
                    self.fatal(&remap_msg(line));
                } else if line.starts_with("=E:") {
                    self.state = State::Error;
                    self.error(&remap_msg(line));
                } else if line.starts_with("=W:") {
                    self.state = State::Warning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("=N") {
                    self.state = State::Info;
                    self.info(&remap_msg(line));
                } else {
                    self.state = State::Idle;
                }
            }
            State::Error => {
                if line.starts_with("=F:") {
                    self.state = State::Fatal;
                    self.fatal(&remap_msg(line));
                } else if line.starts_with("=E:") || line.is_empty() {
                    self.state = State::Error;
                    self.error(&remap_msg(line));
                } else if line.starts_with("=W:") {
                    self.state = State::Warning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("=N") {
                    self.state = State::Info;
                    self.info(&remap_msg(line));
                } else {
                    self.state = State::Idle;
                }
            }
            State::Warning => {
                if line.starts_with("=F:") {
                    self.state = State::Fatal;
                    self.fatal(&remap_msg(line));
                } else if line.starts_with("=E:") {
                    self.state = State::Error;
                    self.error(&remap_msg(line));
                } else if line.starts_with("=W:") || line.is_empty() {
                    self.state = State::Warning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("=N") {
                    self.state = State::Info;
                    self.info(&remap_msg(line));
                } else {
                    self.state = State::Idle;
                }
            }
            State::Info => {
                if line.starts_with("=F:") {
                    self.state = State::Fatal;
                    self.fatal(&remap_msg(line));
                } else if line.starts_with("=E:") {
                    self.state = State::Error;
                    self.error(&remap_msg(line));
                } else if line.starts_with("=W:") {
                    self.state = State::Warning;
                    self.warning(&remap_msg(line));
                } else if line.starts_with("=N") || line.is_empty() {
                    self.state = State::Info;
                    self.info(&remap_msg(line));
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
        Ok(())
    }
}

impl Default for Dsim {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for Dsim {
    fn run(
        &mut self,
        metadata: &Metadata,
        test: StrId,
        _top: Option<StrId>,
        path: PathId,
        mut wave: bool,
    ) -> Result<bool> {
        self.success = true;

        let temp_dir = tempfile::tempdir().into_diagnostic()?;

        // in this case, the global includes may be irrelevant
        if !metadata.test.include_files.is_empty() {
            warn!("Including files is unimplemented for this backend!");
        }

        info!("Compiling test ({test})");

        let mut defines = vec![];
        defines.push(format!(
            "+define+__veryl_test_{}_{}__",
            metadata.project.name, test
        ));

        if wave {
            if WaveFormFormat::Vcd == metadata.test.waveform_format {
                defines.push(format!(
                    "+define+__veryl_wavedump_{}_{}__",
                    metadata.project.name, test
                ));
            } else {
                warn!("Only VCD is supported as a waveform format for DSim!");
                wave = false;
            }
        }

        let rt = Runtime::new().unwrap();

        rt.block_on(async {
            let compile = Command::new("dsim")
                .arg("-genimage")
                .arg(test.to_string())
                .arg("-sv2017")
                .arg("-f")
                .arg(metadata.filelist_path())
                .args(&defines)
                .args(&metadata.test.dsim.compile_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run \"dsim\"")?;
            self.parse(compile).await
        })?;

        if !self.success {
            error!("Failed compile ({test})");
            return Ok(false);
        }

        info!("Executing test ({test})");

        rt.block_on(async {
            let simulate = Command::new("dsim")
                .arg("-image")
                .arg(test.to_string())
                .args(&metadata.test.dsim.simulate_args)
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run simulation binary")?;
            self.parse(simulate).await
        })?;

        if wave {
            copy_wave(test, path, metadata, temp_dir.path())?;
        }

        if self.success {
            info!("Succeeded test ({test})");
            Ok(true)
        } else {
            error!("Failed test ({test})");
            Ok(false)
        }
    }

    fn name(&self) -> &'static str {
        "DSim"
    }

    fn failure(&mut self) {
        self.success = false;
    }
}
