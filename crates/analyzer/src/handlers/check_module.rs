use crate::analyzer_error::AnalyzerError;
use crate::symbol::{Symbol, SymbolId};
use crate::symbol_table::{self, ResolveSymbol};
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;
use veryl_parser::{resource_table, veryl_grammar_trait::*};

pub struct CheckModule<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    inputs: Vec<(SymbolId, u32)>,
    block_record: Vec<SymbolId>,
    in_if_for_depth: u32,
    in_module: bool,
}

impl<'a> CheckModule<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            inputs: vec![],
            block_record: vec![],
            in_if_for_depth: 0,
            in_module: false,
        }
    }

    fn assign_check_inputs(&mut self, symb: &Symbol, token: &VerylToken) {
        if self
            .inputs
            .binary_search_by(|probe| probe.0.cmp(&symb.id))
            .is_ok()
        {
            let identifier = resource_table::get_str_value(symb.token.text).unwrap();
            self.errors.push(AnalyzerError::assignment_to_input(
                self.text,
                &identifier,
                token,
            ));
        }
    }
}

impl<'a> Handler for CheckModule<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckModule<'a> {
    fn module_declaration(&mut self, _arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_module = true;
            }
            HandlerPoint::After => {
                self.in_module = false;
            }
        }
        Ok(())
    }

    fn always_comb_declaration(&mut self, _arg: &AlwaysCombDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.block_record.clear();
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, _arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.block_record.clear();
        }
        Ok(())
    }

    fn function_declaration(&mut self, _arg: &FunctionDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.block_record.clear();
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if self.in_module {
            if let HandlerPoint::Before = self.point {
                if let IdentifierStatementGroup::Assignment(_) = &*arg.identifier_statement_group {
                    let symb = match symbol_table::resolve(arg.expression_identifier.as_ref()) {
                        Ok(x) => match x.found {
                            ResolveSymbol::Symbol(x) => x,
                            // External symbol can't be checkd
                            ResolveSymbol::External => return Ok(()),
                        },
                        Err(_) => panic!(),
                    };
                    let token = &arg.expression_identifier.identifier.identifier_token;
                    self.assign_check_inputs(&symb, token);
                }
            }
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if !self.in_module {
            return Ok(());
        }
        if let HandlerPoint::Before = self.point {
            match &*arg.port_declaration_item_group {
                PortDeclarationItemGroup::DirectionArrayType(x) => {
                    let id = match symbol_table::resolve(arg.identifier.as_ref()) {
                        Ok(x) => match x.found {
                            ResolveSymbol::Symbol(x) => x.id,
                            // External symbol can't be checkd
                            ResolveSymbol::External => return Ok(()),
                        },
                        Err(_) => panic!(),
                    };
                    if let Direction::Input(_) = &*x.direction {
                        self.inputs.push((id, 0));
                    }
                    // TODO: Cover Inout, Ref, Outputs, and Modports for checking
                }
                // TODO: Support Interface output checking
                PortDeclarationItemGroup::InterfacePortDeclarationItemOpt(_) => {}
            }
        }
        Ok(())
    }

    fn assign_declaration(&mut self, arg: &AssignDeclaration) -> Result<(), ParolError> {
        if self.in_module {
            if let HandlerPoint::Before = self.point {
                let symb = match symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
                    Ok(x) => match x.found {
                        ResolveSymbol::Symbol(x) => x,
                        // External symbol can't be checkd
                        ResolveSymbol::External => return Ok(()),
                    },
                    Err(_) => panic!(),
                };
                let token = &arg.hierarchical_identifier.identifier.identifier_token;
                self.assign_check_inputs(&symb, token);
            }
        }
        Ok(())
    }

    fn module_if_declaration(&mut self, _arg: &ModuleIfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_if_for_depth += 1,
            HandlerPoint::After => self.in_if_for_depth -= 1,
        }
        Ok(())
    }

    fn module_for_declaration(&mut self, _arg: &ModuleForDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_if_for_depth += 1,
            HandlerPoint::After => self.in_if_for_depth -= 1,
        }
        Ok(())
    }

    fn port_declaration_list(&mut self, _arg: &PortDeclarationList) -> Result<(), ParolError> {
        if self.in_module {
            if let HandlerPoint::After = self.point {
                self.inputs.sort();
            }
        }
        Ok(())
    }
}
