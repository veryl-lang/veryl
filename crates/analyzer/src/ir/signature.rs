use crate::conv::Context;
use crate::generic_inference_table;
use crate::ir::ValueVariant;
use crate::namespace::Namespace;
use crate::symbol::GenericMap;
use crate::symbol::{GenericBoundKind, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::GenericSymbolPath;
use crate::{scope, symbol_table};
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Signature {
    pub symbol: SymbolId,
    pub full_path: Vec<StrId>,
    pub parameters: Vec<(StrId, ValueVariant)>,
    pub generic_parameters: Vec<(StrId, GenericSymbolPath)>,
    /// Signatures of the interface instances connected to this instance's
    /// modport ports (port name -> interface signature). A module body is
    /// monomorphized by the parameters of the interfaces behind its modport
    /// ports, so they must participate in the instance cache key; otherwise
    /// a module connected to `some_if #(W: 128)` reuses the body built for
    /// `some_if` at its default parameters (or vice versa).
    pub modport_signatures: Vec<(StrId, Signature)>,
}

fn same_generic_argument(left: &GenericSymbolPath, right: &GenericSymbolPath) -> bool {
    left.kind == right.kind && left.mangled_path() == right.mangled_path()
}

fn canonical_function_argument(
    context: &Context,
    symbol: SymbolId,
    name: StrId,
    argument: GenericSymbolPath,
) -> GenericSymbolPath {
    context
        .func_paths
        .keys()
        .filter(|path| path.sig.symbol == symbol)
        .flat_map(|path| &path.sig.generic_parameters)
        .find(|(existing_name, existing)| {
            *existing_name == name && same_generic_argument(existing, &argument)
        })
        .map(|(_, existing)| existing.clone())
        .unwrap_or(argument)
}

impl Signature {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            full_path: vec![],
            parameters: vec![],
            generic_parameters: vec![],
            modport_signatures: vec![],
        }
    }

    pub fn is_compatible(
        &self,
        x: &Signature,
        ignore_params: bool,
        ignore_generic_params: bool,
    ) -> bool {
        if self.symbol != x.symbol {
            return false;
        }
        if !ignore_params && self.parameters != x.parameters {
            return false;
        }
        if !ignore_generic_params && self.generic_parameters != x.generic_parameters {
            return false;
        }
        true
    }

    pub fn add_parameter(&mut self, id: StrId, value: ValueVariant) {
        self.parameters.push((id, value));
    }

    pub fn add_generic_parameter(&mut self, id: StrId, value: GenericSymbolPath) {
        self.generic_parameters.push((id, value));
    }

    pub fn add_modport_signature(&mut self, id: StrId, sig: Signature) {
        self.modport_signatures.push((id, sig));
    }

    pub fn normalize(&mut self) {
        self.parameters.sort();
        self.generic_parameters.sort();
        self.modport_signatures.sort();
    }

    pub fn from_path(context: &mut Context, mut path: GenericSymbolPath) -> Option<Self> {
        let (scope, define_context) = scope::token_scope(path.paths[0].base.id).unwrap();
        path.resolve_imported(scope, &define_context, None);
        path.unalias(None);

        let symbol = symbol_table::resolve(&path).ok()?;

        generic_inference_table::apply_inferred_args(&mut path, &symbol.found);
        let mut sig = match &symbol.found.kind {
            SymbolKind::Function(x) if x.is_proto => {
                let resolved = context.resolve_path(path.clone());
                let symbol = symbol_table::resolve(&resolved).ok()?;
                match &symbol.found.kind {
                    SymbolKind::Function(_) => Self::new(symbol.found.id),
                    _ => return None,
                }
            }
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
            SymbolKind::AliasModule(x) if x.is_proto => {
                let symbol = symbol_table::resolve(&x.target).ok()?;
                return Some(Signature::new(symbol.found.id));
            }
            _ => return None,
        };

        if !context.in_generic {
            let namespace = scope::namespace(scope, &define_context);

            for (i, id) in symbol.full_path.iter().enumerate() {
                let path_symbol = if (i + 1) == symbol.full_path.len() {
                    symbol_table::get(sig.symbol).unwrap()
                } else {
                    symbol_table::get(*id).unwrap()
                };

                // Apply default value
                let params = path_symbol.generic_parameters();
                let n_args = path.paths[i].arguments.len();
                for (_, default_value) in params.iter().skip(n_args) {
                    if let Some(default_value) = &default_value.default_value {
                        path.paths[i].arguments.push(default_value.clone())
                    }
                }

                if !path_symbol.is_global_function() {
                    for arg in path.paths[i].arguments.iter_mut() {
                        arg.append_namespace_path(&namespace, &path_symbol.namespace);
                    }
                }
            }

            if path.is_generic() {
                let (scope, define_context) = scope::token_scope(path.paths[0].base.id).unwrap();
                path.resolve_imported(scope, &define_context, None);

                // Apply generic map
                let path = context.resolve_path(path);
                let mut structurally_resolved_function_parameters = Vec::new();
                let direct_function_arguments = {
                    let base = symbol_table::get(sig.symbol).unwrap();
                    if matches!(base.kind, SymbolKind::Function(_)) {
                        let params = base.generic_parameters();
                        path.paths
                            .last()
                            .filter(|segment| segment.arguments.len() == params.len())
                            .map(|segment| {
                                params
                                    .into_iter()
                                    .zip(&segment.arguments)
                                    .map(|((name, _), argument)| (name, argument.clone()))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    }
                };

                if let Some((found, full_path)) =
                    symbol_table::resolve_generic_structural(&path, path.paths[0].base.id)
                        .ok()
                        .map(|r| (r.found.clone(), r.full_path.clone()))
                {
                    let current_namespace = context.current_namespace();
                    for id in &full_path {
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
                                if inst.base == sig.symbol {
                                    structurally_resolved_function_parameters.push(*name);
                                }
                            }
                        }
                    }

                    sig.full_path
                        .append(&mut found.inner_namespace().paths.to_vec());
                }

                // A generic const evaluated while converting one function
                // specialization can produce a concrete specialization that
                // pass1 did not register as a `GenericInstance`. The resolved
                // final-segment arguments still form its canonical call key.
                for (name, argument) in direct_function_arguments {
                    // Structural resolution is authoritative when it found
                    // this parameter: it may have qualified a container-local
                    // generic const (for example `Pkg::<1>::B`). The direct
                    // syntax is only a fallback for a missing function
                    // specialization and must not replace that resolved path.
                    if !structurally_resolved_function_parameters.contains(&name) {
                        let argument =
                            canonical_function_argument(context, sig.symbol, name, argument);
                        sig.add_generic_parameter(name, argument);
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
