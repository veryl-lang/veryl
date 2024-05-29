use anstyle::{AnsiColor, Style};
use log::{debug, log_enabled, Level};
use miette::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::PathBuf;
use veryl_metadata::Metadata;
use veryl_parser::resource_table::StrId;
use veryl_sourcemap::SourceMap;

mod vcs;
mod verilator;
mod vivado;
pub use vcs::*;
pub use verilator::*;
pub use vivado::*;

pub trait Runner {
    fn run(&mut self, metadata: &Metadata, test: StrId) -> Result<bool>;

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

        if let Ok(source_map) = SourceMap::from_src(&PathBuf::from(path)) {
            if let Some((path, line, column)) = source_map.lookup(line, column.unwrap_or(1)) {
                ret.push_str(&format!(
                    "\n{}^ from: {}:{}:{}",
                    " ".repeat(start),
                    path.to_string_lossy(),
                    line,
                    column
                ));
            }
        }
    }

    ret
}
