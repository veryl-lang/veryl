use crate::allow_table;
use crate::analyzer_error::AnalyzerError;
use crate::symbol::SymbolKind;
use crate::symbol_table::{self, ResolveSymbol};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};
use veryl_parser::{ParolError, Stringifier};

pub struct CheckInstance<'a> {
    pub errors: Vec<AnalyzerError>,
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
    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut connected_ports = Vec::new();
            if let Some(ref x) = arg.inst_declaration_opt1 {
                if let Some(ref x) = x.inst_declaration_opt2 {
                    let items: Vec<InstPortItem> = x.inst_port_list.as_ref().into();
                    for item in items {
                        connected_ports.push(item.identifier.identifier_token.token.text);
                    }
                }
            }

            if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                let mut stringifier = Stringifier::new();
                stringifier.scoped_identifier(&arg.scoped_identifier);
                let name = stringifier.as_str();
                if let ResolveSymbol::Symbol(symbol) = symbol.found {
                    match symbol.kind {
                        SymbolKind::Module(ref x) => {
                            for port in &x.ports {
                                if !connected_ports.contains(&port.name)
                                    && !allow_table::contains("missing_port")
                                {
                                    let port = resource_table::get_str_value(port.name).unwrap();
                                    self.errors.push(AnalyzerError::missing_port(
                                        name,
                                        &port,
                                        self.text,
                                        &arg.identifier.identifier_token,
                                    ));
                                }
                            }
                            for port in &connected_ports {
                                if !x.ports.iter().any(|x| &x.name == port) {
                                    let port = resource_table::get_str_value(*port).unwrap();
                                    self.errors.push(AnalyzerError::unknown_port(
                                        name,
                                        &port,
                                        self.text,
                                        &arg.identifier.identifier_token,
                                    ));
                                }
                            }
                        }
                        SymbolKind::Interface(_) => (),
                        SymbolKind::SystemVerilog => (),
                        _ => {
                            self.errors.push(AnalyzerError::mismatch_type(
                                name,
                                "module or interface",
                                &symbol.kind.to_kind_name(),
                                self.text,
                                &arg.identifier.identifier_token,
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
