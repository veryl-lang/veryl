use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckReset<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    in_always_ff: bool,
    in_if_reset: bool,
    if_reset_brace: usize,
    if_reset_exist: bool,
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
    fn l_brace(&mut self, _arg: &LBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_reset {
                self.if_reset_brace += 1;
            }
        }
        Ok(())
    }

    fn r_brace(&mut self, _arg: &RBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_reset {
                self.if_reset_brace -= 1;
                if self.if_reset_brace == 0 {
                    self.in_if_reset = false;
                }
            }
        }
        Ok(())
    }

    fn if_reset(&mut self, _arg: &IfReset) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.if_reset_exist = true;
            self.in_if_reset = true;
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
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
                    self.errors.push(AnalyzerError::missing_if_reset(
                        self.text,
                        &arg.always_ff.always_ff_token.token,
                    ));
                }

                self.in_always_ff = true;
            }
            HandlerPoint::After => {
                // Check reset signal when if_reset exists
                if self.if_reset_exist && arg.always_ff_declaration_opt.is_none() {
                    self.errors.push(AnalyzerError::missing_reset_signal(
                        self.text,
                        &arg.always_ff.always_ff_token.token,
                    ));
                }

                self.in_always_ff = false;
                self.if_reset_exist = false;
            }
        }
        Ok(())
    }
}
