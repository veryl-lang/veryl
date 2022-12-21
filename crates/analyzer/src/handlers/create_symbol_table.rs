use crate::analyze_error::AnalyzeError;
use crate::symbol_table::{Location, NameSpace, Symbol, SymbolKind, SymbolTable};
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CreateSymbolTable<'a> {
    pub errors: Vec<AnalyzeError>,
    pub table: SymbolTable,
    text: &'a str,
    point: HandlerPoint,
    name_space: NameSpace,
}

impl<'a> CreateSymbolTable<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CreateSymbolTable<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateSymbolTable<'a> {
    fn variable_declaration(&mut self, arg: &VariableDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let name = arg.identifier.identifier_token.text();
            let loc: Location = arg.identifier.identifier_token.location().into();

            let symbol = Symbol::new(&name, SymbolKind::Variable, &self.name_space, &loc);
            if !self.table.insert(&name, symbol) {
                self.errors.push(AnalyzeError::duplicated_identifier(
                    name,
                    self.text,
                    &arg.identifier.identifier_token,
                ));
            }
        }
        Ok(())
    }

    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let name = arg.identifier.identifier_token.text();
            let loc: Location = arg.identifier.identifier_token.location().into();

            let symbol = Symbol::new(&name, SymbolKind::Parameter, &self.name_space, &loc);
            if !self.table.insert(&name, symbol) {
                self.errors.push(AnalyzeError::duplicated_identifier(
                    name,
                    self.text,
                    &arg.identifier.identifier_token,
                ));
            }
        }
        Ok(())
    }

    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let name = arg.identifier.identifier_token.text();
            let loc: Location = arg.identifier.identifier_token.location().into();

            let symbol = Symbol::new(&name, SymbolKind::Parameter, &self.name_space, &loc);
            if !self.table.insert(&name, symbol) {
                self.errors.push(AnalyzeError::duplicated_identifier(
                    name,
                    self.text,
                    &arg.identifier.identifier_token,
                ));
            }
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let name = arg.identifier.identifier_token.text();
            let loc: Location = arg.identifier.identifier_token.location().into();

            let symbol = Symbol::new(&name, SymbolKind::Parameter, &self.name_space, &loc);
            if !self.table.insert(&name, symbol) {
                self.errors.push(AnalyzeError::duplicated_identifier(
                    name,
                    self.text,
                    &arg.identifier.identifier_token,
                ));
            }
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let name = arg.identifier.identifier_token.text();
            let loc: Location = arg.identifier.identifier_token.location().into();

            let symbol = Symbol::new(&name, SymbolKind::Variable, &self.name_space, &loc);
            if !self.table.insert(&name, symbol) {
                self.errors.push(AnalyzeError::duplicated_identifier(
                    name,
                    self.text,
                    &arg.identifier.identifier_token,
                ));
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.text();
                let loc: Location = arg.identifier.identifier_token.location().into();

                let symbol = Symbol::new(&name, SymbolKind::Function, &self.name_space, &loc);
                if !self.table.insert(&name, symbol) {
                    self.errors.push(AnalyzeError::duplicated_identifier(
                        name,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }

                self.name_space.push(&name)
            }
            HandlerPoint::After => self.name_space.pop(),
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.text();
                let loc: Location = arg.identifier.identifier_token.location().into();

                let symbol = Symbol::new(&name, SymbolKind::Module, &self.name_space, &loc);
                if !self.table.insert(&name, symbol) {
                    self.errors.push(AnalyzeError::duplicated_identifier(
                        name,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }

                self.name_space.push(&name)
            }
            HandlerPoint::After => self.name_space.pop(),
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.text();
                let loc: Location = arg.identifier.identifier_token.location().into();

                let symbol = Symbol::new(&name, SymbolKind::Interface, &self.name_space, &loc);
                if !self.table.insert(&name, symbol) {
                    self.errors.push(AnalyzeError::duplicated_identifier(
                        name,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }

                self.name_space.push(&name)
            }
            HandlerPoint::After => self.name_space.pop(),
        }
        Ok(())
    }
}
