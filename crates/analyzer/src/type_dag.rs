use crate::symbol::{Symbol, SymbolId};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use bimap::BiMap;
use daggy::petgraph::unionfind::UnionFind;
use daggy::petgraph::visit::{EdgeRef, NodeIndexable};
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
    pub path: SymbolPathNamespace,
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
    TypeDef,
    Module,
    Interface,
    Package,
    Modport,
    ExpressionIdentifier,
    GenericInstance,
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
            nodes: BiMap::new(),
            paths: HashMap::new(),
            symbols: HashMap::new(),
            source,
        }
    }

    fn insert_node(
        &mut self,
        path: &SymbolPathNamespace,
        id: &str,
        token: &Token,
    ) -> Result<u32, DagError> {
        let trinfo = TypeResolveInfo {
            path: path.clone(),
            name: id.into(),
            token: *token,
        };
        let sym = match symbol_table::resolve(&trinfo.path) {
            Ok(rr) => rr.found,
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
                self.symbols.insert(node_idx, sym);
                Ok(node_idx)
            }
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
        let graph = self.dag.graph();
        let mut vertex_sets = UnionFind::new(graph.node_bound());
        for edge in graph.edge_references() {
            let (a, b) = (edge.source(), edge.target());

            // Ignore Index0 because it is root node
            if a.index() != 0 {
                vertex_sets.union(graph.to_index(a), graph.to_index(b));
            }
        }
        let labels = vertex_sets.into_labeling();

        let mut ret = HashMap::new();
        for node in graph.node_indices() {
            let label = labels[graph.to_index(node)];
            let index = node.index() as u32;

            if self.paths.contains_key(&index) {
                let sym = self.get_symbol(index);
                ret.entry(label)
                    .and_modify(|x: &mut Vec<_>| x.push(sym.clone()))
                    .or_insert(vec![sym]);
            }
        }
        ret.into_values().collect()
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

pub fn insert_node(start: &SymbolPathNamespace, id: &str, token: &Token) -> Result<u32, DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_node(start, id, token))
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
