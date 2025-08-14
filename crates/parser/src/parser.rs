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
        let text = TextInfo {
            path,
            text: input.to_string(),
        };
        text_table::set_current_text(text);

        let mut grammar = VerylGrammar::new();
        parse(input, file, &mut grammar)?;

        let veryl = grammar.veryl.ok_or(anyhow!("parse failure"))?;

        Ok(Parser { veryl })
    }
}
