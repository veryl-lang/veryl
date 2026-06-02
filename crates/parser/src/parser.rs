use crate::parser_error::ParserError;
use crate::resource_table;
use crate::text_table::{self, TextInfo};
use crate::veryl_grammar::VerylGrammar;
use crate::veryl_grammar_trait::Veryl;
use crate::veryl_parser::parse;
use anyhow::anyhow;
use std::path::Path;

#[derive(Debug)]
pub struct Parser {
    pub veryl: Veryl,
}

impl Parser {
    pub fn parse<T: AsRef<Path>>(input: &str, file: &T) -> Result<Self, ParserError> {
        let path = resource_table::insert_path(file.as_ref());
        let text = {
            let mut text = input.to_string();
            if !text.ends_with("\n") {
                text.push('\n');
            }
            TextInfo { path, text }
        };
        // Parse the newline-terminated copy, not the original `input`: the lexer's
        // line-comment regex needs a trailing newline, so a file ending in `// ...`
        // would otherwise fail to lex.
        let buf = text.text.clone();
        text_table::set_current_text(text);

        let mut grammar = VerylGrammar::new();
        parse(&buf, file, &mut grammar)?;

        let veryl = grammar.veryl.ok_or(anyhow!("parse failure"))?;

        Ok(Parser { veryl })
    }
}
