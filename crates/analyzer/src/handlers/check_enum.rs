use crate::analyzer_error::AnalyzerError;
use crate::evaluator::Evaluator;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckEnum {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

impl CheckEnum {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckEnum {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn calc_width(value: usize) -> usize {
    (usize::BITS - value.leading_zeros()) as usize
}

impl VerylGrammarTrait for CheckEnum {
    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let enum_symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
            if let SymbolKind::Enum(r#enum) = enum_symbol.found.kind {
                if let Some(r#type) = r#enum.r#type {
                    if let Some(width) = Evaluator::new().type_width(r#type) {
                        if width.len() != 1 {
                            unimplemented!();
                        } else {
                            let variants = r#enum.members.len();
                            if calc_width(variants - 1) > width[0] {
                                let name = arg.identifier.identifier_token.to_string();
                                self.errors.push(AnalyzerError::too_much_enum_variant(
                                    &name,
                                    variants,
                                    width[0],
                                    &arg.identifier.as_ref().into(),
                                ));
                            }

                            for id in r#enum.members {
                                let member_symbol = symbol_table::get(id).unwrap();
                                if let SymbolKind::EnumMember(member) = member_symbol.kind {
                                    let member_value = member.value.value().unwrap_or(0);
                                    if calc_width(member_value) > width[0] {
                                        self.errors.push(AnalyzerError::too_large_enum_variant(
                                            &member_symbol.token.to_string(),
                                            member_value as isize,
                                            width[0],
                                            &member_symbol.token.into(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
