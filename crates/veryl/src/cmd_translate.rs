use crate::OptTranslate;
use log::{info, warn};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::path::{Path, PathBuf};
use veryl_metadata::{Metadata, NewlineStyle};

pub struct CmdTranslate {
    opt: OptTranslate,
}

impl CmdTranslate {
    pub fn new(opt: OptTranslate) -> Self {
        Self { opt }
    }

    pub fn exec(&self) -> Result<bool> {
        if self.opt.files.is_empty() {
            return Err(miette::miette!(
                "no input files; pass one or more `.sv` paths"
            ));
        }

        let newline_style = match Metadata::search_from_current() {
            Ok(p) => Metadata::load(p)
                .map(|m| m.format.newline_style)
                .unwrap_or(NewlineStyle::Auto),
            Err(_) => NewlineStyle::Auto,
        };

        let mut all_pass = true;
        for input in &self.opt.files {
            all_pass &= self.translate_one(input, newline_style)?;
        }
        Ok(all_pass)
    }

    fn translate_one(&self, input: &Path, newline_style: NewlineStyle) -> Result<bool> {
        info!("Translating ({})", input.to_string_lossy());

        let src = fs::read_to_string(input)
            .into_diagnostic()
            .wrap_err("failed to read input")?;

        let out = veryl_translator::translate_str(&src, input, !self.opt.no_format, newline_style)
            .map_err(|e| miette::miette!("{e}"))?;

        if !out.unsupported.is_empty() {
            for r in &out.unsupported {
                warn!("unsupported {} at line {}", r.kind, r.line);
            }
            warn!(
                "{}: {} unsupported construct(s)",
                input.to_string_lossy(),
                out.unsupported.len()
            );
        }

        if self.opt.stdout {
            print!("{}", out.veryl);
        } else {
            let dest = sibling_veryl_path(input)?;
            fs::write(&dest, out.veryl.as_bytes())
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to write {}", dest.to_string_lossy()))?;
            info!("Wrote ({})", dest.to_string_lossy());
        }

        if self.opt.strict && !out.unsupported.is_empty() {
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

fn sibling_veryl_path(input: &Path) -> Result<PathBuf> {
    let mut dest = input.to_path_buf();
    if !dest.set_extension("veryl") {
        return Err(miette::miette!(
            "cannot derive output path for {}",
            input.to_string_lossy()
        ));
    }
    Ok(dest)
}
