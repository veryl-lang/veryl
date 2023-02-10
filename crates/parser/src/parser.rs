use crate::parser_error::ParserError;
use crate::resource_table;
use crate::veryl_grammar::VerylGrammar;
use crate::veryl_grammar_trait::Veryl;
use crate::veryl_parser::parse;
use std::path::Path;

#[derive(Debug)]
pub struct Parser {
    pub veryl: Veryl,
}

impl Parser {
    #[allow(clippy::result_large_err)]
    pub fn parse<T: AsRef<Path>>(input: &str, file: &T) -> Result<Self, ParserError> {
        // Inserting PathId because it will not be inserted if input doesn't have token.
        let _ = resource_table::insert_path(file.as_ref());

        let mut grammar = VerylGrammar::new();
        parse(input, file, &mut grammar)?;

        let veryl = grammar.veryl.unwrap();

        Ok(Parser { veryl })
    }
}
