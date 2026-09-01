use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    pub text: String,
    #[serde(default)]
    pub line_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SourceExcerpt {
    start: usize,
    end: usize,
    line_offset: usize,
}

impl SourceExcerpt {
    pub(crate) fn whole(text: &str) -> Self {
        Self {
            start: 0,
            end: text.len(),
            line_offset: 0,
        }
    }

    pub(crate) fn local_span(self, span: &miette::SourceSpan) -> Option<miette::SourceSpan> {
        let span_end = span.offset().checked_add(span.len())?;
        if span.offset() < self.start || span_end > self.end {
            return None;
        }
        Some((span.offset() - self.start, span.len()).into())
    }
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
    #[cfg(test)]
    pub(crate) fn excerpt(
        path: String,
        text: &str,
        span: &miette::SourceSpan,
        span_line: usize,
        context_lines: usize,
    ) -> Option<(Self, miette::SourceSpan)> {
        let excerpt = Self::excerpt_window(text, span, span_line, context_lines)?;
        let local_span = excerpt.local_span(span)?;
        Some((Self::from_excerpt(path, text, excerpt)?, local_span))
    }

    pub(crate) fn excerpt_window(
        text: &str,
        span: &miette::SourceSpan,
        span_line: usize,
        context_lines: usize,
    ) -> Option<SourceExcerpt> {
        let span_start = span.offset();
        let span_end = span_start.checked_add(span.len())?;
        if span_end > text.len()
            || !text.is_char_boundary(span_start)
            || !text.is_char_boundary(span_end)
        {
            return None;
        }

        let bytes = text.as_bytes();
        let mut excerpt_start = line_start(bytes, span_start);
        let mut preceding_context = 0;
        for _ in 0..context_lines {
            if excerpt_start == 0 {
                break;
            }
            excerpt_start = previous_line_start(bytes, excerpt_start);
            preceding_context += 1;
        }

        let mut excerpt_end = span_end;
        for _ in 0..=context_lines {
            excerpt_end = next_line_end(bytes, excerpt_end);
            if excerpt_end == text.len() {
                break;
            }
        }

        let line_offset = span_line.saturating_sub(preceding_context + 1);
        Some(SourceExcerpt {
            start: excerpt_start,
            end: excerpt_end,
            line_offset,
        })
    }

    pub(crate) fn from_excerpt(path: String, text: &str, excerpt: SourceExcerpt) -> Option<Self> {
        Some(Self {
            path,
            text: text.get(excerpt.start..excerpt.end)?.to_string(),
            line_offset: excerpt.line_offset,
        })
    }
}

fn line_start(text: &[u8], position: usize) -> usize {
    text[..position]
        .iter()
        .rposition(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(0, |newline| newline + 1)
}

fn previous_line_start(text: &[u8], current_start: usize) -> usize {
    let previous_terminator = if current_start >= 2
        && text[current_start - 2] == b'\r'
        && text[current_start - 1] == b'\n'
    {
        current_start - 2
    } else {
        current_start - 1
    };
    line_start(text, previous_terminator)
}

fn next_line_end(text: &[u8], position: usize) -> usize {
    let Some(relative) = text[position..]
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
    else {
        return text.len();
    };
    let terminator = position + relative;
    if text[terminator] == b'\r' && text.get(terminator + 1) == Some(&b'\n') {
        terminator + 2
    } else {
        terminator + 1
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
        let mut start = 0usize;
        let mut source = None;
        for candidate in &self.sources {
            let end = start
                .checked_add(candidate.text.len())
                .ok_or(miette::MietteError::OutOfBounds)?;
            if span.offset() < end {
                source = Some((candidate, end));
                break;
            }
            start = end;
        }
        let (source, end) = source.ok_or(miette::MietteError::OutOfBounds)?;
        let span_end = span
            .offset()
            .checked_add(span.len())
            .ok_or(miette::MietteError::OutOfBounds)?;
        if span_end > end {
            return Err(miette::MietteError::OutOfBounds);
        }

        let local_span = &(span.offset() - start, span.len()).into();
        let local = source
            .text
            .read_span(local_span, context_lines_before, context_lines_after)?;

        let local_span = local.span();
        let global_offset = local_span
            .offset()
            .checked_add(start)
            .ok_or(miette::MietteError::OutOfBounds)?;
        let span = (global_offset, local_span.len()).into();
        let line = local
            .line()
            .checked_add(source.line_offset)
            .ok_or(miette::MietteError::OutOfBounds)?;

        Ok(Box::new(miette::MietteSpanContents::new_named(
            source.path.clone(),
            local.data(),
            span,
            line,
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

    #[test]
    fn excerpts_on_the_same_long_line_share_a_window() {
        let text = "before\na = b; b = c; c = a;\nafter\n";
        let first_span = (7, 6).into();
        let last_span = (21, 6).into();

        let first = Source::excerpt_window(text, &first_span, 2, 1).unwrap();
        let last = Source::excerpt_window(text, &last_span, 2, 1).unwrap();

        assert_eq!(first, last);
        assert_eq!(first.local_span(&first_span).unwrap().offset(), 7);
        assert_eq!(last.local_span(&last_span).unwrap().offset(), 21);
    }

    #[test]
    fn excerpt_handles_bare_carriage_return_lines() {
        let text = "zero\rone\rtwo\rthree\rfour\r";
        let original_span = (9, 3).into();
        let (source, local_span) =
            Source::excerpt("sample.veryl".into(), text, &original_span, 3, 1).unwrap();
        assert_eq!(source.text, "one\rtwo\rthree\r");

        let input = MultiSources {
            sources: vec![source],
        };
        let contents = input.read_span(&local_span, 0, 0).unwrap();
        assert_eq!(contents.line(), 2);
        assert_eq!(contents.data(), b"two");
    }

    #[test]
    fn excerpt_handles_crlf_around_utf8_text() {
        let text = "zero\r\none\r\n二\r\nthree\r\nfour\r\n";
        let start = text.find('二').unwrap();
        let original_span = (start, '二'.len_utf8()).into();
        let (source, local_span) =
            Source::excerpt("sample.veryl".into(), text, &original_span, 3, 1).unwrap();
        assert_eq!(source.text, "one\r\n二\r\nthree\r\n");

        let input = MultiSources {
            sources: vec![source],
        };
        let contents = input.read_span(&local_span, 0, 0).unwrap();
        assert_eq!(contents.line(), 2);
        assert_eq!(contents.data(), "二".as_bytes());
    }

    #[test]
    fn span_lookup_handles_empty_and_nonempty_sources() {
        let input = MultiSources {
            sources: vec![
                Source::new("empty".into(), String::new()),
                Source::new("first".into(), "abc".into()),
                Source::new("also-empty".into(), String::new()),
                Source::new("second".into(), "def".into()),
            ],
        };

        let first = input.read_span(&(1, 1).into(), 0, 0).unwrap();
        assert_eq!(first.name(), Some("first"));
        assert_eq!(first.data(), b"b");
        let second = input.read_span(&(4, 1).into(), 0, 0).unwrap();
        assert_eq!(second.name(), Some("second"));
        assert_eq!(second.data(), b"e");
    }

    #[test]
    fn source_serialization_shape_is_unchanged() {
        #[derive(Serialize)]
        struct SerializedSource<'a> {
            path: &'a str,
            text: &'a str,
            line_offset: usize,
        }

        let source = Source::new("sample.veryl".into(), "abc".into());
        let actual = postcard::to_allocvec(&source).unwrap();
        let expected = postcard::to_allocvec(&SerializedSource {
            path: &source.path,
            text: &source.text,
            line_offset: source.line_offset,
        })
        .unwrap();
        assert_eq!(actual, expected);
    }
}
