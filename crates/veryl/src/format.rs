use crate::utils;
use crate::Fmt;
use console::{style, Style};
use miette::{IntoDiagnostic, Result, WrapErr};
use similar::{ChangeTag, TextDiff};
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
use veryl_formatter::formatter::Formatter;
use veryl_parser::veryl_grammar::VerylGrammar;
use veryl_parser::veryl_parser::parse;

pub fn format(opt: &Fmt) -> Result<bool> {
    let files = if opt.files.is_empty() {
        utils::gather_files("./")?
    } else {
        opt.files.clone()
    };

    let mut all_pass = true;
    let now = Instant::now();

    for file in &files {
        print(
            &format!("[Info] Processing file: {}", file.to_string_lossy()),
            opt,
        );

        let input = fs::read_to_string(file).into_diagnostic().wrap_err("")?;
        let mut veryl_grammar = VerylGrammar::new();
        parse(&input, file, &mut veryl_grammar)
            .wrap_err(format!("Failed parsing file {}", file.to_string_lossy()))?;
        let mut formatter = Formatter::new();

        if let Some(ref veryl) = veryl_grammar.veryl {
            formatter.format(veryl);

            let pass = input.as_str() == formatter.as_str();

            if !pass {
                if opt.check {
                    print_diff(file, input.as_str(), formatter.as_str());
                    all_pass = false;
                } else {
                    print(
                        &format!("[Info] Overwrite file: {}", file.to_string_lossy()),
                        opt,
                    );
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
    }

    let elapsed_time = now.elapsed();
    print(
        &format!(
            "[Info] Elapsed time: {} milliseconds.",
            elapsed_time.as_millis()
        ),
        opt,
    );

    Ok(all_pass)
}

fn print(msg: &str, opt: &Fmt) {
    if opt.verbose {
        println!("{}", msg);
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
