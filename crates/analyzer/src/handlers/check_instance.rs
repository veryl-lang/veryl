use crate::analyze_error::AnalyzeError;
use crate::namespace_table;
use crate::symbol::SymbolKind;
use crate::symbol_table::{self, SymbolPath};
use veryl_parser::miette::Result;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

pub struct CheckInstance<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckInstance<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl<'a> Handler for CheckInstance<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckInstance<'a> {
    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let path = &SymbolPath::new(&[arg.identifier0.identifier_token.token.text]);

            let mut connected_ports = Vec::new();
            if let Some(ref x) = arg.inst_declaration_opt1 {
                if let Some(ref x) = x.inst_declaration_opt2 {
                    let x = &x.inst_port_list;
                    connected_ports.push(x.inst_port_item.identifier.identifier_token.token.text);
                    for x in &x.inst_port_list_list {
                        connected_ports
                            .push(x.inst_port_item.identifier.identifier_token.token.text);
                    }
                }
            }

            let namespace = namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
            let symbol = symbol_table::get(path, &namespace);
            if let Some(symbol) = symbol {
                match symbol.kind {
                    SymbolKind::Module(ref x) => {
                        for port in &x.ports {
                            if !connected_ports.contains(&port.name) {
                                let name =
                                    resource_table::get_str_value(*path.as_slice().last().unwrap())
                                        .unwrap();
                                let port = resource_table::get_str_value(port.name).unwrap();
                                self.errors.push(AnalyzeError::missing_port(
                                    &name,
                                    &port,
                                    self.text,
                                    &arg.identifier.identifier_token,
                                ));
                            }
                        }
                        for port in &connected_ports {
                            if !x.ports.iter().any(|x| &x.name == port) {
                                let name =
                                    resource_table::get_str_value(*path.as_slice().last().unwrap())
                                        .unwrap();
                                let port = resource_table::get_str_value(*port).unwrap();
                                self.errors.push(AnalyzeError::unknown_port(
                                    &name,
                                    &port,
                                    self.text,
                                    &arg.identifier.identifier_token,
                                ));
                            }
                        }
                    }
                    SymbolKind::Interface(_) => (),
                    _ => {
                        let name = resource_table::get_str_value(*path.as_slice().last().unwrap())
                            .unwrap();
                        self.errors.push(AnalyzeError::mismatch_type(
                            &name,
                            "module or interface",
                            &symbol.kind.to_kind_name(),
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
