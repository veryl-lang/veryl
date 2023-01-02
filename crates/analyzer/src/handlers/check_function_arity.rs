use crate::analyze_error::AnalyzeError;
use crate::namespace_table;
use crate::symbol_table::{self, Name, SymbolKind};
use veryl_parser::global_table;
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

pub struct CheckFunctionArity<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckFunctionArity<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl<'a> Handler for CheckFunctionArity<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckFunctionArity<'a> {
    fn factor(&mut self, arg: &Factor) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            if let Factor::FactorOptScopedOrHierIdentifierFactorOpt0(x) = arg {
                // skip system function
                if x.factor_opt.is_some() {
                    return Ok(());
                }

                let name: Name = (&*x.scoped_or_hier_identifier).into();
                let namespace = namespace_table::get(
                    x.scoped_or_hier_identifier
                        .identifier
                        .identifier_token
                        .token
                        .id,
                )
                .unwrap();
                let symbol = symbol_table::get(&name, &namespace);

                let arity = if let Some(symbol) = symbol {
                    if let SymbolKind::Function { ref ports, .. } = symbol.kind {
                        Some(ports.len())
                    } else {
                        None
                    }
                } else {
                    None
                };

                let mut args = 0;
                if let Some(ref x) = x.factor_opt0 {
                    if let Some(ref x) = x.factor_opt1 {
                        args += 1;
                        args += x.function_call_arg.function_call_arg_list.len();
                    }
                }

                if let Some(arity) = arity {
                    if arity != args {
                        let name =
                            global_table::get_str_value(*name.as_slice().last().unwrap()).unwrap();
                        self.errors.push(AnalyzeError::mismatch_arity(
                            &name,
                            arity,
                            args,
                            self.text,
                            &x.scoped_or_hier_identifier.identifier.identifier_token,
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}
