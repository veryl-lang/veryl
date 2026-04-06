use crate::conv::Context;
use crate::ir::ValueVariant;
use crate::symbol::GenericMap;
use crate::symbol::{GenericBoundKind, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::GenericSymbolPath;
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
        let mut sig = match &symbol.found.kind {
            SymbolKind::Module(_)
            | SymbolKind::Interface(_)
            | SymbolKind::Modport(_)
            | SymbolKind::Function(_)
            | SymbolKind::SystemVerilog => Self::new(symbol.found.id),
            SymbolKind::ModportFunctionMember(x) => Self::new(x.function),
            SymbolKind::GenericParameter(_) => {
                let path = context.resolve_path(path.clone());
                let symbol = symbol_table::resolve(&path).ok()?;
                if let SymbolKind::GenericParameter(x) = &symbol.found.kind {
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
                Self::new(symbol.found.id)
            }
            SymbolKind::ProtoFunction(_) => {
                let resolved = context.resolve_path(path.clone());
                let symbol = symbol_table::resolve(&resolved).ok()?;
                match &symbol.found.kind {
                    SymbolKind::Function(_) => Self::new(symbol.found.id),
                    _ => return None,
                }
            }
            SymbolKind::ProtoAliasModule(x) => {
                let symbol = symbol_table::resolve(&x.target).ok()?;
                return Some(Signature::new(symbol.found.id));
            }
            _ => return None,
        };

        if !context.in_generic {
            // Apply default value
            for (i, id) in symbol.full_path.iter().enumerate() {
                let path_symbol = if (i + 1) == symbol.full_path.len() {
                    symbol_table::get(sig.symbol).unwrap()
                } else {
                    symbol_table::get(*id).unwrap()
                };

                let params = path_symbol.generic_parameters();
                let n_args = path.paths[i].arguments.len();
                for (_, default_value) in params.iter().skip(n_args) {
                    if let Some(default_value) = &default_value.default_value {
                        path.paths[i].arguments.push(default_value.clone())
                    }
                }
            }

            if path.is_generic() {
                let namespace = namespace_table::get(path.paths[0].base.id).unwrap();
                path.resolve_imported(&namespace, None);

                // Apply generic map
                let path = context.resolve_path(path);

                let namespace = namespace_table::get(path.paths[0].base.id).unwrap();
                if let Ok(symbol) = symbol_table::resolve((&path.mangled_path(), &namespace)) {
                    for id in &symbol.full_path {
                        let symbol = symbol_table::get(*id).unwrap();
                        let SymbolKind::GenericInstance(inst) = &symbol.kind else {
                            continue;
                        };

                        let base = symbol_table::get(inst.base).unwrap();
                        let params = base.generic_parameters();
                        if inst.arguments.len() == params.len() {
                            for (i, (name, _)) in params.iter().enumerate() {
                                sig.add_generic_parameter(*name, inst.arguments[i].clone());
                            }
                        }
                    }
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
