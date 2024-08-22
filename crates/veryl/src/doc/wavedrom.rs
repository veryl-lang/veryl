// This file is based on:
//
// https://github.com/JasMoH/mdbook-wavedrom
// Released under the MPL-2.0 license

use crate::doc::utils::escape_html;
use mdbook::book::{Book, BookItem, Chapter};
use mdbook::errors::Result;
use mdbook::preprocess::{Preprocessor, PreprocessorContext};
use pulldown_cmark::{CodeBlockKind::*, Event, Options, Parser, Tag, TagEnd};

pub struct Wavedrom;

impl Preprocessor for Wavedrom {
    fn name(&self) -> &str {
        "wavedrom"
    }

    fn run(&self, _ctx: &PreprocessorContext, mut book: Book) -> Result<Book> {
        let mut res = None;
        book.for_each_mut(|item: &mut BookItem| {
            if let Some(Err(_)) = res {
                return;
            }

            if let BookItem::Chapter(ref mut chapter) = *item {
                res = Some(Wavedrom::add_wavedrom(chapter).map(|md| {
                    chapter.content = md;
                }));
            }
        });

        res.unwrap_or(Ok(())).map(|_| book)
    }

    fn supports_renderer(&self, renderer: &str) -> bool {
        renderer == "html"
    }
}

impl Wavedrom {
    fn add_wavedrom(chapter: &mut Chapter) -> Result<String> {
        let content = &chapter.content;
        let mut wavedrom_content = String::new();
        let mut in_wavedrom_block = false;

        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_FOOTNOTES);
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        opts.insert(Options::ENABLE_TASKLISTS);

        let mut code_span = 0..0;
        let mut start_new_code_span = true;

        let mut wavedrom_blocks = vec![];

        let events = Parser::new_ext(content, opts);
        for (e, span) in events.into_offset_iter() {
            if let Event::Start(Tag::CodeBlock(Fenced(code))) = e.clone() {
                if &*code == "wavedrom" {
                    in_wavedrom_block = true;
                    wavedrom_content.clear();
                }
                continue;
            }

            if !in_wavedrom_block {
                continue;
            }

            // We're in the code block. The text is what we want.
            // Code blocks can come in multiple text events.

            if let Event::Text(_) = e {
                if start_new_code_span {
                    code_span = span;
                    start_new_code_span = false;
                } else {
                    code_span = code_span.start..span.end;
                }

                continue;
            }

            if let Event::End(TagEnd::CodeBlock) = e {
                in_wavedrom_block = false;

                let wavedrom_content = &content[code_span.clone()];
                let wavedrom_content = escape_html(wavedrom_content);
                let wavedrom_content = wavedrom_content.replace("\r\n", "\n");
                let wavedrom_code = format!("<body onload=\"WaveDrom.ProcessAll()\">\n\n<script type=\"WaveDrom\">{}</script>\n\n", wavedrom_content);
                wavedrom_blocks.push((span, wavedrom_code));

                start_new_code_span = true;
            }
        }

        let mut content = content.to_string();
        for (span, block) in wavedrom_blocks.iter().rev() {
            let pre_content = &content[0..span.start];
            let post_content = &content[span.end..];

            content = format!("{}\n{}{}", pre_content, block, post_content);
        }
        Ok(content)
    }
}
