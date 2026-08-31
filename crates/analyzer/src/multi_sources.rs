use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    pub text: String,
    #[serde(default)]
    pub line_offset: usize,
}

impl Source {
    pub fn new(path: String, text: String) -> Self {
        Self {
            path,
            text,
            line_offset: 0,
        }
    }

    /// Keep a bounded excerpt around `span` while retaining the one-based
    /// `span_line` it had in the complete source buffer.
    pub(crate) fn excerpt(
        path: String,
        text: &str,
        span: &miette::SourceSpan,
        span_line: usize,
        context_lines: usize,
    ) -> Option<(Self, miette::SourceSpan)> {
        let span_start = span.offset();
        let span_end = span_start.checked_add(span.len())?;
        if span_end > text.len() {
            return None;
        }

        let mut excerpt_start = text[..span_start]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        let mut preceding_context = 0;
        for _ in 0..context_lines {
            if excerpt_start == 0 {
                break;
            }
            excerpt_start = text[..excerpt_start - 1]
                .rfind('\n')
                .map_or(0, |newline| newline + 1);
            preceding_context += 1;
        }

        let mut excerpt_end = span_end;
        for _ in 0..=context_lines {
            excerpt_end = text[excerpt_end..]
                .find('\n')
                .map_or(text.len(), |newline| excerpt_end + newline + 1);
            if excerpt_end == text.len() {
                break;
            }
        }

        let line_offset = span_line.saturating_sub(preceding_context + 1);
        let local_span = (span_start - excerpt_start, span.len()).into();
        Some((
            Self {
                path,
                text: text[excerpt_start..excerpt_end].to_string(),
                line_offset,
            },
            local_span,
        ))
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct MultiSources {
    pub sources: Vec<Source>,
}

impl miette::SourceCode for MultiSources {
    fn read_span<'a>(
        &'a self,
        span: &miette::SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> Result<Box<dyn miette::SpanContents<'a> + 'a>, miette::MietteError> {
        let mut start = 0;
        let mut code = None;
        let mut header = None;
        let mut line_offset = 0;
        for source in &self.sources {
            if span.offset() < start + source.text.len() {
                code = Some(source.text.as_str());
                header = Some(&source.path);
                line_offset = source.line_offset;
                break;
            }
            start += source.text.len();
        }

        let code = code.ok_or(miette::MietteError::OutOfBounds)?;
        let header = header.ok_or(miette::MietteError::OutOfBounds)?;

        let local_span = &(span.offset() - start, span.len()).into();
        let local = code.read_span(local_span, context_lines_before, context_lines_after)?;

        let local_span = local.span();
        let span = (local_span.offset() + start, local_span.len()).into();

        Ok(Box::new(miette::MietteSpanContents::new_named(
            header.to_owned(),
            local.data(),
            span,
            local.line() + line_offset,
            local.column(),
            local.line_count(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miette::SourceCode;

    #[test]
    fn excerpt_preserves_the_original_line_number() {
        let text = "zero\none\ntwo\nthree\nfour\n";
        let original_span = (9, 3).into();
        let (source, local_span) =
            Source::excerpt("sample.veryl".into(), text, &original_span, 3, 1).unwrap();
        assert_eq!(source.text, "one\ntwo\nthree\n");

        let input = MultiSources {
            sources: vec![source],
        };
        let contents = input.read_span(&local_span, 0, 0).unwrap();
        assert_eq!(contents.line(), 2);
        assert_eq!(contents.column(), 0);
        assert_eq!(contents.data(), b"two");
    }
}
