use crate::{
    symbol::{Symbol, SymbolId},
    symbol_table::{self, ResolveSymbol, SymbolPathNamespace},
};
use bimap::BiMap;
use std::{cell::RefCell, collections::HashMap, collections::HashSet};

use daggy::{petgraph::algo, Dag, Walker};
use veryl_parser::veryl_token::VerylToken;

#[derive(Default)]
pub struct TypeDag {
    dag: Dag<(), Context, u32>,
    /// One-to-one relation between SymbolId and DAG NodeIdx
    nodes: BiMap<SymbolId, u32>,
    /// Map between NodeIdx and Symbol Resolve Information
    paths: HashMap<u32, TypeResolveInfo>,
    source: u32,
}

#[derive(Clone, Debug)]
pub struct TypeResolveInfo {
    pub path: SymbolPathNamespace,
    pub name: String,
    pub token: VerylToken,
}

#[derive(Default, Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq)]
pub enum Context {
    #[default]
    Irrelevant,
    Struct,
    Union,
    Enum,
    TypeDef,
    Module,
    Interface,
    Package,
}

#[derive(Debug, Clone)]
pub enum DagError {
    Cyclic(Symbol, Symbol),
    UnableToResolve(Box<TypeResolveInfo>),
}

impl TypeDag {
    #[allow(dead_code)]
    fn new() -> Self {
        let mut dag = Dag::<(), Context, u32>::new();
        let source = dag.add_node(()).index() as u32;
        Self {
            dag,
            nodes: BiMap::<SymbolId, u32>::new(),
            paths: HashMap::<u32, TypeResolveInfo>::new(),
            source,
        }
    }

    fn insert_node(
        &mut self,
        path: &SymbolPathNamespace,
        id: &str,
        token: &VerylToken,
    ) -> Result<u32, DagError> {
        let trinfo = TypeResolveInfo {
            path: path.clone(),
            name: id.into(),
            token: token.clone(),
        };
        let sym = match symbol_table::resolve(&trinfo.path) {
            Ok(rr) => match rr.found {
                ResolveSymbol::Symbol(symbol) => symbol,
                ResolveSymbol::External => {
                    let e = DagError::UnableToResolve(Box::new(trinfo));
                    return Err(e);
                }
            },
            Err(_) => {
                let e = DagError::UnableToResolve(Box::new(trinfo));
                return Err(e);
            }
        };
        match self.nodes.get_by_left(&sym.id) {
            Some(node_idx) => Ok(*node_idx),
            None => {
                let node_idx = self.dag.add_node(()).index() as u32;
                self.insert_edge(self.source, node_idx, Context::Irrelevant)?;
                self.nodes.insert(sym.id, node_idx);
                self.paths.insert(node_idx, trinfo);
                Ok(node_idx)
            }
        }
    }

    fn get_symbol(&self, node: u32) -> Symbol {
        match self.paths.get(&node) {
            Some(TypeResolveInfo { path, .. }) => match symbol_table::resolve(path) {
                Ok(rr) => match rr.found {
                    ResolveSymbol::Symbol(symbol) => symbol,
                    ResolveSymbol::External => unreachable!(),
                },
                Err(_) => {
                    unreachable!();
                }
            },
            None => {
                panic!("Must insert node before accessing");
            }
        }
    }

    fn insert_edge(&mut self, start: u32, end: u32, edge: Context) -> Result<(), DagError> {
        match self.dag.add_edge(start.into(), end.into(), edge) {
            Ok(_) => Ok(()),
            Err(_) => {
                // Direct recursion of module/interface is allowed
                let is_direct_recursion = start == end;
                if matches!(edge, Context::Module | Context::Interface) && is_direct_recursion {
                    Ok(())
                } else {
                    let ssym = self.get_symbol(start);
                    let esym = self.get_symbol(end);
                    Err(DagError::Cyclic(ssym, esym))
                }
            }
        }
    }

    fn toposort(&self) -> Vec<VerylToken> {
        let nodes = algo::toposort(self.dag.graph(), None).unwrap();
        let mut ret = vec![];
        for node in nodes {
            if let Some(path) = self.paths.get(&(node.index() as u32)) {
                ret.push(path.token.clone());
            }
        }
        ret
    }

    fn dump(&self) -> String {
        let nodes = algo::toposort(self.dag.graph(), None).unwrap();
        let mut ret = "".to_string();
        for node in nodes {
            if let Some(path) = self.paths.get(&(node.index() as u32)) {
                ret.push_str(&format!("{}\n", path.name,));
                let mut set = HashSet::new();
                for parent in self.dag.parents(node).iter(&self.dag) {
                    let node = parent.1.index() as u32;
                    if !set.contains(&node) {
                        set.insert(node);
                        if let Some(path) = self.paths.get(&node) {
                            ret.push_str(&format!(" |- {}\n", path.name));
                        }
                    }
                }
            }
        }
        ret
    }
}

thread_local!(static TYPE_DAG: RefCell<TypeDag> = RefCell::new(TypeDag::new()));

pub fn insert_edge(start: u32, end: u32, context: Context) -> Result<(), DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_edge(start, end, context))
}

pub fn insert_node(
    start: &SymbolPathNamespace,
    id: &str,
    token: &VerylToken,
) -> Result<u32, DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_node(start, id, token))
}

pub fn get_symbol(node: u32) -> Symbol {
    TYPE_DAG.with(|f| f.borrow().get_symbol(node))
}

pub fn toposort() -> Vec<VerylToken> {
    TYPE_DAG.with(|f| f.borrow().toposort())
}

pub fn dump() -> String {
    TYPE_DAG.with(|f| f.borrow().dump())
}
