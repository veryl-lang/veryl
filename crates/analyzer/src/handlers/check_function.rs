use crate::analyzer_error::AnalyzerError;
use crate::symbol::SymbolKind;
use crate::symbol_path::SymbolPath;
use crate::symbol_table;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckFunction {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

impl CheckFunction {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckFunction {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckFunction {
    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let IdentifierStatementGroup::FunctionCall(_) = &*arg.identifier_statement_group {
                // skip system function
                if matches!(
                    arg.expression_identifier
                        .scoped_identifier
                        .scoped_identifier_group
                        .as_ref(),
                    ScopedIdentifierGroup::DollarIdentifier(_)
                ) {
                    return Ok(());
                }

                if let Ok(symbol) = symbol_table::resolve(arg.expression_identifier.as_ref()) {
                    let function_symbol = match symbol.found.kind {
                        SymbolKind::Function(_) => symbol.found,
                        SymbolKind::ModportFunctionMember(x) => {
                            symbol_table::get(x.function).unwrap()
                        }
                        _ => return Ok(()),
                    };
                    if let SymbolKind::Function(x) = function_symbol.kind {
                        if x.ret.is_some() {
                            let name = format!(
                                "{}",
                                SymbolPath::from(arg.expression_identifier.as_ref())
                                    .as_slice()
                                    .last()
                                    .unwrap()
                            );
                            self.errors.push(AnalyzerError::unused_return(
                                &name,
                                &arg.expression_identifier.as_ref().into(),
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn identifier_factor(&mut self, arg: &IdentifierFactor) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // not function call
            if arg.identifier_factor_opt.is_none() {
                return Ok(());
            }
            // skip system function
            if matches!(
                arg.expression_identifier
                    .scoped_identifier
                    .scoped_identifier_group
                    .as_ref(),
                ScopedIdentifierGroup::DollarIdentifier(_)
            ) {
                return Ok(());
            }

            if let Ok(symbol) = symbol_table::resolve(arg.expression_identifier.as_ref()) {
                let arity = match symbol.found.kind {
                    SymbolKind::Function(x) => Some(x.ports.len()),
                    SymbolKind::ModportFunctionMember(x) => {
                        if let SymbolKind::Function(x) = symbol_table::get(x.function).unwrap().kind
                        {
                            Some(x.ports.len())
                        } else {
                            unreachable!();
                        }
                    }
                    _ => None,
                };

                let mut args = 0;
                if let Some(ref x) = arg.identifier_factor_opt {
                    if let IdentifierFactorOptGroup::FunctionCall(x) =
                        x.identifier_factor_opt_group.as_ref()
                    {
                        if let Some(ref x) = x.function_call.function_call_opt {
                            let list: Vec<_> = x.argument_list.as_ref().into();

                            let positional_only =
                                list.iter().all(|x| x.argument_item_opt.is_none());
                            let named_only = list.iter().all(|x| x.argument_item_opt.is_some());

                            if !positional_only && !named_only {
                                self.errors.push(AnalyzerError::mixed_function_argument(
                                    &arg.expression_identifier.as_ref().into(),
                                ));
                            }

                            args += list.len();
                        }
                    }
                }

                if let Some(arity) = arity {
                    if arity != args {
                        let name = format!(
                            "{}",
                            SymbolPath::from(arg.expression_identifier.as_ref())
                                .as_slice()
                                .last()
                                .unwrap()
                        );
                        self.errors.push(AnalyzerError::mismatch_function_arity(
                            &name,
                            arity,
                            args,
                            &arg.expression_identifier.as_ref().into(),
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}
