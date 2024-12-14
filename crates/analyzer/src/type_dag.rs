use crate::symbol::{Symbol, SymbolId};
use crate::symbol_table;
use bimap::BiMap;
use daggy::petgraph::visit::Dfs;
use daggy::{petgraph::algo, Dag, Walker};
use std::{cell::RefCell, collections::HashMap, collections::HashSet};
use veryl_parser::veryl_token::Token;

#[derive(Clone, Default)]
pub struct TypeDag {
    dag: Dag<(), Context, u32>,
    /// One-to-one relation between SymbolId and DAG NodeIdx
    nodes: BiMap<SymbolId, u32>,
    /// Map between NodeIdx and Symbol Resolve Information
    paths: HashMap<u32, TypeResolveInfo>,
    symbols: HashMap<u32, Symbol>,
    source: u32,
}

#[derive(Clone, Debug)]
pub struct TypeResolveInfo {
    pub symbol_id: SymbolId,
    pub name: String,
    pub token: Token,
}

#[derive(Default, Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq)]
pub enum Context {
    #[default]
    Irrelevant,
    Struct,
    Union,
    Enum,
    Function,
    TypeDef,
    Const,
    Module,
    Interface,
    Package,
    Modport,
    GenericInstance,
}

#[derive(Debug, Clone)]
pub enum DagError {
    Cyclic(Symbol, Symbol),
}

impl TypeDag {
    #[allow(dead_code)]
    fn new() -> Self {
        let mut dag = Dag::<(), Context, u32>::new();
        let source = dag.add_node(()).index() as u32;
        Self {
            dag,
            nodes: BiMap::new(),
            paths: HashMap::new(),
            symbols: HashMap::new(),
            source,
        }
    }

    fn insert_node(&mut self, symbol_id: SymbolId, name: &str) -> Result<u32, DagError> {
        let symbol = symbol_table::get(symbol_id).unwrap();
        let trinfo = TypeResolveInfo {
            symbol_id,
            name: name.into(),
            token: symbol.token,
        };
        if let Some(node_index) = self.nodes.get_by_left(&symbol_id) {
            Ok(*node_index)
        } else {
            let node_index = self.dag.add_node(()).index() as u32;
            self.insert_edge(self.source, node_index, Context::Irrelevant)?;
            self.nodes.insert(symbol_id, node_index);
            self.paths.insert(node_index, trinfo);
            self.symbols.insert(node_index, symbol);
            Ok(node_index)
        }
    }

    fn get_symbol(&self, node: u32) -> Symbol {
        match self.symbols.get(&node) {
            Some(x) => x.clone(),
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

    fn exist_edge(&self, start: u32, end: u32) -> bool {
        self.dag.find_edge(start.into(), end.into()).is_some()
    }

    fn remove_edge(&mut self, start: u32, end: u32) {
        while let Some(x) = self.dag.find_edge(start.into(), end.into()) {
            self.dag.remove_edge(x);
        }
    }

    fn toposort(&self) -> Vec<Symbol> {
        let nodes = algo::toposort(self.dag.graph(), None).unwrap();
        let mut ret = vec![];
        for node in nodes {
            let index = node.index() as u32;
            if self.paths.contains_key(&index) {
                let sym = self.get_symbol(index);
                ret.push(sym);
            }
        }
        ret
    }

    fn connected_components(&self) -> Vec<Vec<Symbol>> {
        let mut ret = Vec::new();
        let mut graph = self.dag.graph().clone();

        // Reverse edge to traverse nodes which are called from parent node
        graph.reverse();

        for node in self.symbols.keys() {
            let mut connected = Vec::new();
            let mut dfs = Dfs::new(&graph, (*node).into());
            while let Some(x) = dfs.next(&graph) {
                let index = x.index() as u32;
                if self.paths.contains_key(&index) {
                    let symbol = self.get_symbol(index);
                    connected.push(symbol);
                }
            }
            if !connected.is_empty() {
                ret.push(connected);
            }
        }

        ret
    }

    fn dump(&self) -> String {
        let nodes = algo::toposort(self.dag.graph(), None).unwrap();
        let mut ret = "".to_string();

        let mut max_width = 0;
        for node in &nodes {
            let idx = node.index() as u32;
            if let Some(path) = self.paths.get(&idx) {
                max_width = max_width.max(path.name.len());
                for parent in self.dag.parents(*node).iter(&self.dag) {
                    let node = parent.1.index() as u32;
                    if let Some(path) = self.paths.get(&node) {
                        max_width = max_width.max(path.name.len() + 4);
                    }
                }
            }
        }

        for node in &nodes {
            let idx = node.index() as u32;
            if let Some(path) = self.paths.get(&idx) {
                let symbol = self.symbols.get(&idx).unwrap();
                ret.push_str(&format!(
                    "{}{} : {}\n",
                    path.name,
                    " ".repeat(max_width - path.name.len()),
                    symbol.kind
                ));
                let mut set = HashSet::new();
                for parent in self.dag.parents(*node).iter(&self.dag) {
                    let node = parent.1.index() as u32;
                    if !set.contains(&node) {
                        set.insert(node);
                        if let Some(path) = self.paths.get(&node) {
                            let symbol = self.symbols.get(&node).unwrap();
                            ret.push_str(&format!(
                                " |- {}{} : {}\n",
                                path.name,
                                " ".repeat(max_width - path.name.len() - 4),
                                symbol.kind
                            ));
                        }
                    }
                }
            }
        }
        ret
    }

    fn clear(&mut self) {
        self.clone_from(&Self::new());
    }
}

thread_local!(static TYPE_DAG: RefCell<TypeDag> = RefCell::new(TypeDag::new()));

pub fn insert_edge(start: u32, end: u32, context: Context) -> Result<(), DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_edge(start, end, context))
}

pub fn exist_edge(start: u32, end: u32) -> bool {
    TYPE_DAG.with(|f| f.borrow().exist_edge(start, end))
}

pub fn remove_edge(start: u32, end: u32) {
    TYPE_DAG.with(|f| f.borrow_mut().remove_edge(start, end))
}

pub fn insert_node(symbol_id: SymbolId, name: &str) -> Result<u32, DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_node(symbol_id, name))
}

pub fn get_symbol(node: u32) -> Symbol {
    TYPE_DAG.with(|f| f.borrow().get_symbol(node))
}

pub fn toposort() -> Vec<Symbol> {
    TYPE_DAG.with(|f| f.borrow().toposort())
}

pub fn connected_components() -> Vec<Vec<Symbol>> {
    TYPE_DAG.with(|f| f.borrow().connected_components())
}

pub fn dump() -> String {
    TYPE_DAG.with(|f| f.borrow().dump())
}

pub fn clear() {
    TYPE_DAG.with(|f| f.borrow_mut().clear())
}
