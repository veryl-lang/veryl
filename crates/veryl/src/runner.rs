use anstyle::{AnsiColor, Style};
use log::{debug, log_enabled, Level};
use miette::Result;
use once_cell::sync::Lazy;
use veryl_metadata::Metadata;
use veryl_parser::resource_table::StrId;

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
