use crate::analyzer_error::AnalyzerError;
use crate::evaluator::{Evaluated, Evaluator};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckEnum<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    enum_variants: usize,
    enum_member_values: Vec<(Evaluated, VerylToken)>,
}

impl<'a> CheckEnum<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            enum_variants: 0,
            enum_member_values: Vec::new(),
        }
    }
}

impl<'a> Handler for CheckEnum<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckEnum<'a> {
    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.enum_variants = 0;
                self.enum_member_values.clear();
            }
            HandlerPoint::After => {
                let mut evaluator = Evaluator::new();
                let r#type: crate::symbol::Type = arg.r#type.as_ref().into();
                let width = evaluator.type_width(r#type);
                if let Some(width) = width {
                    let max_members = 2_usize.pow(width as u32);
                    if self.enum_variants > max_members {
                        let name = arg.identifier.identifier_token.text();
                        self.errors.push(AnalyzerError::enum_variant_too_much(
                            &name,
                            self.enum_variants,
                            width,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }

                    for (enum_value, token) in &self.enum_member_values {
                        if let Evaluated::Fixed { value, .. } = enum_value {
                            if *value as usize >= max_members {
                                self.errors.push(AnalyzerError::enum_variant_too_large(
                                    &token.text(),
                                    *value,
                                    width,
                                    self.text,
                                    token,
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn enum_item(&mut self, arg: &EnumItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.enum_variants += 1;
            if let Some(ref x) = arg.enum_item_opt {
                let token = arg.identifier.identifier_token.clone();
                let mut evaluator = Evaluator::new();
                let evaluated = evaluator.expression(&x.expression);
                self.enum_member_values.push((evaluated, token));
            }
        }
        Ok(())
    }
}
