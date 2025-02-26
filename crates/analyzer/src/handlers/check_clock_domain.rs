use crate::analyzer_error::AnalyzerError;
use crate::r#unsafe::Unsafe;
use crate::symbol::{ClockDomain, GenericBoundKind, Port, Symbol, SymbolId, SymbolKind};
use crate::symbol_table;
use crate::unsafe_table;
use std::collections::HashMap;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenRange};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckClockDomain {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    expr_clock_domains: Vec<(ClockDomain, TokenRange)>,
    inst_clock_domains: HashMap<StrId, (ClockDomain, TokenRange)>,
    always_ff_clock_domain: Option<(ClockDomain, TokenRange)>,
    default_clock: Option<SymbolId>,
}

impl CheckClockDomain {
    pub fn new() -> Self {
        Self::default()
    }

    fn push_expr_clock_domain(&mut self, kind: &SymbolKind, range: TokenRange) {
        match kind {
            SymbolKind::Variable(x) => {
                self.expr_clock_domains.push((x.clock_domain, range));
            }
            SymbolKind::Port(x) => {
                self.expr_clock_domains.push((x.clock_domain, range));
            }
            SymbolKind::Instance(x) => {
                self.expr_clock_domains.push((x.clock_domain, range));
            }
            _ => (),
        }
    }

    fn check_expr_clock_domains(&mut self, token: &Token) -> ClockDomain {
        let mut prev: Option<(ClockDomain, TokenRange)> = self.always_ff_clock_domain;
        for curr in &self.expr_clock_domains {
            if let Some(prev) = prev {
                if !curr.0.compatible(&prev.0) && !unsafe_table::contains(token, Unsafe::Cdc) {
                    self.errors.push(AnalyzerError::mismatch_clock_domain(
                        &curr.0.to_string(),
                        &prev.0.to_string(),
                        &curr.1,
                        &prev.1,
                    ));
                }
            }

            prev = Some(*curr);
        }
        prev.map(|(x, _)| x).unwrap_or(ClockDomain::None)
    }

    fn check_cdc_on_port_connections(&mut self, ports: &Vec<Port>, unsafe_token: &Token) {
        let mut connection_table = HashMap::<ClockDomain, (ClockDomain, TokenRange)>::new();

        for port in ports {
            if let Some(connected) = self.inst_clock_domains.get(&port.name()) {
                let port_domain = port.property().clock_domain;
                if let Some(assigned) = connection_table.get(&port_domain) {
                    if !assigned.0.compatible(&connected.0)
                        && !unsafe_table::contains(unsafe_token, Unsafe::Cdc)
                    {
                        self.errors.push(AnalyzerError::mismatch_clock_domain(
                            &connected.0.to_string(),
                            &assigned.0.to_string(),
                            &connected.1,
                            &assigned.1,
                        ));
                    }
                } else {
                    connection_table.insert(port_domain, *connected);
                }
            }
        }
    }
}

impl Handler for CheckClockDomain {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckClockDomain {
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(symbol) = symbol_table::resolve(arg) {
                self.push_expr_clock_domain(&symbol.found.kind, arg.into());
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.expr_clock_domains.clear(),
            HandlerPoint::After => {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    self.push_expr_clock_domain(&symbol.found.kind, arg.identifier.as_ref().into());
                }
                self.check_expr_clock_domains(&arg.semicolon.semicolon_token.token);
            }
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.expr_clock_domains.clear(),
            HandlerPoint::After => {
                // clock domain is assigned to base identifier
                let ident = arg.expression_identifier.scoped_identifier.as_ref();
                if let Ok(symbol) = symbol_table::resolve(ident) {
                    self.push_expr_clock_domain(
                        &symbol.found.kind,
                        arg.expression_identifier.as_ref().into(),
                    );
                }

                self.check_expr_clock_domains(&arg.semicolon.semicolon_token.token);
            }
        }
        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.expr_clock_domains.clear(),
            HandlerPoint::After => {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    self.push_expr_clock_domain(&symbol.found.kind, arg.identifier.as_ref().into());
                }
                self.check_expr_clock_domains(&arg.semicolon.semicolon_token.token);
            }
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let range: TokenRange = arg.always_ff.always_ff_token.token.into();
                if let Some(ref x) = arg.always_ff_declaration_opt {
                    // clock domain is assigned to base identifier
                    let ident = x
                        .always_ff_event_list
                        .always_ff_clock
                        .hierarchical_identifier
                        .identifier
                        .as_ref();
                    if let Ok(symbol) = symbol_table::resolve(ident) {
                        match symbol.found.kind {
                            SymbolKind::Port(x) => {
                                self.always_ff_clock_domain = Some((x.clock_domain, range))
                            }
                            SymbolKind::Variable(x) => {
                                self.always_ff_clock_domain = Some((x.clock_domain, range))
                            }
                            _ => (),
                        }
                    }
                } else if let Some(ref x) = self.default_clock {
                    if let Some(symbol) = symbol_table::get(*x) {
                        match symbol.kind {
                            SymbolKind::Port(x) => {
                                self.always_ff_clock_domain = Some((x.clock_domain, range))
                            }
                            SymbolKind::Variable(x) => {
                                self.always_ff_clock_domain = Some((x.clock_domain, range))
                            }
                            _ => (),
                        }
                    }
                }
            }
            HandlerPoint::After => self.always_ff_clock_domain = None,
        }
        Ok(())
    }

    fn assign_declaration(&mut self, arg: &AssignDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.expr_clock_domains.clear(),
            HandlerPoint::After => {
                // clock domain is assigned to base identifier
                let ident = arg.hierarchical_identifier.identifier.as_ref();
                if let Ok(symbol) = symbol_table::resolve(ident) {
                    self.push_expr_clock_domain(
                        &symbol.found.kind,
                        arg.hierarchical_identifier.as_ref().into(),
                    );
                }
                self.check_expr_clock_domains(&arg.semicolon.semicolon_token.token);
            }
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.expr_clock_domains.clear(),
            HandlerPoint::After => {
                if arg.inst_port_item_opt.is_none() {
                    if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                        self.push_expr_clock_domain(
                            &symbol.found.kind,
                            arg.identifier.as_ref().into(),
                        );
                    }
                }
                let domain = self.check_expr_clock_domains(&arg.identifier.identifier_token.token);
                let range: TokenRange = arg.identifier.as_ref().into();
                self.inst_clock_domains
                    .insert(arg.identifier.identifier_token.token.text, (domain, range));
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.inst_clock_domains.clear(),
            HandlerPoint::After => {
                if let Ok(inst_symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    if let Some(type_kind) = get_inst_type_kind(&inst_symbol.found) {
                        if !matches!(type_kind, SymbolKind::Interface(_)) {
                            if let Some(ref x) = arg.inst_declaration_opt {
                                self.errors.push(AnalyzerError::invalid_clock_domain(
                                    &x.clock_domain.as_ref().into(),
                                ));
                                return Ok(());
                            }
                        }

                        let unsafe_token = &arg.semicolon.semicolon_token.token;
                        match type_kind {
                            SymbolKind::Module(x) => {
                                self.check_cdc_on_port_connections(&x.ports, unsafe_token);
                            }
                            SymbolKind::Interface(_) => {
                                if let SymbolKind::Instance(x) = &inst_symbol.found.kind {
                                    if x.clock_domain == ClockDomain::None {
                                        let mut property = x.clone();
                                        property.clock_domain = ClockDomain::Implicit;

                                        let mut symbol = inst_symbol.found.clone();
                                        symbol.kind = SymbolKind::Instance(property);
                                        symbol_table::update(symbol);
                                    }
                                }
                            }
                            SymbolKind::SystemVerilog => {
                                let mut prev: Option<(ClockDomain, TokenRange)> = None;
                                for curr in self.inst_clock_domains.values() {
                                    if let Some(prev) = prev {
                                        if !prev.0.compatible(&curr.0)
                                            && !unsafe_table::contains(unsafe_token, Unsafe::Cdc)
                                        {
                                            self.errors.push(AnalyzerError::mismatch_clock_domain(
                                                &curr.0.to_string(),
                                                &prev.0.to_string(),
                                                &curr.1,
                                                &prev.1,
                                            ));
                                        }
                                    }
                                    prev = Some(*curr);
                                }
                            }
                            SymbolKind::ProtoModule(x) => {
                                self.check_cdc_on_port_connections(&x.ports, unsafe_token);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
            if let SymbolKind::Module(ref x) = symbol.found.kind {
                self.default_clock = x.default_clock;
            }
        }
        Ok(())
    }
}

fn get_inst_type_kind(inst_symbol: &Symbol) -> Option<SymbolKind> {
    if let SymbolKind::Instance(ref x) = inst_symbol.kind {
        if let Ok(type_symbol) =
            symbol_table::resolve((&x.type_name.mangled_path(), &inst_symbol.namespace))
        {
            match type_symbol.found.kind {
                SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::SystemVerilog => {
                    return Some(type_symbol.found.kind)
                }
                SymbolKind::GenericInstance(ref x) => {
                    let base = symbol_table::get(x.base).unwrap();
                    return Some(base.kind);
                }
                SymbolKind::GenericParameter(ref x) => {
                    if let GenericBoundKind::Proto(ref x) = x.bound {
                        if let Ok(proto_symbol) =
                            symbol_table::resolve((x, &type_symbol.found.namespace))
                        {
                            return Some(proto_symbol.found.kind);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    None
}
