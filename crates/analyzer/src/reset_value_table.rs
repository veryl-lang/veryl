use crate::AnalyzerError;
use crate::HashMap;
use crate::symbol::{Direction, SymbolKind, Type, TypeKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use crate::utils::{InstTypeSource, resolve_inst_type};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use veryl_parser::resource_table::{StrId, TokenId};
use veryl_parser::veryl_token::Token;

/// A reset value literal (`reset_active` / `reset_inactive`) and the counterpart
/// whose reset type determines its polarity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResetValueCandidate {
    pub token: Token,
    pub target: ResetValueTarget,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ResetValueTarget {
    /// Assignment destination, counterpart of `==`/`!=` or typed `let`.
    /// `select` is the number of selects applied to the last element of the path.
    Variable {
        path: GenericSymbolPath,
        select: usize,
    },
    /// Port connection of an instance.
    InstPort {
        inst_type: GenericSymbolPath,
        port: StrId,
    },
}

#[derive(Default, Debug)]
pub struct ResetValueTable {
    candidates: Vec<ResetValueCandidate>,
    /// Reset type of the counterpart of each valid literal.
    table: HashMap<TokenId, TypeKind>,
}

enum Resolved {
    Valid(TypeKind),
    Invalid,
    Unresolved,
}

impl ResetValueTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, candidate: ResetValueCandidate) {
        self.candidates.push(candidate);
    }

    pub fn apply(&mut self) -> Vec<AnalyzerError> {
        let mut errors = Vec::new();
        let candidates = std::mem::take(&mut self.candidates);

        for candidate in &candidates {
            let resolved = match &candidate.target {
                ResetValueTarget::Variable { path, select } => resolve_variable(path, *select),
                ResetValueTarget::InstPort { inst_type, port } => {
                    resolve_inst_port(inst_type, *port)
                }
            };

            match resolved {
                Resolved::Valid(x) => {
                    self.table.insert(candidate.token.id, x);
                }
                Resolved::Invalid => {
                    errors.push(invalid_reset_value_literal(&candidate.token));
                }
                // The counterpart is reported as an error by other checks.
                Resolved::Unresolved => {}
            }
        }

        errors
    }
}

pub fn invalid_reset_value_literal(token: &Token) -> AnalyzerError {
    AnalyzerError::invalid_factor(
        Some(&token.to_string()),
        "reset value literal whose polarity can't be inferred",
        &token.into(),
        &[],
    )
}

/// Kind of a single reset signal whose type is declared directly by a reset keyword.
fn reset_type(r#type: &Type, select: usize) -> Option<TypeKind> {
    let is_reset =
        r#type.kind.is_reset() && r#type.width.is_empty() && r#type.array.len() == select;
    is_reset.then(|| r#type.kind.clone())
}

fn to_resolved(r#type: Option<&Type>, select: usize) -> Resolved {
    match r#type.and_then(|x| reset_type(x, select)) {
        Some(x) => Resolved::Valid(x),
        None => Resolved::Invalid,
    }
}

fn resolve_variable(path: &GenericSymbolPath, select: usize) -> Resolved {
    let namespace = SymbolPathNamespace::from_token(path.generic_path(), path.paths[0].base.id);
    let Ok(symbol) = symbol_table::resolve(namespace) else {
        return Resolved::Unresolved;
    };

    match &symbol.found.kind {
        SymbolKind::Variable(x) => to_resolved(Some(&x.r#type), select),
        SymbolKind::Port(x) => to_resolved(Some(&x.r#type), select),
        SymbolKind::StructMember(x) => to_resolved(Some(&x.r#type), select),
        SymbolKind::UnionMember(x) => to_resolved(Some(&x.r#type), select),
        SymbolKind::ModportVariableMember(x) => {
            let Some(variable) = symbol_table::get(x.variable) else {
                return Resolved::Unresolved;
            };
            to_resolved(variable.kind.get_type(), select)
        }
        _ => Resolved::Invalid,
    }
}

fn resolve_inst_port(inst_type: &GenericSymbolPath, port: StrId) -> Resolved {
    let namespace =
        SymbolPathNamespace::from_token(inst_type.generic_path(), inst_type.paths[0].base.id);
    // A generic parameter is resolved to its proto module, whose ports
    // match those of the actual module.
    let Some(symbol) = resolve_inst_type(&InstTypeSource::Path(namespace)) else {
        return Resolved::Unresolved;
    };

    match &symbol.kind {
        SymbolKind::Module(x) => {
            let Some(port) = x.ports.iter().find(|x| x.name() == port) else {
                return Resolved::Unresolved;
            };
            let property = port.property();
            if property.direction != Direction::Input {
                return Resolved::Invalid;
            }
            to_resolved(Some(&property.r#type), 0)
        }
        // The port type of SystemVerilog module and testbench component is unknown.
        SymbolKind::SystemVerilog | SymbolKind::TbComponent(_) => Resolved::Invalid,
        _ => Resolved::Unresolved,
    }
}

thread_local!(static RESET_VALUE_TABLE: RefCell<ResetValueTable> = RefCell::new(ResetValueTable::new()));

pub fn add(candidate: ResetValueCandidate) {
    RESET_VALUE_TABLE.with(|f| f.borrow_mut().add(candidate))
}

/// Returns the current number of pending candidates. Used as a watermark
/// to delimit one file's pass1 additions for fragment caching.
pub fn candidates_len() -> usize {
    RESET_VALUE_TABLE.with(|f| f.borrow().candidates.len())
}

/// Exports the candidates added since the given watermark.
pub fn export_candidates_since(watermark: usize) -> Vec<ResetValueCandidate> {
    RESET_VALUE_TABLE.with(|f| f.borrow().candidates[watermark..].to_vec())
}

pub fn apply() -> Vec<AnalyzerError> {
    RESET_VALUE_TABLE.with(|f| f.borrow_mut().apply())
}

/// Returns the reset type of the counterpart of the given reset value literal token.
/// `TypeKind::Reset` means the polarity follows the build option.
pub fn get(id: TokenId) -> Option<TypeKind> {
    RESET_VALUE_TABLE.with(|f| f.borrow().table.get(&id).cloned())
}

pub fn dump() -> String {
    RESET_VALUE_TABLE.with(|f| {
        let table = &f.borrow().table;
        let mut entries: Vec<_> = table.iter().collect();
        entries.sort_by_key(|(id, _)| **id);
        entries
            .iter()
            .map(|(id, kind)| format!("{id}: {kind:?}\n"))
            .collect()
    })
}

pub fn clear() {
    RESET_VALUE_TABLE.with(|f| *f.borrow_mut() = ResetValueTable::new())
}
