use crate::conv::Context;
use crate::ir::ValueVariant;
use crate::symbol::GenericMap;
use crate::symbol::{GenericBoundKind, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPathNamespace};
use crate::{namespace_table, symbol_table};
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
        let namespace = namespace_table::get(path.paths[0].base.id).unwrap();
        path.resolve_imported(&namespace, None);
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
                if let SymbolKind::GenericParameter(x) = symbol.found.kind {
                    if let GenericBoundKind::Proto(x) = &x.bound {
                        if let TypeKind::UserDefined(x) = &x.kind {
                            let symbol = symbol_table::resolve(&x.path).ok()?;
                            return Some(Signature::new(symbol.found.id));
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                }
                symbol.found
            }
            SymbolKind::ProtoAliasModule(x) => {
                let symbol = symbol_table::resolve(&x.target).ok()?;
                return Some(Signature::new(symbol.found.id));
            }
            _ => {
                return None;
            }
        };

        let mut sig = Signature::new(symbol.id);

        let generic_args = path.paths[0].arguments.len();
        for (i, (_, default_value)) in symbol.generic_parameters().iter().enumerate() {
            if i >= generic_args
                && let Some(x) = &default_value.default_value
            {
                path.paths[0].arguments.push(x.clone());
            }
        }

        if path.is_generic() {
            let namespace = namespace_table::get(path.paths[0].base.id).unwrap();
            path.resolve_imported(&namespace, None);

            // apply generic parameters in this context
            let path = context.resolve_path(path);

            let mut x: SymbolPathNamespace = (&path).into();
            x.0 = path.mangled_path();
            if let Ok(symbol) = symbol_table::resolve(&x) {
                match &symbol.found.kind {
                    SymbolKind::GenericInstance(x) => {
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
                    SymbolKind::Module(x) => {
                        for (i, p) in x.generic_parameters.iter().enumerate() {
                            let p = symbol_table::get(*p).unwrap();
                            let name = p.token.text;
                            sig.add_generic_parameter(
                                name,
                                path.paths.last().unwrap().arguments[i].clone(),
                            );
                        }
                    }
                    SymbolKind::Interface(x) => {
                        for (i, p) in x.generic_parameters.iter().enumerate() {
                            let p = symbol_table::get(*p).unwrap();
                            let name = p.token.text;
                            sig.add_generic_parameter(
                                name,
                                path.paths.last().unwrap().arguments[i].clone(),
                            );
                        }
                    }
                    _ => (),
                }
            } else {
                match &symbol.kind {
                    SymbolKind::Module(x) => {
                        for (i, p) in x.generic_parameters.iter().enumerate() {
                            let p = symbol_table::get(*p).unwrap();
                            let name = p.token.text;
                            sig.add_generic_parameter(
                                name,
                                path.paths.last().unwrap().arguments[i].clone(),
                            );
                        }
                    }
                    SymbolKind::Interface(x) => {
                        for (i, p) in x.generic_parameters.iter().enumerate() {
                            let p = symbol_table::get(*p).unwrap();
                            let name = p.token.text;
                            sig.add_generic_parameter(
                                name,
                                path.paths.last().unwrap().arguments[i].clone(),
                            );
                        }
                    }
                    _ => (),
                }
            }
        }

        Some(sig)
    }

    pub fn to_generic_map(&self) -> Vec<GenericMap> {
        let mut ret = GenericMap::default();
        for (key, val) in &self.generic_parameters {
            ret.map.insert(*key, val.clone());
        }
        vec![ret]
    }

    pub fn is_generic(&self) -> bool {
        !self.generic_parameters.is_empty()
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
