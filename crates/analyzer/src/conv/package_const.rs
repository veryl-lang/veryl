use crate::HashMap;
use crate::ir::Comptime;
use crate::literal::Literal;
use crate::literal_table;
use crate::symbol::{GenericMap, Symbol, SymbolId, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, GenericSymbolPathKind};
use crate::symbol_table;
use veryl_parser::resource_table::StrId;

/// A package const can be neither overridden nor bound to a module parameter,
/// so its value depends only on the symbol and the generic maps in scope.
#[derive(Clone, Default)]
pub struct PackageConstCache {
    /// With the `eval_factor_path` depth the evaluation took.
    values: HashMap<PackageConstKey, (Comptime, usize)>,
}

/// The sites finish the value differently.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageConstUse {
    Factor,
    StructWhole,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PackageConstKey {
    symbol: SymbolId,
    usage: PackageConstUse,
    in_generic: bool,
    generic_maps: Vec<Vec<Vec<(StrId, CanonicalArg)>>>,
}

/// An identifier is keyed by the symbol it resolves to, not its text: its
/// meaning depends on the scope it was written in.
#[derive(Clone, PartialEq, Eq, Hash)]
enum CanonicalArg {
    Literal(Literal),
    Type(GenericSymbolPathKind, StrId),
    Symbol(SymbolId, Vec<Vec<CanonicalArg>>),
}

impl PackageConstCache {
    pub fn get(&self, key: &PackageConstKey) -> Option<(&Comptime, usize)> {
        self.values.get(key).map(|(value, depth)| (value, *depth))
    }

    pub fn insert(&mut self, key: PackageConstKey, value: Comptime, depth: usize) {
        self.values.insert(key, (value, depth));
    }
}

impl PackageConstKey {
    pub fn new(
        symbol: &Symbol,
        usage: PackageConstUse,
        in_generic: bool,
        generic_maps: &[Vec<GenericMap>],
    ) -> Option<Self> {
        let parent = symbol.get_parent()?;
        if !matches!(&parent.kind, SymbolKind::Package(x) if !x.is_proto) {
            return None;
        }

        // Frames stay apart since each substitutes once. Empty maps, pushed
        // per path segment, would make nesting depth part of the key.
        let maps = generic_maps
            .iter()
            .map(|frame| {
                frame
                    .iter()
                    .filter(|map| !map.map.is_empty())
                    .map(|map| {
                        let mut entries = map
                            .map
                            .iter()
                            .map(|(name, arg)| Some((*name, canonical_arg(arg)?)))
                            .collect::<Option<Vec<_>>>()?;
                        entries.sort_by_key(|(name, _)| *name);
                        Some(entries)
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .filter(|frame| !matches!(frame, Some(x) if x.is_empty()))
            .collect::<Option<Vec<_>>>()?;

        Some(Self {
            symbol: symbol.id,
            usage,
            in_generic,
            generic_maps: maps,
        })
    }
}

fn canonical_arg(arg: &GenericSymbolPath) -> Option<CanonicalArg> {
    let base = arg.paths.first()?.base;
    if let Some(x) = literal_table::get(&base.id) {
        return Some(CanonicalArg::Literal(x));
    }
    match &arg.kind {
        GenericSymbolPathKind::ValueLiteral => None,
        GenericSymbolPathKind::TypeLiteral | GenericSymbolPathKind::VariableType(_) => {
            Some(CanonicalArg::Type(arg.kind.clone(), base.text))
        }
        GenericSymbolPathKind::Identifier => {
            let symbol = symbol_table::resolve(arg).ok()?;
            let args = arg
                .paths
                .iter()
                .map(|x| x.arguments.iter().map(canonical_arg).collect())
                .collect::<Option<Vec<_>>>()?;
            Some(CanonicalArg::Symbol(symbol.found.id, args))
        }
    }
}
