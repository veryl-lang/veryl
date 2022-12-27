use crate::analyze_error::AnalyzeError;
use crate::symbol_table::{HierarchicalName, NameSpace, SymbolKind, SymbolTable};
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

pub struct CheckFunctionArity<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    symbol_table: &'a SymbolTable,
    point: HandlerPoint,
    name_space: NameSpace,
}

impl<'a> CheckFunctionArity<'a> {
    pub fn new(text: &'a str, symbol_table: &'a SymbolTable) -> Self {
        Self {
            errors: Vec::new(),
            text,
            symbol_table,
            point: HandlerPoint::Before,
            name_space: NameSpace::default(),
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
            if let Factor::FactorOptHierarchicalIdentifierFactorOpt0(x) = arg {
                // skip system function
                if x.factor_opt.is_some() {
                    return Ok(());
                }

                let hierarchical_name: HierarchicalName = (&*x.hierarchical_identifier).into();
                let symbol = self.symbol_table.get(&hierarchical_name, &self.name_space);

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
                        self.errors.push(AnalyzeError::mismatch_arity(
                            hierarchical_name.paths.last().unwrap(),
                            arity,
                            args,
                            self.text,
                            &x.hierarchical_identifier.identifier.identifier_token,
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.text();
                self.name_space.push(name)
            }
            HandlerPoint::After => self.name_space.pop(),
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.text();
                self.name_space.push(name)
            }
            HandlerPoint::After => self.name_space.pop(),
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.text();
                self.name_space.push(name)
            }
            HandlerPoint::After => self.name_space.pop(),
        }
        Ok(())
    }
}
