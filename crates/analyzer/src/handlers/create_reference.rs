use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::symbol::{Symbol, SymbolKind};
use crate::symbol_table::{self, SymbolPath};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateReference<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CreateReference<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CreateReference<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn scoped_identifier_tokens(arg: &ScopedIdentifier) -> Vec<Token> {
    let mut ret = Vec::new();
    if let Some(ref x) = arg.scoped_identifier_opt {
        ret.push(x.dollar.dollar_token.token);
    }
    ret.push(arg.identifier.identifier_token.token);
    for x in &arg.scoped_identifier_list {
        ret.push(x.identifier.identifier_token.token);
    }
    ret
}

fn expression_identifier_tokens(arg: &ExpressionIdentifier) -> Vec<Token> {
    let mut ret = Vec::new();
    if let Some(ref x) = arg.expression_identifier_opt {
        ret.push(x.dollar.dollar_token.token);
    }
    ret.push(arg.identifier.identifier_token.token);
    if let ExpressionIdentifierGroup::ExpressionIdentifierScoped(x) =
        arg.expression_identifier_group.as_ref()
    {
        let x = &x.expression_identifier_scoped;
        ret.push(x.identifier.identifier_token.token);
        for x in &x.expression_identifier_scoped_list {
            ret.push(x.identifier.identifier_token.token);
        }
    }
    ret
}

impl<'a> VerylGrammarTrait for CreateReference<'a> {
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for symbol in symbol.full_path {
                        symbol_table::add_reference(
                            symbol.token.id,
                            &arg.identifier.identifier_token.token,
                        );
                    }
                }
                Err(err) => {
                    let is_single_identifier = SymbolPath::from(arg).as_slice().len() == 1;
                    let name = arg.identifier.identifier_token.text();
                    if name == "_" && is_single_identifier {
                        return Ok(());
                    }

                    if let Some(last_found) = err.last_found {
                        // TODO check SV-side member to suppress error
                        let name = format!("{}", last_found.token.text);
                        let member = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::unknown_member(
                            &name,
                            &member,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    } else {
                        let name = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::undefined_identifier(
                            &name,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // Add symbols under $sv namespace
            if arg.scoped_identifier_opt.is_some() && arg.identifier.identifier_token.text() == "sv"
            {
                let mut namespace = Namespace::new();
                for (i, token) in scoped_identifier_tokens(arg).iter().enumerate() {
                    if i != 0 {
                        let symbol =
                            Symbol::new(token, SymbolKind::SystemVerilog, &namespace, vec![]);
                        let _ = symbol_table::insert(token, symbol);
                    }
                    namespace.push(token.text);
                }
            }

            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for symbol in symbol.full_path {
                        symbol_table::add_reference(
                            symbol.token.id,
                            &arg.identifier.identifier_token.token,
                        );
                    }
                }
                Err(err) => {
                    if let Some(last_found) = err.last_found {
                        let name = format!("{}", last_found.token.text);
                        let member = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::unknown_member(
                            &name,
                            &member,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    } else {
                        let name = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::undefined_identifier(
                            &name,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // Add symbols under $sv namespace
            if arg.expression_identifier_opt.is_some() {
                if let ExpressionIdentifierGroup::ExpressionIdentifierScoped(_) =
                    arg.expression_identifier_group.as_ref()
                {
                    if arg.identifier.identifier_token.text() == "sv" {
                        let mut namespace = Namespace::new();
                        for (i, token) in expression_identifier_tokens(arg).iter().enumerate() {
                            if i != 0 {
                                let symbol = Symbol::new(
                                    token,
                                    SymbolKind::SystemVerilog,
                                    &namespace,
                                    vec![],
                                );
                                let _ = symbol_table::insert(token, symbol);
                            }
                            namespace.push(token.text);
                        }
                    }
                }
            }

            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for symbol in symbol.full_path {
                        symbol_table::add_reference(
                            symbol.token.id,
                            &arg.identifier.identifier_token.token,
                        );
                    }
                }
                Err(err) => {
                    let is_single_identifier = SymbolPath::from(arg).as_slice().len() == 1;
                    let name = arg.identifier.identifier_token.text();
                    if name == "_" && is_single_identifier {
                        return Ok(());
                    }

                    if let Some(last_found) = err.last_found {
                        let name = format!("{}", last_found.token.text);
                        let member = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::unknown_member(
                            &name,
                            &member,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    } else {
                        let name = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::undefined_identifier(
                            &name,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn modport_item(&mut self, arg: &ModportItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg.identifier.as_ref()) {
                Ok(symbol) => {
                    for symbol in symbol.full_path {
                        symbol_table::add_reference(
                            symbol.token.id,
                            &arg.identifier.identifier_token.token,
                        );
                    }
                }
                Err(err) => {
                    if let Some(last_found) = err.last_found {
                        let name = format!("{}", last_found.token.text);
                        let member = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::unknown_member(
                            &name,
                            &member,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    } else {
                        let name = format!("{}", err.not_found);
                        self.errors.push(AnalyzerError::undefined_identifier(
                            &name,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // implicit port connection by name
            if arg.inst_port_item_opt.is_none() {
                match symbol_table::resolve(arg.identifier.as_ref()) {
                    Ok(symbol) => {
                        for symbol in symbol.full_path {
                            symbol_table::add_reference(
                                symbol.token.id,
                                &arg.identifier.identifier_token.token,
                            );
                        }
                    }
                    Err(err) => {
                        if let Some(last_found) = err.last_found {
                            let name = format!("{}", last_found.token.text);
                            let member = format!("{}", err.not_found);
                            self.errors.push(AnalyzerError::unknown_member(
                                &name,
                                &member,
                                self.text,
                                &arg.identifier.identifier_token,
                            ));
                        } else {
                            let name = format!("{}", err.not_found);
                            self.errors.push(AnalyzerError::undefined_identifier(
                                &name,
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
