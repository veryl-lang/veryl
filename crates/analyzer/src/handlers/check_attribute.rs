use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckAttribute<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckAttribute<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl<'a> Handler for CheckAttribute<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckAttribute<'a> {
    fn attribute(&mut self, arg: &Attribute) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let identifier = arg.identifier.identifier_token.text();
            match identifier.as_str() {
                "ifdef" | "ifndef" => {
                    let valid_arg = if let Some(ref x) = arg.attribute_opt {
                        let args: Vec<AttributeItem> = x.attribute_list.as_ref().into();
                        if args.len() != 1 {
                            false
                        } else {
                            matches!(args[0], AttributeItem::Identifier(_))
                        }
                    } else {
                        false
                    };

                    if !valid_arg {
                        self.errors.push(AnalyzerError::mismatch_attribute_args(
                            &identifier,
                            "single identifier",
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
                "sv" => {
                    let valid_arg = if let Some(ref x) = arg.attribute_opt {
                        let args: Vec<AttributeItem> = x.attribute_list.as_ref().into();
                        if args.len() != 1 {
                            false
                        } else {
                            matches!(args[0], AttributeItem::StringLiteral(_))
                        }
                    } else {
                        false
                    };

                    if !valid_arg {
                        self.errors.push(AnalyzerError::mismatch_attribute_args(
                            &identifier,
                            "single string",
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
                _ => {
                    self.errors.push(AnalyzerError::unknown_attribute(
                        &identifier,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }
            }
        }
        Ok(())

        //self.hash(&arg.hash);
        //self.l_bracket(&arg.l_bracket);
        //self.identifier(&arg.identifier);
        //if let Some(ref x) = arg.attribute_opt {
        //    self.l_paren(&x.l_paren);
        //    self.attribute_list(&x.attribute_list);
        //    self.r_paren(&x.r_paren);
        //}
        //self.r_bracket(&arg.r_bracket);
    }
}
