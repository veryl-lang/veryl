use crate::analyzer_error::AnalyzerError;
use crate::symbol::{Direction, Port, Symbol, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::is_anonymous_text;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckAnonymous {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    inst_ports: Vec<Port>,
    inst_sv_module: bool,
    is_anonymous_identifier: bool,
    port_direction: Option<Direction>,
}

impl CheckAnonymous {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckAnonymous {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn get_inst_ports(symbol: &Symbol, ports: &mut Vec<Port>) {
    match &symbol.kind {
        SymbolKind::Module(x) => ports.extend(x.ports.clone()),
        SymbolKind::ProtoModule(x) => ports.extend(x.ports.clone()),
        SymbolKind::AliasModule(x) => {
            if let Ok(symbol) = symbol_table::resolve((&x.target.generic_path(), &symbol.namespace))
            {
                get_inst_ports(&symbol.found, ports);
            }
        }
        SymbolKind::GenericParameter(x) => {
            if let Some(proto) = x.bound.resolve_proto_bound(&symbol.namespace) {
                if let Some(symbol) = proto.get_symbol() {
                    get_inst_ports(&symbol, ports);
                }
            }
        }
        SymbolKind::GenericInstance(x) => {
            if let Some(symbol) = symbol_table::get(x.base) {
                get_inst_ports(&symbol, ports);
            }
        }
        _ => {}
    }
}

impl VerylGrammarTrait for CheckAnonymous {
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !self.is_anonymous_identifier {
                let ident = arg.identifier().token;
                if is_anonymous_text(ident.text) {
                    let path: GenericSymbolPath = arg.into();
                    self.errors
                        .push(AnalyzerError::anonymous_identifier_usage(&path.range));
                }
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                    match symbol.found.kind {
                        SymbolKind::SystemVerilog => self.inst_sv_module = true,
                        _ => get_inst_ports(&symbol.found, &mut self.inst_ports),
                    }
                }
            }
            HandlerPoint::After => {
                self.inst_ports.clear();
                self.inst_sv_module = false;
            }
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Some(ref x) = arg.inst_port_item_opt {
                    if let Some(port) = self
                        .inst_ports
                        .iter()
                        .find(|x| x.name() == arg.identifier.identifier_token.token.text)
                    {
                        if let SymbolKind::Port(port) = symbol_table::get(port.symbol).unwrap().kind
                        {
                            self.is_anonymous_identifier = port.direction == Direction::Output
                                && x.expression.is_anonymous_expression();
                        }
                    } else if self.inst_sv_module {
                        // For SV module, any ports can be connected with anonymous identifier
                        self.is_anonymous_identifier = x.expression.is_anonymous_expression();
                    }
                }
            }
            HandlerPoint::After => self.is_anonymous_identifier = false,
        }
        Ok(())
    }

    fn port_type_concrete(&mut self, arg: &PortTypeConcrete) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if arg.port_type_concrete_opt0.is_some() {
                    self.port_direction = Some(arg.direction.as_ref().into());
                }
            }
            HandlerPoint::After => self.port_direction = None,
        }
        Ok(())
    }

    fn port_default_value(&mut self, arg: &PortDefaultValue) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.is_anonymous_identifier =
                    matches!(self.port_direction.unwrap(), Direction::Output)
                        && arg.expression.is_anonymous_expression();
            }
            _ => self.is_anonymous_identifier = false,
        }
        Ok(())
    }

    fn import_declaration(&mut self, arg: &ImportDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                let is_wildcard = arg.import_declaration_opt.is_some();
                let is_valid_import = if matches!(symbol.found.kind, SymbolKind::SystemVerilog) {
                    true
                } else if is_wildcard {
                    symbol.found.is_package(false)
                } else {
                    let package_symbol = symbol
                        .full_path
                        .get(symbol.full_path.len() - 2)
                        .map(|x| symbol_table::get(*x).unwrap())
                        .unwrap();
                    // The preceding symbol must be a package or
                    // a proto-package referenced through a generic parameter.
                    package_symbol.is_package(false) && symbol.found.is_importable(true)
                };

                if !is_valid_import {
                    self.errors.push(AnalyzerError::invalid_import(
                        &arg.scoped_identifier.as_ref().into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
