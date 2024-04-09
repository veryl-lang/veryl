use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckEmbed<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckEmbed<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl<'a> Handler for CheckEmbed<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckEmbed<'a> {
    fn embed_declaration(&mut self, arg: &EmbedDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let way = arg.identifier.identifier_token.to_string();
            let lang = arg.identifier0.identifier_token.to_string();

            if !EMBED_WAY.contains(&way.as_str()) {
                self.errors.push(AnalyzerError::unknown_embed_way(
                    &way,
                    self.text,
                    &arg.identifier.identifier_token.token,
                ));
            }

            if !EMBED_LANG.contains(&lang.as_str()) {
                self.errors.push(AnalyzerError::unknown_embed_lang(
                    &lang,
                    self.text,
                    &arg.identifier0.identifier_token.token,
                ));
            }
        }
        Ok(())
    }
}

const EMBED_WAY: [&str; 1] = ["inline"];
const EMBED_LANG: [&str; 1] = ["sv"];
