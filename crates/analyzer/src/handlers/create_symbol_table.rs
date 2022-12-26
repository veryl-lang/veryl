use crate::analyze_error::AnalyzeError;
use crate::symbol_table::{Location, NameSpace, Symbol, SymbolKind, SymbolTable};
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
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

    fn insert_symbol(&mut self, token: &VerylToken, kind: SymbolKind) {
        let name = token.text();
        let loc: Location = token.location().into();

        let symbol = Symbol::new(name, kind, &self.name_space, &loc);
        if !self.table.insert(name, symbol) {
            self.errors
                .push(AnalyzeError::duplicated_identifier(name, self.text, &token));
        }
    }
}

impl<'a> Handler for CreateSymbolTable<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateSymbolTable<'a> {
    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Variable);
        }
        Ok(())
    }

    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Parameter);
        }
        Ok(())
    }

    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Parameter);
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Parameter);
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Variable);
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Function);

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
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Module);

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
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Interface);

                let name = arg.identifier.identifier_token.text();
                self.name_space.push(name)
            }
            HandlerPoint::After => self.name_space.pop(),
        }
        Ok(())
    }
}
