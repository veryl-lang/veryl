use anstyle::{AnsiColor, Style};
use log::{Level, debug, log_enabled};
use miette::{IntoDiagnostic, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use veryl_metadata::{Metadata, WaveFormTarget};
use veryl_parser::resource_table::{PathId, StrId};
use veryl_sourcemap::SourceMap;

mod cocotb;
mod vcs;
mod verilator;
mod vivado;
pub use cocotb::*;
pub use vcs::*;
pub use verilator::*;
pub use vivado::*;

pub trait Runner {
    fn run(
        &mut self,
        metadata: &Metadata,
        test: StrId,
        top: Option<StrId>,
        path: PathId,
        wave: bool,
    ) -> Result<bool>;

    fn name(&self) -> &'static str;

    fn failure(&mut self);

    fn debug(&self, line: &str) {
        if log_enabled!(Level::Debug) {
            debug!("{} : {}", self.name(), line);
        }
    }

    fn info(&self, line: &str) {
        static STYLE: Lazy<Style> =
            Lazy::new(|| Style::new().fg_color(Some(AnsiColor::Green.into())));
        if !log_enabled!(Level::Debug) {
            println!("{}{}{}", STYLE.render(), line, STYLE.render_reset());
        }
    }

    fn warning(&mut self, line: &str) {
        static STYLE: Lazy<Style> =
            Lazy::new(|| Style::new().fg_color(Some(AnsiColor::Yellow.into())));
        if !log_enabled!(Level::Debug) {
            println!("{}{}{}", STYLE.render(), line, STYLE.render_reset());
        }
    }

    fn error(&mut self, line: &str) {
        static STYLE: Lazy<Style> =
            Lazy::new(|| Style::new().fg_color(Some(AnsiColor::Red.into())));
        if !log_enabled!(Level::Debug) {
            println!("{}{}{}", STYLE.render(), line, STYLE.render_reset());
        }
        self.failure();
    }

    fn fatal(&mut self, line: &str) {
        static STYLE: Lazy<Style> =
            Lazy::new(|| Style::new().fg_color(Some(AnsiColor::Red.into())).bold());
        if !log_enabled!(Level::Debug) {
            println!("{}{}{}", STYLE.render(), line, STYLE.render_reset());
        }
        self.failure();
    }
}

pub fn remap_msg_by_regex(line: &str, re: &Regex) -> String {
    let mut ret = line.to_string();

    if let Some(caps) = re.captures(line) {
        let start = caps.get(0).unwrap().start();
        let path = caps.name("path").unwrap().as_str().to_string();
        let line = caps.name("line").unwrap().as_str().parse::<u32>().unwrap();
        let column = caps
            .name("column")
            .map(|x| x.as_str().parse::<u32>().unwrap());

        if let Ok(source_map) = SourceMap::from_src(&PathBuf::from(path))
            && let Some((path, line, column)) = source_map.lookup(line, column.unwrap_or(1))
        {
            ret.push_str(&format!(
                "\n{}^ from: {}:{}:{}",
                " ".repeat(start),
                path.to_string_lossy(),
                line,
                column
            ));
        }
    }

    ret
}

pub fn copy_wave(
    test_name: StrId,
    test_path: PathId,
    metadata: &Metadata,
    work_path: &Path,
) -> Result<()> {
    // The file always has a `.vcd` extension, because `$dumpfile` doesn't have the metadata information
    let wave_src_path = work_path.join(format!("{test_name}.vcd"));

    // but let's rename the target file to the correct extension, based on the selected format
    let target_name = format!(
        "{}.{}",
        test_name,
        metadata.test.waveform_format.extension()
    );

    let wave_dst_path = match &metadata.test.waveform_target {
        WaveFormTarget::Target => PathBuf::from(test_path.to_string())
            .parent()
            .unwrap()
            .join(target_name),
        WaveFormTarget::Directory { path } => path.join(target_name),
    };

    let wave_dst_dir = wave_dst_path.parent().unwrap();
    if !wave_dst_dir.exists() {
        fs::create_dir_all(wave_dst_dir).into_diagnostic()?;
    }

    fs::copy(wave_src_path, wave_dst_path).into_diagnostic()?;
    Ok(())
}
