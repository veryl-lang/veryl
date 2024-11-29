use crate::analyzer_error::AnalyzerError;
use crate::unsafe_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckUnsafe<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckUnsafe<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl Handler for CheckUnsafe<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckUnsafe<'_> {
    fn unsafe_block(&mut self, arg: &UnsafeBlock) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let value: Result<crate::r#unsafe::Unsafe, crate::r#unsafe::UnsafeError> =
                    arg.try_into();

                match value {
                    Ok(value) => {
                        unsafe_table::begin(arg.r#unsafe.unsafe_token.token, Some(value));
                    }
                    Err(err) => {
                        unsafe_table::begin(arg.r#unsafe.unsafe_token.token, None);
                        match err {
                            crate::r#unsafe::UnsafeError::UnknownUnsafe => {
                                self.errors.push(AnalyzerError::unknown_unsafe(
                                    &arg.identifier.identifier_token.to_string(),
                                    self.text,
                                    &arg.identifier.as_ref().into(),
                                ));
                            }
                        }
                    }
                }
            }
            HandlerPoint::After => {
                unsafe_table::end(arg.r_brace.r_brace_token.token);
            }
        }
        Ok(())
    }
}
