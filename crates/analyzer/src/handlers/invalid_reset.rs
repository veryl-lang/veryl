use crate::analyze_error::AnalyzeError;
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct InvalidReset<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> InvalidReset<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for InvalidReset<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for InvalidReset<'a> {
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            // Chcek first if_reset when reset signel exists
            let if_reset_required = if arg.always_ff_declaration_opt.is_some() {
                if let Some(x) = arg.always_ff_declaration_list.first() {
                    !matches!(&*x.statement, Statement::Statement2(_))
                } else {
                    true
                }
            } else {
                false
            };
            if if_reset_required {
                self.errors.push(AnalyzeError::if_reset_required(
                    self.text,
                    &arg.always_ff.always_ff_token,
                ));
            }

            // Chcek reset signal when if_reset exists
            let mut if_reset_exist = false;
            for x in &arg.always_ff_declaration_list {
                if let Statement::Statement2(_) = &*x.statement {
                    if_reset_exist = true;
                }
            }
            if if_reset_exist && arg.always_ff_declaration_opt.is_none() {
                self.errors.push(AnalyzeError::reset_signal_missing(
                    self.text,
                    &arg.always_ff.always_ff_token,
                ));
            }
        }
        Ok(())
    }
}
