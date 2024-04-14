use crate::analyzer_error::AnalyzerError;
use crate::evaluator::{Evaluated, Evaluator};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckEnum<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    enum_variants: usize,
    enum_member_values: Vec<(Evaluated, Token)>,
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

fn calc_width(value: usize) -> usize {
    (usize::BITS - value.leading_zeros()) as usize
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
                let r#type: crate::symbol::Type = arg.scalar_type.as_ref().into();
                let width = evaluator.type_width(r#type);
                if let Some(width) = width {
                    if calc_width(self.enum_variants - 1) > width {
                        let name = arg.identifier.identifier_token.to_string();
                        self.errors.push(AnalyzerError::too_much_enum_variant(
                            &name,
                            self.enum_variants,
                            width,
                            self.text,
                            &arg.identifier.as_ref().into(),
                        ));
                    }

                    for (enum_value, token) in &self.enum_member_values {
                        if let Evaluated::Fixed { value, .. } = enum_value {
                            if calc_width(*value as usize) > width {
                                self.errors.push(AnalyzerError::too_large_enum_variant(
                                    &token.to_string(),
                                    *value,
                                    width,
                                    self.text,
                                    &token.into(),
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
                let token = arg.identifier.identifier_token.token;
                let mut evaluator = Evaluator::new();
                let evaluated = evaluator.expression(&x.expression);
                self.enum_member_values.push((evaluated, token));
            }
        }
        Ok(())
    }
}
