pub mod convert;
pub mod writer;

use std::collections::HashMap;
use std::path::Path;
use sv_parser::{Defines, parse_sv_str};
use thiserror::Error;
use veryl_metadata::NewlineStyle;

pub use convert::UnsupportedReport;

#[derive(Debug, Error)]
pub enum TranslateError {
    #[error("SystemVerilog parse error: {0}")]
    Parse(String),
}

pub struct TranslateOutput {
    pub veryl: String,
    pub unsupported: Vec<UnsupportedReport>,
}

/// Translate SystemVerilog source to Veryl source. If `format` is true the
/// output is parsed and run through `veryl-formatter` for pretty-printing;
/// formatting failures fall back to the raw output. The `newline_style`
/// argument controls the line ending of the produced text and mirrors the
/// `format.newline_style` setting in `Veryl.toml`.
pub fn translate_str(
    src: &str,
    path: impl AsRef<Path>,
    format: bool,
    newline_style: NewlineStyle,
) -> Result<TranslateOutput, TranslateError> {
    let defines: Defines<std::collections::hash_map::RandomState> = HashMap::new();
    let include_paths: Vec<std::path::PathBuf> = Vec::new();
    let (tree, _) = parse_sv_str(src, path.as_ref(), &defines, &include_paths, false, false)
        .map_err(|e| TranslateError::Parse(format!("{e:?}")))?;

    let newline = newline_style.newline_str(src);
    let conv = convert::Converter::new(&tree, src, newline);
    let (raw, unsupported) = conv.run();

    let veryl = if format { format_veryl(&raw) } else { raw };
    Ok(TranslateOutput { veryl, unsupported })
}

/// Best-effort: parse + format the generated Veryl text. If parsing fails,
/// return the input untouched so the user still gets a (less polished) result.
fn format_veryl(text: &str) -> String {
    let dummy_path = std::path::PathBuf::from("translated.veryl");
    let parser = match veryl_parser::Parser::parse(text, &dummy_path) {
        Ok(p) => p,
        Err(_) => return text.to_string(),
    };
    let metadata = match veryl_metadata::Metadata::create_default("translated") {
        Ok(m) => m,
        Err(_) => return text.to_string(),
    };
    let mut formatter = veryl_formatter::Formatter::new(&metadata);
    formatter.format(&parser.veryl, text);
    formatter.as_str().to_string()
}
