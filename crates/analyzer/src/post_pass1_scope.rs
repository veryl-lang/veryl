//! What one run of `Analyzer::analyze_post_pass1_scoped` covers.
//!
//! The packages and interfaces marked `#[export(rust)]` refer to nothing but
//! exported packages, so they can be resolved apart from the rest
//! ([`PostPass1Scope::Export`]): their description is then written before
//! the components it is expanded into are built, and the rest
//! ([`PostPass1Scope::Rest`]) is resolved after the components are known.
//!
//! What belongs to them is told by [`Symbol::exported`], set in pass1.

use crate::namespace::Namespace;
use crate::scope::{self, ScopeId};
use crate::symbol::{Symbol, SymbolId, SymbolKind};
use crate::symbol_table;
use veryl_parser::veryl_token::Token;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostPass1Scope {
    /// Everything at once.
    All,
    /// The exported packages and interfaces.
    Export,
    /// What [`Self::Export`] left.
    Rest,
}

/// Whether a symbol is a top-level package or interface marked
/// `#[export(rust)]`.
pub fn is_exported_item(symbol: &Symbol) -> bool {
    symbol.exported
        && symbol.namespace.depth() == 1
        && matches!(
            symbol.kind,
            SymbolKind::Package(_) | SymbolKind::Interface(_)
        )
}

/// Whether what is written in `scope` belongs to an exported package or
/// interface: the nearest scope owned by a symbol tells. `exported` tells
/// whether a symbol is [`Symbol::exported`], so that the symbol table can
/// ask while borrowed.
fn is_exported_scope(mut scope: ScopeId, exported: &impl Fn(SymbolId) -> bool) -> bool {
    loop {
        if let Some(owner) = scope::owner_of(scope) {
            return exported(owner);
        }
        match scope::parent(scope) {
            Some(parent) => scope = parent,
            None => return false,
        }
    }
}

fn exported_symbol(id: SymbolId) -> bool {
    symbol_table::get_shared(id).is_some_and(|x| x.exported)
}

/// Whether what is written in `namespace` belongs to an exported package or
/// interface, with `exported` telling whether a symbol is.
pub fn in_exported_namespace_by(
    namespace: &Namespace,
    exported: &impl Fn(SymbolId) -> bool,
) -> bool {
    namespace.depth() >= 2 && is_exported_scope(scope::intern_namespace(namespace), exported)
}

/// Whether what is written at `token` belongs to an exported package or
/// interface, with `exported` telling whether a symbol is.
pub fn in_exported_token_by(token: &Token, exported: &impl Fn(SymbolId) -> bool) -> bool {
    scope::token_scope(token.id).is_some_and(|(scope, _)| is_exported_scope(scope, exported))
}

/// Whether what is written in `namespace` belongs to an exported package or
/// interface.
pub fn in_exported_namespace(namespace: &Namespace) -> bool {
    in_exported_namespace_by(namespace, &exported_symbol)
}

/// Whether what is written at `token` belongs to an exported package or
/// interface.
pub fn in_exported_token(token: &Token) -> bool {
    in_exported_token_by(token, &exported_symbol)
}

impl PostPass1Scope {
    fn includes(&self, exported: impl FnOnce() -> bool) -> bool {
        match self {
            PostPass1Scope::All => true,
            PostPass1Scope::Export => exported(),
            PostPass1Scope::Rest => !exported(),
        }
    }

    /// Whether something written in `namespace` is covered.
    pub fn includes_namespace(&self, namespace: &Namespace) -> bool {
        self.includes(|| in_exported_namespace(namespace))
    }

    /// Whether a symbol is covered.
    pub fn includes_symbol(&self, symbol: &Symbol) -> bool {
        self.includes(|| symbol.exported)
    }

    /// Whether something written at `token` is covered.
    pub fn includes_token(&self, token: &Token) -> bool {
        self.includes(|| in_exported_token(token))
    }

    /// Takes the entries of `list` this run covers. Every entry left is
    /// outside [`Self::Export`] once it ran, so the other runs take them all.
    pub fn take<T>(&self, list: &mut Vec<T>, exported: impl Fn(&T) -> bool) -> Vec<T> {
        match self {
            PostPass1Scope::Export => list.extract_if(.., |x| exported(x)).collect(),
            PostPass1Scope::All | PostPass1Scope::Rest => std::mem::take(list),
        }
    }
}
