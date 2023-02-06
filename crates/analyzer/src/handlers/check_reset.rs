use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckReset<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckReset<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckReset<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckReset<'a> {
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // Check first if_reset when reset signel exists
            let if_reset_required = if arg.always_ff_declaration_opt.is_some() {
                if let Some(x) = arg.always_ff_declaration_list.first() {
                    !matches!(&*x.statement, Statement::IfResetStatement(_))
                } else {
                    true
                }
            } else {
                false
            };
            if if_reset_required {
                self.errors.push(AnalyzerError::if_reset_required(
                    self.text,
                    &arg.always_ff.always_ff_token,
                ));
            }

            // Check reset signal when if_reset exists
            let mut if_reset_exist = false;
            for x in &arg.always_ff_declaration_list {
                if let Statement::IfResetStatement(_) = &*x.statement {
                    if_reset_exist = true;
                }
            }
            if if_reset_exist && arg.always_ff_declaration_opt.is_none() {
                self.errors.push(AnalyzerError::reset_signal_missing(
                    self.text,
                    &arg.always_ff.always_ff_token,
                ));
            }
        }
        Ok(())
    }
}
