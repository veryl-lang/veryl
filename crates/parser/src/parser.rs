use crate::parser_error::ParserError;
use crate::veryl_grammar::VerylGrammar;
use crate::veryl_grammar_trait::Veryl;
use crate::veryl_parser::parse;
use std::path::Path;

#[derive(Debug)]
pub struct Parser {
    pub veryl: Veryl,
}

impl Parser {
    pub fn parse<T: AsRef<Path>>(input: &str, file: &T) -> Result<Self, ParserError> {
        let mut grammar = VerylGrammar::new();
        parse(input, file, &mut grammar)?;

        let veryl = grammar.veryl.unwrap();

        Ok(Parser { veryl })
    }
}
