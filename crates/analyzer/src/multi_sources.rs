#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    pub path: String,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
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
        for source in &self.sources {
            if span.offset() < start + source.text.len() {
                code = Some(&source.text);
                header = Some(&source.path);
                break;
            }
            start += source.text.len();
        }

        let code = code.unwrap();
        let header = header.unwrap();

        let local_span = &(span.offset() - start, span.len()).into();
        let local = code.read_span(local_span, context_lines_before, context_lines_after)?;

        let local_span = local.span();
        let span = (local_span.offset() + start, local_span.len()).into();

        Ok(Box::new(miette::MietteSpanContents::new_named(
            header.to_owned(),
            local.data(),
            span,
            local.line(),
            local.column(),
            local.line_count(),
        )))
    }
}
