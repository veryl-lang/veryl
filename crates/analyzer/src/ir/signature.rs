use crate::conv::Context;
use crate::ir::ValueVariant;
use crate::symbol::{SymbolId, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Signature {
    pub symbol: SymbolId,
    pub parameters: Vec<(StrId, ValueVariant)>,
    pub generic_parameters: Vec<(StrId, GenericSymbolPath)>,
}

impl Signature {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            parameters: vec![],
            generic_parameters: vec![],
        }
    }

    pub fn add_parameter(&mut self, id: StrId, value: ValueVariant) {
        self.parameters.push((id, value));
    }

    pub fn add_generic_parameter(&mut self, id: StrId, value: GenericSymbolPath) {
        self.generic_parameters.push((id, value));
    }

    pub fn normalize(&mut self) {
        self.parameters.sort();
        self.generic_parameters.sort();
    }

    pub fn from_path(context: &mut Context, mut path: GenericSymbolPath) -> Option<Self> {
        path.unalias();
        let symbol = symbol_table::resolve(&path).ok()?;

        let symbol = match &symbol.found.kind {
            SymbolKind::Module(_)
            | SymbolKind::Interface(_)
            | SymbolKind::Modport(_)
            | SymbolKind::Function(_)
            | SymbolKind::SystemVerilog => symbol.found,
            SymbolKind::ModportFunctionMember(x) => symbol_table::get(x.function).unwrap(),
            SymbolKind::GenericParameter(_) => {
                let path = context.resolve_path(path.clone());
                let symbol = symbol_table::resolve(&path).ok()?;
                if let SymbolKind::GenericParameter(_) = symbol.found.kind {
                    return None;
                }
                symbol.found
            }
            _ => {
                return None;
            }
        };

        let mut sig = Signature::new(symbol.id);

        if path.is_generic() {
            let mut x: SymbolPathNamespace = (&path).into();
            x.0 = path.mangled_path();
            if let Ok(symbol) = symbol_table::resolve(&x)
                && let SymbolKind::GenericInstance(x) = &symbol.found.kind
            {
                let base = symbol_table::get(x.base).unwrap();
                let params = base.kind.get_generic_parameters();

                if params.len() == x.arguments.len() {
                    for (i, p) in params.iter().enumerate() {
                        let p = symbol_table::get(*p).unwrap();
                        let name = p.token.text;
                        sig.add_generic_parameter(name, x.arguments[i].clone());
                    }
                }
            }
        }

        Some(sig)
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let symbol = symbol_table::get(self.symbol).unwrap();
        let mut ret = symbol.token.text.to_string();

        for x in &self.generic_parameters {
            ret.push_str(&format!("::<{}>", x.1));
        }

        ret.fmt(f)
    }
}
