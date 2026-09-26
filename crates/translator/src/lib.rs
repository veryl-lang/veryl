pub mod convert;
pub mod writer;

use miette::{Diagnostic, NamedSource, SourceSpan};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use sv_parser::{Defines, Locate, parse_sv_pp, preprocess_str};
use thiserror::Error;
use veryl_metadata::NewlineStyle;

#[derive(Debug, Error)]
pub enum TranslateError {
    #[error("SystemVerilog parse error: {0}")]
    Parse(String),
}

/// A SystemVerilog construct that has no Veryl equivalent (yet). Rendered by
/// miette as a graphical warning that underlines the offending source span and
/// shows the reason as the `help` line — mirroring how `veryl-analyzer` emits
/// its diagnostics.
#[derive(Debug, Clone, Error, Diagnostic)]
#[error("unsupported {kind}")]
#[diagnostic(severity(Warning), code(translate::unsupported))]
pub struct UnsupportedConstruct {
    pub kind: String,
    #[help]
    pub reason: String,
    /// Shared across all constructs from the same file so the source text is
    /// stored once rather than cloned per diagnostic.
    #[source_code]
    pub src: Arc<NamedSource<String>>,
    #[label("cannot be translated to Veryl")]
    pub span: SourceSpan,
}

pub struct TranslateOutput {
    pub veryl: String,
    pub unsupported: Vec<UnsupportedConstruct>,
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
    // The syntax tree's offsets index the preprocessed text, which is not the
    // input: directives are dropped, macros expanded and whitespace adjusted.
    // Node text is taken from that text, and diagnostic spans are mapped back
    // to the input.
    let (pp, pp_defines) = preprocess_str(
        src,
        path.as_ref(),
        &defines,
        &include_paths,
        false,
        false,
        0,
        0,
    )
    .map_err(|e| TranslateError::Parse(format!("{e:?}")))?;
    let pp_src = pp.text().to_string();
    let (tree, _) =
        parse_sv_pp(pp, pp_defines, false).map_err(|e| TranslateError::Parse(format!("{e:?}")))?;

    let newline = newline_style.newline_str(src);
    let conv = convert::Converter::new(&tree, &pp_src, newline);
    let (raw, reports) = conv.run();

    let name = path.as_ref().to_string_lossy().into_owned();
    // No `with_language` hint for now: miette's syntect highlighter is not
    // enabled and ships no SystemVerilog grammar, so a language tag would be a
    // no-op. Syntax highlighting is left for a follow-up.
    let shared_src = Arc::new(NamedSource::new(name, src.to_string()));
    let unsupported = reports
        .into_iter()
        .map(|r| UnsupportedConstruct {
            kind: r.kind,
            reason: r.reason,
            src: Arc::clone(&shared_src),
            span: origin_span(&tree, r.offset, r.len, src.len()).into(),
        })
        .collect();

    let veryl = if format { format_veryl(&raw) } else { raw };
    Ok(TranslateOutput { veryl, unsupported })
}

/// Map a span of the preprocessed text back to the input, keeping its length
/// but clamping it to the input so a span from macro-expanded text stays valid.
fn origin_span(
    tree: &sv_parser::SyntaxTree,
    offset: usize,
    len: usize,
    src_len: usize,
) -> (usize, usize) {
    let locate = Locate {
        offset,
        line: 0,
        len,
    };
    let offset = tree
        .get_origin(&locate)
        .map_or(offset, |(_, origin)| origin)
        .min(src_len);
    (offset, len.min(src_len - offset))
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
