use crate::analyzer_error::AnalyzerError;
use crate::attribute::AllowItem;
use crate::attribute::Attribute as Attr;
use crate::attribute_table;
use crate::evaluator::Evaluator;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};
use veryl_parser::{ParolError, Stringifier};

pub struct CheckInstance<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    evaluator: Evaluator,
}

impl<'a> CheckInstance<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            evaluator: Evaluator::new(),
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
            let mut connected_params = Vec::new();
            if let Some(ref x) = arg.inst_declaration_opt0 {
                if let Some(ref x) = x.inst_parameter.inst_parameter_opt {
                    let items: Vec<InstParameterItem> = x.inst_parameter_list.as_ref().into();
                    for item in items {
                        connected_params.push(item.identifier.identifier_token.token.text);
                        // match self.evaluator.inst_parameter_item(&item) {
                        //     crate::evaluator::Evaluated::Fixed { .. } => todo!(),
                        //     crate::evaluator::Evaluated::Variable { _ } |
                        //     crate::evaluator::Evaluated::Unknown => {
                        //         pass!()
                        //     }
                        // }
                    }
                }
            }

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
                match symbol.found.kind {
                    SymbolKind::Module(ref x) => {
                        for port in &x.ports {
                            if !connected_ports.contains(&port.name)
                                && !attribute_table::contains(
                                    &arg.inst.inst_token.token,
                                    Attr::Allow(AllowItem::MissingPort),
                                )
                            {
                                let port = resource_table::get_str_value(port.name).unwrap();
                                self.errors.push(AnalyzerError::missing_port(
                                    name,
                                    &port,
                                    self.text,
                                    &arg.identifier.as_ref().into(),
                                ));
                            }
                        }
                        for param in &connected_params {
                            if !x.parameters.iter().any(|x| &x.name == param) {
                                let param = resource_table::get_str_value(*param).unwrap();
                                self.errors.push(AnalyzerError::unknown_param(
                                    name,
                                    &param,
                                    self.text,
                                    &arg.identifier.as_ref().into(),
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
                                    &arg.identifier.as_ref().into(),
                                ));
                            }
                        }
                    }
                    SymbolKind::Interface(_) => (),
                    SymbolKind::SystemVerilog => (),
                    SymbolKind::GenericParameter(_) => (),
                    _ => {
                        self.errors.push(AnalyzerError::mismatch_type(
                            name,
                            "module or interface",
                            &symbol.found.kind.to_kind_name(),
                            self.text,
                            &arg.identifier.as_ref().into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
