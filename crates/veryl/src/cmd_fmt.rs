use crate::OptFmt;
use console::{style, Style};
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use similar::{ChangeTag, TextDiff};
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use veryl_formatter::Formatter;
use veryl_metadata::Metadata;
use veryl_parser::Parser;

pub struct CmdFmt {
    opt: OptFmt,
}

impl CmdFmt {
    pub fn new(opt: OptFmt) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files)?;

        let mut all_pass = true;
        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;
            let mut formatter = Formatter::new(metadata);
            formatter.format(&parser.veryl);

            let pass = input.as_str() == formatter.as_str();

            if !pass {
                if self.opt.check {
                    print_diff(&path.src, input.as_str(), formatter.as_str());
                    all_pass = false;
                } else {
                    let mut file = OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&path.src)
                        .into_diagnostic()?;
                    file.write_all(formatter.as_str().as_bytes())
                        .into_diagnostic()?;
                    file.flush().into_diagnostic()?;
                    debug!("Overwritten file ({})", path.src.to_string_lossy());
                }
            }
        }

        Ok(all_pass)
    }
}

struct Line(Option<usize>);

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            None => write!(f, "    "),

            Some(idx) => write!(f, "{:<4}", idx + 1),
        }
    }
}

fn print_diff(file: &Path, org: &str, new: &str) {
    let diff = TextDiff::from_lines(org, new);

    println!("Diff in {}", file.to_string_lossy());

    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            println!("{:-^1$}", "-", 80);
        }
        for op in group {
            for change in diff.iter_inline_changes(op) {
                let (sign, s) = match change.tag() {
                    ChangeTag::Delete => ("-", Style::new().red()),
                    ChangeTag::Insert => ("+", Style::new().green()),
                    ChangeTag::Equal => (" ", Style::new().dim()),
                };
                print!(
                    "{}{} |{}",
                    style(Line(change.old_index())).dim(),
                    style(Line(change.new_index())).dim(),
                    s.apply_to(sign).bold(),
                );
                for (emphasized, value) in change.iter_strings_lossy() {
                    if emphasized {
                        print!("{}", s.apply_to(value).underlined().on_black());
                    } else {
                        print!("{}", s.apply_to(value));
                    }
                }
                if change.missing_newline() {
                    println!();
                }
            }
        }
    }
}
