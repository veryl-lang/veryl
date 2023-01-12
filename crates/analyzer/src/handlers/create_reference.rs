use crate::analyze_error::AnalyzeError;
use crate::namespace_table;
use crate::symbol_table::{self, SymbolPath};
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CreateReference<'a> {
    pub errors: Vec<AnalyzeError>,
    _text: &'a str,
    point: HandlerPoint,
}

impl<'a> CreateReference<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            _text: text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CreateReference<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateReference<'a> {
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let namespace = namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg);
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier.identifier_token.token,
                );
            }
        }
        Ok(())
    }

    fn scoped_or_hier_identifier(&mut self, arg: &ScopedOrHierIdentifier) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let namespace = namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg);
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier.identifier_token.token,
                );
            }
        }
        Ok(())
    }

    fn modport_item(&mut self, arg: &ModportItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let namespace = namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg.identifier.as_ref());
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier.identifier_token.token,
                );
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let namespace =
                namespace_table::get(arg.identifier0.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg.identifier0.as_ref());
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier0.identifier_token.token,
                );
            }
        }
        Ok(())
    }
}
