use crate::utils;
use crate::OptFmt;
use console::{style, Style};
use similar::{ChangeTag, TextDiff};
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
use veryl_formatter::Formatter;
use veryl_parser::miette::{IntoDiagnostic, Result, WrapErr};
use veryl_parser::Parser;

pub struct CmdFmt {
    opt: OptFmt,
}

impl CmdFmt {
    pub fn new(opt: OptFmt) -> Self {
        Self { opt }
    }

    pub fn exec(&self) -> Result<bool> {
        let files = if self.opt.files.is_empty() {
            utils::gather_files("./")?
        } else {
            self.opt.files.clone()
        };

        let mut all_pass = true;
        let now = Instant::now();

        for file in &files {
            self.print(&format!(
                "[Info] Processing file: {}",
                file.to_string_lossy()
            ));

            let input = fs::read_to_string(file).into_diagnostic().wrap_err("")?;
            let parser = Parser::parse(&input, file)?;
            let mut formatter = Formatter::new();
            formatter.format(&parser.veryl);

            let pass = input.as_str() == formatter.as_str();

            if !pass {
                if self.opt.check {
                    print_diff(file, input.as_str(), formatter.as_str());
                    all_pass = false;
                } else {
                    self.print(&format!(
                        "[Info] Overwrite file: {}",
                        file.to_string_lossy()
                    ));
                    let mut file = OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(file)
                        .into_diagnostic()?;
                    file.write_all(formatter.as_str().as_bytes())
                        .into_diagnostic()?;
                    file.flush().into_diagnostic()?;
                }
            }
        }

        let elapsed_time = now.elapsed();
        self.print(&format!(
            "[Info] Elapsed time: {} milliseconds.",
            elapsed_time.as_millis()
        ));

        Ok(all_pass)
    }

    fn print(&self, msg: &str) {
        if self.opt.verbose {
            println!("{}", msg);
        }
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
