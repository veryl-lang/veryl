use crate::analyze_error::AnalyzeError;
use crate::symbol_table::{HierarchicalName, NameSpace, SymbolKind, SymbolTable};
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

pub struct CheckModuleInstance<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    symbol_table: &'a SymbolTable,
    point: HandlerPoint,
    name_space: NameSpace,
}

impl<'a> CheckModuleInstance<'a> {
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

impl<'a> Handler for CheckModuleInstance<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckModuleInstance<'a> {
    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            if let LetDeclarationGroup::InstanceDeclaration(x) = &*arg.let_declaration_group {
                let x = &x.instance_declaration;
                let name = x.identifier.identifier_token.text().into();
                let name = &HierarchicalName { paths: vec![name] };

                let mut connected_ports = Vec::new();
                if let Some(ref x) = x.instance_declaration_opt1 {
                    if let Some(ref x) = x.instance_declaration_opt2 {
                        let x = &x.instance_port_list;
                        connected_ports.push(
                            x.instance_port_item
                                .identifier
                                .identifier_token
                                .text()
                                .to_string(),
                        );
                        for x in &x.instance_port_list_list {
                            connected_ports.push(
                                x.instance_port_item
                                    .identifier
                                    .identifier_token
                                    .text()
                                    .to_string(),
                            );
                        }
                    }
                }

                let symbol = self.symbol_table.get(name, &self.name_space);
                if let Some(symbol) = symbol {
                    if let SymbolKind::Module { ref ports, .. } = symbol.kind {
                        for port in ports {
                            if !connected_ports.contains(&port.name) {
                                self.errors.push(AnalyzeError::missing_port(
                                    name.paths.last().unwrap(),
                                    &port.name,
                                    self.text,
                                    &x.identifier.identifier_token,
                                ));
                            }
                        }
                        for port in &connected_ports {
                            if !ports.iter().any(|x| &x.name == port) {
                                self.errors.push(AnalyzeError::unknown_port(
                                    name.paths.last().unwrap(),
                                    port,
                                    self.text,
                                    &x.identifier.identifier_token,
                                ));
                            }
                        }
                    } else {
                        self.errors.push(AnalyzeError::mismatch_type(
                            name.paths.last().unwrap(),
                            "module",
                            &symbol.kind.to_string(),
                            self.text,
                            &x.identifier.identifier_token,
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
