use crate::conv::Context;
use crate::generic_inference_table;
use crate::ir::ValueVariant;
use crate::namespace::Namespace;
use crate::symbol::GenericMap;
use crate::symbol::{GenericBoundKind, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::GenericSymbolPath;
use crate::{namespace_table, symbol_table};
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Signature {
    pub symbol: SymbolId,
    pub full_path: Vec<StrId>,
    pub parameters: Vec<(StrId, ValueVariant)>,
    pub generic_parameters: Vec<(StrId, GenericSymbolPath)>,
}

impl Signature {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            full_path: vec![],
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

        generic_inference_table::apply_inferred_args(&mut path, &symbol.found);
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
                    let current_namespace = context.current_namespace();
                    for id in &symbol.full_path {
                        let symbol = symbol_table::get(*id).unwrap();
                        let SymbolKind::GenericInstance(inst) = &symbol.kind else {
                            continue;
                        };

                        let base = symbol_table::get(inst.base).unwrap();
                        let params = base.generic_parameters();
                        if inst.arguments.len() == params.len() {
                            for (i, (name, _)) in params.iter().enumerate() {
                                let mut arg = inst.arguments[i].clone();
                                if let Some(current_namespace) = &current_namespace {
                                    arg.append_namespace_path(current_namespace, &base.namespace);
                                }
                                sig.add_generic_parameter(*name, arg);
                            }
                        }
                    }

                    sig.full_path
                        .append(&mut symbol.found.inner_namespace().paths.to_vec());
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

        let symbol = symbol_table::get(self.symbol).unwrap();
        symbol.eval_generic_consts(&mut ret);

        vec![ret]
    }

    pub fn namespace(&self) -> Namespace {
        if self.full_path.is_empty() {
            let symbol = symbol_table::get(self.symbol).unwrap();
            symbol.inner_namespace()
        } else {
            let mut ret = Namespace::new();

            for path in &self.full_path {
                ret.push(*path);
            }

            ret
        }
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
