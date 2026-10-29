//! The analysis of each unit of the unit cache. A replayed module has no IR
//! body, so its results and those of the modules below it are replayed in
//! the order analyzing them would produce.

use super::coarse::CoarseSummary;
use super::hierarchy;
use super::model::ModuleCombSummary;
use crate::conv::unit_cache;
use crate::ir::{Ir, Module, Signature, VarPath};
use crate::symbol_table;
use crate::{AnalyzerError, HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct UnitAnalysis {
    /// The module's symbol, which identifies a cycle report across signatures.
    pub(super) symbol: String,
    /// In declaration order, which the traversal follows.
    pub(super) children: Vec<String>,
    pub(super) coarse: Option<CoarseSummary>,
    pub(super) cleared: bool,
    pub(super) complete: bool,
    /// Built when the module itself or a parent needed it, with what
    /// building it reported.
    pub(super) precise: Option<(ModuleCombSummary, Vec<AnalyzerError>)>,
    /// Every cycle found in the module, also those another signature of the
    /// same symbol reported first.
    pub(super) cycles: Vec<(Vec<VarPath>, AnalyzerError)>,
}

pub(super) fn get(key: &str) -> Option<UnitAnalysis> {
    unit_cache::analysis(key)
}

pub(super) fn update(key: &str, f: impl FnOnce(&mut UnitAnalysis)) {
    unit_cache::update_analysis(key, f);
}

thread_local!(static KEYS: RefCell<HashMap<Signature, String>> = RefCell::new(HashMap::default()));

pub(super) fn key_of(signature: &Signature) -> String {
    KEYS.with(|x| {
        x.borrow_mut()
            .entry(signature.clone())
            .or_insert_with(|| signature.stable_key())
            .clone()
    })
}

pub(super) fn clear_keys() {
    KEYS.with(|x| x.borrow_mut().clear());
    SIGNATURES.with(|x| x.borrow_mut().clear());
}

thread_local!(static SIGNATURES: RefCell<HashMap<String, Signature>> = RefCell::new(HashMap::default()));

/// Indexes the modules of `ir` by key, so a module first reached below a
/// replayed one still has the signature its parents in the IR look it up by.
fn index_signatures(ir: &Ir) {
    let mut work: Vec<&Module> = ir
        .components
        .iter()
        .filter_map(|x| match x {
            crate::ir::Component::Module(x) => Some(x),
            _ => None,
        })
        .collect();
    let mut visited = HashSet::default();
    while let Some(module) = work.pop() {
        if !visited.insert(&module.signature) {
            continue;
        }
        let key = key_of(&module.signature);
        SIGNATURES.with(|x| x.borrow_mut().insert(key, module.signature.clone()));
        work.extend(hierarchy::child_modules(module));
    }
}

/// Identity of a module in the traversal: its signature, or its stable key
/// while the unit cache is in use, which a replayed module has alone.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum Ident {
    Signature(Signature),
    Key(String),
}

#[derive(Clone)]
pub(super) enum Node<'a> {
    Live(&'a Module),
    Cached(Rc<Cached>),
}

pub(super) struct Cached {
    pub(super) key: String,
    /// The signature the module's parents in the IR look its summary up by,
    /// if one has it.
    pub(super) signature: Option<Signature>,
    /// Keys from the replayed module in the IR down to this one, to convert
    /// again if a result is missing.
    pub(super) chain: Vec<String>,
    pub(super) analysis: Option<UnitAnalysis>,
}

impl<'a> Node<'a> {
    pub(super) fn of(module: &'a Module) -> Self {
        match unit_cache::stub_key(&module.signature) {
            Some(key) => Node::Cached(Rc::new(Cached {
                analysis: get(&key),
                chain: vec![key.clone()],
                key,
                signature: Some(module.signature.clone()),
            })),
            None => Node::Live(module),
        }
    }

    pub(super) fn ident(&self) -> Ident {
        match self {
            Node::Live(module) if unit_cache::is_active() => Ident::Key(key_of(&module.signature)),
            Node::Live(module) => Ident::Signature(module.signature.clone()),
            Node::Cached(x) => Ident::Key(x.key.clone()),
        }
    }

    pub(super) fn children(&self) -> Vec<Node<'a>> {
        match self {
            Node::Live(module) => hierarchy::child_modules(module).map(Node::of).collect(),
            Node::Cached(x) => x
                .analysis
                .iter()
                .flat_map(|analysis| &analysis.children)
                .map(|key| {
                    let mut chain = x.chain.clone();
                    chain.push(key.clone());
                    Node::Cached(Rc::new(Cached {
                        key: key.clone(),
                        signature: SIGNATURES.with(|x| x.borrow().get(key).cloned()),
                        chain,
                        analysis: get(key),
                    }))
                })
                .collect(),
        }
    }
}

impl Cached {
    /// The analysis, or `None` after asking for the chain to be converted.
    pub(super) fn analysis(&self) -> Option<&UnitAnalysis> {
        if self.analysis.is_none() {
            unit_cache::request_conversion(&self.chain);
        }
        self.analysis.as_ref()
    }
}

/// The instantiated specializations in children-before-parents order, also
/// below replayed modules. Unevaluable generic templates are not stable
/// bodies and therefore do not claim the same signature as a concrete
/// default specialization.
pub(super) fn postorder(ir: &Ir) -> Vec<Node<'_>> {
    fn visit<'a>(
        node: Node<'a>,
        visited: &mut HashSet<Ident>,
        active: &mut HashSet<Ident>,
        order: &mut Vec<Node<'a>>,
    ) {
        if let Node::Live(module) = &node
            && module.suppress_unassigned
        {
            return;
        }
        let ident = node.ident();
        if visited.contains(&ident) || !active.insert(ident.clone()) {
            return;
        }
        for child in node.children() {
            visit(child, visited, active, order);
        }
        active.remove(&ident);
        visited.insert(ident);
        order.push(node);
    }

    if unit_cache::is_active() {
        index_signatures(ir);
    }
    let mut visited = HashSet::default();
    let mut active = HashSet::default();
    let mut order = Vec::new();
    for component in &ir.components {
        if let crate::ir::Component::Module(module) = component {
            visit(Node::of(module), &mut visited, &mut active, &mut order);
        }
    }
    order
}

pub(super) fn symbol_name(module: &Module) -> String {
    let symbol = symbol_table::get(module.signature.symbol).unwrap();
    format!("{}::{}", symbol.namespace, symbol.token.text)
}
