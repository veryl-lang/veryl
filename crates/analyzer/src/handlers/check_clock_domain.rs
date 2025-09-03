use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::symbol::{ClockDomain, Port, Symbol, SymbolId, SymbolKind};
use crate::symbol_table;
use crate::r#unsafe::Unsafe;
use crate::unsafe_table;
use veryl_parser::ParolError;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckClockDomain {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    expr_clock_domains: Vec<(ClockDomain, TokenRange)>,
    inst_clock_domains: HashMap<StrId, (ClockDomain, TokenRange)>,
    always_ff_clock_domain: Option<(ClockDomain, TokenRange)>,
    default_clock: Option<SymbolId>,
    default_reset: Option<SymbolId>,
}

impl CheckClockDomain {
    pub fn new() -> Self {
        Self::default()
    }

    fn set_always_ff_clock_domain(&mut self, symbol: &Symbol, range: TokenRange) {
        self.always_ff_clock_domain = get_clock_domain(symbol, range);
    }

    fn push_expr_clock_domain(&mut self, symbol: &Symbol, range: TokenRange) {
        if let Some(domain) = get_clock_domain(symbol, range) {
            self.expr_clock_domains.push(domain);
        }
    }

    fn check_expr_clock_domains(&mut self, token: &Token) -> ClockDomain {
        let cdc_unsafe = unsafe_table::contains(token, Unsafe::Cdc);
        let mut prev: Option<(ClockDomain, TokenRange)> = self.always_ff_clock_domain;
        for curr in &self.expr_clock_domains {
            if let Some(prev) = prev {
                check_clock_domain(curr, &prev, cdc_unsafe, &mut self.errors);
            }

            prev = Some(*curr);
        }
        prev.map(|(x, _)| x).unwrap_or(ClockDomain::None)
    }

    fn check_cdc_on_port_connections(&mut self, ports: &Vec<Port>, cdc_unsafe: bool) {
        let mut connection_table = HashMap::<ClockDomain, (ClockDomain, TokenRange)>::default();

        for port in ports {
            if let Some(connected) = self.inst_clock_domains.get(&port.name()) {
                let port_domain = port.property().clock_domain;
                if let Some(assigned) = connection_table.get(&port_domain) {
                    check_clock_domain(connected, assigned, cdc_unsafe, &mut self.errors);
                } else {
                    connection_table.insert(port_domain, *connected);
                }
            }
        }
    }

    fn check_inst(&mut self, arg: &ComponentInstantiation, semicolon: &Semicolon) {
        if let Ok(inst_symbol) = symbol_table::resolve(arg.identifier.as_ref())
            && let Some(type_kind) = get_inst_type_kind(&inst_symbol.found)
        {
            if !matches!(type_kind, SymbolKind::Interface(_))
                && let Some(ref x) = arg.component_instantiation_opt
            {
                self.errors.push(AnalyzerError::invalid_clock_domain(
                    &x.clock_domain.as_ref().into(),
                ));
                return;
            }

            let cdc_unsafe = unsafe_table::contains(&semicolon.semicolon_token.token, Unsafe::Cdc);
            match type_kind {
                SymbolKind::Module(x) => {
                    self.check_cdc_on_port_connections(&x.ports, cdc_unsafe);
                }
                SymbolKind::Interface(_) => {
                    if let SymbolKind::Instance(x) = &inst_symbol.found.kind
                        && x.clock_domain == ClockDomain::None
                    {
                        let mut property = x.clone();
                        property.clock_domain = ClockDomain::Implicit;

                        let mut symbol = inst_symbol.found.clone();
                        symbol.kind = SymbolKind::Instance(property);
                        symbol_table::update(symbol);
                    }
                }
                SymbolKind::SystemVerilog => {
                    let mut prev: Option<(ClockDomain, TokenRange)> = None;
                    for curr in self.inst_clock_domains.values() {
                        if let Some(prev) = prev {
                            check_clock_domain(curr, &prev, cdc_unsafe, &mut self.errors);
                        }
                        prev = Some(*curr);
                    }
                }
                SymbolKind::ProtoModule(x) => {
                    self.check_cdc_on_port_connections(&x.ports, cdc_unsafe);
                }
                _ => {}
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
        if let HandlerPoint::Before = self.point
            && let Ok(symbol) = symbol_table::resolve(arg)
        {
            self.push_expr_clock_domain(&symbol.found, arg.into());
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.expr_clock_domains.clear(),
            HandlerPoint::After => {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    self.push_expr_clock_domain(&symbol.found, arg.identifier.as_ref().into());
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
                        &symbol.found,
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
                    self.push_expr_clock_domain(&symbol.found, arg.identifier.as_ref().into());
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
                if let Some(clock) = arg.get_explicit_clock() {
                    // clock domain is assigned to base identifier
                    if let Ok(symbol) = symbol_table::resolve(clock.identifier.as_ref()) {
                        self.set_always_ff_clock_domain(&symbol.found, range);
                    }
                } else if let Some(clock) = self.default_clock
                    && let Some(symbol) = symbol_table::get(clock)
                {
                    self.set_always_ff_clock_domain(&symbol, range);
                }

                let check_reset = self.always_ff_clock_domain.is_some()
                    && arg.has_if_reset()
                    && (arg.has_explicit_clock() || arg.has_explicit_reset());
                if check_reset {
                    let clock_domain = self.always_ff_clock_domain.unwrap();

                    if let Some(reset) = arg.get_explicit_reset() {
                        // clock domain is assigned to base identifier
                        if let Ok(symbol) = symbol_table::resolve(reset.identifier.as_ref())
                            && let Some(reset_domain) =
                                get_clock_domain(&symbol.found, symbol.found.token.into())
                        {
                            check_clock_domain(
                                &reset_domain,
                                &clock_domain,
                                false,
                                &mut self.errors,
                            );
                        }
                    } else if let Some(reset) = self.default_reset
                        && let Some(symbol) = symbol_table::get(reset)
                        && let Some(reset_domain) = get_clock_domain(&symbol, symbol.token.into())
                    {
                        check_clock_domain(&reset_domain, &clock_domain, false, &mut self.errors);
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
                let idents: Vec<_> = arg.assign_destination.as_ref().into();
                for ident in idents {
                    // clock domain is assigned to base identifier not hierarchical_identifier
                    let ident = ident.identifier.as_ref();
                    if let Ok(symbol) = symbol_table::resolve(ident) {
                        self.push_expr_clock_domain(&symbol.found, ident.into());
                    }
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
                if arg.inst_port_item_opt.is_none()
                    && let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref())
                {
                    self.push_expr_clock_domain(&symbol.found, arg.identifier.as_ref().into());
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
                self.check_inst(&arg.component_instantiation, &arg.semicolon);
            }
        }
        Ok(())
    }

    fn bind_declaration(&mut self, arg: &BindDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.inst_clock_domains.clear(),
            HandlerPoint::After => {
                if symbol_table::resolve(arg.scoped_identifier.as_ref()).is_ok() {
                    self.check_inst(&arg.component_instantiation, &arg.semicolon);
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
                self.default_reset = x.default_reset;

                if let (Some(clock), Some(reset)) = (self.default_clock, self.default_reset) {
                    let clock = symbol_table::get(clock)
                        .map(|x| get_clock_domain(&x, x.token.into()).unwrap())
                        .unwrap();
                    let reset = symbol_table::get(reset)
                        .map(|x| get_clock_domain(&x, x.token.into()).unwrap())
                        .unwrap();
                    check_clock_domain(&reset, &clock, false, &mut self.errors);
                }
            }
        }
        Ok(())
    }
}

fn get_clock_domain(symbol: &Symbol, range: TokenRange) -> Option<(ClockDomain, TokenRange)> {
    match &symbol.kind {
        SymbolKind::Port(x) => Some((x.clock_domain, range)),
        SymbolKind::Variable(x) => Some((x.clock_domain, range)),
        SymbolKind::Instance(x) => Some((x.clock_domain, range)),
        _ => None,
    }
}

fn get_inst_type_kind(inst_symbol: &Symbol) -> Option<SymbolKind> {
    if let SymbolKind::Instance(ref x) = inst_symbol.kind
        && let Ok(type_symbol) =
            symbol_table::resolve((&x.type_name.mangled_path(), &inst_symbol.namespace))
    {
        match type_symbol.found.kind {
            SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::SystemVerilog => {
                return Some(type_symbol.found.kind);
            }
            SymbolKind::GenericInstance(ref x) => {
                let base = symbol_table::get(x.base).unwrap();
                return Some(base.kind);
            }
            SymbolKind::GenericParameter(x) => {
                if let Some(proto) = x.bound.resolve_proto_bound(&inst_symbol.namespace) {
                    return proto.get_symbol().map(|x| x.kind);
                }
            }
            _ => {}
        }
    }

    None
}

fn check_clock_domain(
    lhs_domain: &(ClockDomain, TokenRange),
    rhs_domain: &(ClockDomain, TokenRange),
    cdc_unsafe: bool,
    errors: &mut Vec<AnalyzerError>,
) {
    if !lhs_domain.0.compatible(&rhs_domain.0) && !cdc_unsafe {
        errors.push(AnalyzerError::mismatch_clock_domain(
            &lhs_domain.0.to_string(),
            &rhs_domain.0.to_string(),
            &lhs_domain.1,
            &rhs_domain.1,
        ));
    }
}
