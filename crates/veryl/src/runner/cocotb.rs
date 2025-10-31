use crate::runner::{Runner, copy_wave};
use futures::prelude::*;
use log::{error, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::runtime::Runtime;
use tokio_util::codec::{FramedRead, LinesCodec};
use veryl_metadata::{Metadata, WaveFormFormat};
use veryl_parser::resource_table::{self, PathId, StrId};
use veryl_parser::veryl_grammar_trait::{self as syntax_tree};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Info,
    Warning,
    Error,
    Fatal,
}

pub enum CocotbSource {
    Embed(Box<syntax_tree::EmbedContent>),
    Include(StrId),
}

pub struct Cocotb {
    source: CocotbSource,
    state: State,
    success: bool,
}

impl Cocotb {
    pub fn new(source: CocotbSource) -> Self {
        Self {
            source,
            state: State::Idle,
            success: true,
        }
    }

    pub fn runner(self) -> Box<dyn Runner> {
        Box::new(self) as Box<dyn Runner>
    }

    fn parse_line(&mut self, line: &str, force_error: bool) {
        self.debug(line);

        if force_error {
            self.error(line);
        } else {
            if !line.starts_with("                ") {
                self.state = State::Idle;
            }

            match self.state {
                State::Idle => {
                    if line.ends_with("failed") {
                        self.error(line);
                        self.state = State::Error;
                    } else if line.starts_with("     0.00ns INFO") {
                        self.info(line);
                        self.state = State::Info;
                    } else if line.starts_with("     0.00ns WARNING") {
                        self.warning(line);
                        self.state = State::Warning;
                    } else if line.starts_with("     0.00ns ERROR") {
                        self.error(line);
                        self.state = State::Error;
                    } else if line.starts_with("     0.00ns CRITICAL") {
                        self.fatal(line);
                        self.state = State::Fatal;
                    }
                }
                State::Info => {
                    self.info(line);
                }
                State::Warning => {
                    self.warning(line);
                }
                State::Error => {
                    self.error(line);
                }
                State::Fatal => {
                    self.fatal(line);
                }
            }
        }
    }

    async fn parse(&mut self, mut child: Child) -> Result<()> {
        let stdout = child.stdout.take().unwrap();
        let mut reader = FramedRead::new(stdout, LinesCodec::new());
        while let Some(line) = reader.next().await {
            let line = line.into_diagnostic()?;
            self.parse_line(&line, false);
        }

        let ecode = child.wait().await.into_diagnostic()?;
        if !ecode.success() {
            let stderr = child.stderr.take().unwrap();
            let mut reader = FramedRead::new(stderr, LinesCodec::new());
            while let Some(line) = reader.next().await {
                let line = line.into_diagnostic()?;
                self.parse_line(&line, true);
            }
            error!("cocotb failed by error code {ecode}");
            self.failure();
        }
        Ok(())
    }
}

impl Runner for Cocotb {
    fn run(
        &mut self,
        metadata: &Metadata,
        test: StrId,
        top: Option<StrId>,
        path: PathId,
        wave: bool,
    ) -> Result<bool> {
        self.success = true;

        let temp_dir = tempfile::tempdir().into_diagnostic()?;

        info!("Executing test ({test})");

        let src_path = temp_dir.path().join(format!("{test}.py"));

        for include_file in &metadata.test.include_files {
            if include_file.is_dir() {
                miette::bail!("Including directories currently unsupported");
            } else if let Some(file_name) = include_file.iter().next_back() {
                let sim_build_path = temp_dir.path().join("sim_build");
                fs::create_dir_all(&sim_build_path)
                    .into_diagnostic()
                    .with_context(|| {
                        format!("Failed to create `sim_build` directory at {sim_build_path:?}")
                    })?;

                let target_path = sim_build_path.join(file_name);
                fs::copy(include_file, &target_path)
                    .into_diagnostic()
                    .with_context(|| {
                        format!("Failed to copy include {include_file:?} to {target_path:?}")
                    })?;
            } else {
                miette::bail!("Failed to get include file name {:?}", include_file);
            }
        }

        match self.source {
            CocotbSource::Embed(ref x) => {
                let src_text = process_embed_content(x);
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&src_path)
                    .into_diagnostic()?;
                file.write_all(src_text.as_bytes()).into_diagnostic()?;
                file.flush().into_diagnostic()?;
            }
            CocotbSource::Include(x) => {
                let include_path = resource_table::get_path_value(path).unwrap();
                let include_path = include_path
                    .parent()
                    .unwrap()
                    .join(x.to_string().trim_matches('"'));
                fs::copy(include_path, src_path).into_diagnostic()?;
            }
        }

        let file_list = fs::read_to_string(metadata.filelist_path()).into_diagnostic()?;
        let mut sources = String::new();
        for line in file_list.lines() {
            sources.push_str(&format!("\"{line}\","));
        }
        sources = format!("[{}]", sources.strip_suffix(',').unwrap());

        let module = format!("{}_{}", metadata.project.name, top.unwrap());

        let (py_waves, args) = match wave.then_some(metadata.test.waveform_format) {
            Some(WaveFormFormat::Vcd) => ("True", "['--trace']"),
            Some(WaveFormFormat::Fst) => (
                "True",
                "['--trace-fst', '--trace-structs', '--trace-threads', '2']",
            ),
            None => ("False", "[]"),
        };
        let runner_path = temp_dir.path().join("runner.py");
        let runner_text = format!(
            r#"
try:
    from importlib.metadata import version
except ImportError:
    try:
        from importlib_metadata import version
    except ImportError as e:
        raise RuntimeError("Use Python 3.8+ or install importlib_metadata")

cocotb_version = version("cocotb")
if cocotb_version.startswith("2."):
    import cocotb_tools
    import cocotb_tools.runner

    sources = {sources}

    runner = cocotb_tools.runner.get_runner("verilator")
    runner.build(
        sources=sources,
        hdl_toplevel="{module}",
        always=True,
        waves={py_waves},
        build_args={args},
    )

    runner.test(
        hdl_toplevel="{module}",
        test_module="{test},",
        waves={py_waves},
    )
elif cocotb_version.startswith("1.9."):
    import cocotb.runner

    sources = {sources}

    runner = cocotb.runner.get_runner("verilator")
    runner.build(
        verilog_sources=sources,
        hdl_toplevel="{module}",
        always=True,
        waves={py_waves},
        build_args={args},
    )

    runner.test(
        hdl_toplevel="{module}",
        test_module="{test},",
        waves={py_waves},
    )
else:
    raise RuntimeError("unsupported cocotb version")
"#
        );

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&runner_path)
            .into_diagnostic()?;
        file.write_all(runner_text.as_bytes()).into_diagnostic()?;
        file.flush().into_diagnostic()?;

        let rt = Runtime::new().unwrap();

        rt.block_on(async {
            let compile = Command::new("python3")
                .arg("runner.py")
                .current_dir(temp_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .into_diagnostic()
                .wrap_err("Failed to run \"python3\"")?;

            self.parse(compile).await
        })?;

        if wave {
            // `copy_wave` expects the waveform at a certain position and format
            fs::copy(
                temp_dir
                    .path()
                    .join("sim_build")
                    .join("dump")
                    .with_extension(metadata.test.waveform_format.extension()),
                temp_dir.path().join(test.to_string()).with_extension("vcd"),
            )
            .into_diagnostic()?;
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
        "Cocotb"
    }

    fn failure(&mut self) {
        self.success = false;
    }
}

fn process_embed_content(embed_content: &syntax_tree::EmbedContent) -> String {
    embed_content
        .embed_content_list
        .iter()
        .map(|x| process_embed_item(&x.embed_item))
        .collect::<Vec<_>>()
        .join("")
}

fn process_embed_item(embed_item: &syntax_tree::EmbedItem) -> String {
    match embed_item {
        syntax_tree::EmbedItem::EmbedLBraceEmbedItemListEmbedRBrace(x) => {
            let mut ret = String::new();
            ret.push_str(&x.embed_l_brace.embed_l_brace_token.to_string());
            for x in &x.embed_item_list {
                ret.push_str(&process_embed_item(&x.embed_item));
            }
            ret.push_str(&x.embed_r_brace.embed_r_brace_token.to_string());
            ret
        }
        syntax_tree::EmbedItem::Any(x) => x.any.any_token.to_string(),
        _ => unreachable!(),
    }
}
