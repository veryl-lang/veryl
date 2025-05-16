use crate::AnalyzerError;
use crate::namespace::Namespace;
use crate::symbol::{ParameterKind, Symbol, SymbolId, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use crate::{HashMap, HashSet};
use bimap::BiMap;
use daggy::petgraph::visit::Dfs;
use daggy::{Dag, Walker, petgraph::algo};
use std::cell::RefCell;
use veryl_parser::veryl_token::Token;

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum TypeDagCandidate {
    Path {
        path: GenericSymbolPath,
        namespace: Namespace,
        parent: Option<(SymbolId, Context)>,
    },
    Symbol {
        id: SymbolId,
        context: Context,
        parent: Option<(SymbolId, Context)>,
        import: Vec<SymbolPathNamespace>,
    },
}

impl TypeDagCandidate {
    pub fn set_parent(&mut self, x: (SymbolId, Context)) {
        match self {
            TypeDagCandidate::Path { parent, .. } => {
                *parent = Some(x);
            }
            TypeDagCandidate::Symbol { parent, .. } => {
                *parent = Some(x);
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct TypeDag {
    dag: Dag<(), Context, u32>,
    /// One-to-one relation between SymbolId and DAG NodeIdx
    nodes: BiMap<SymbolId, u32>,
    /// Map between NodeIdx and Symbol Resolve Information
    paths: HashMap<u32, TypeResolveInfo>,
    symbols: HashMap<u32, Symbol>,
    source: u32,
    candidates: Vec<TypeDagCandidate>,
    errors: Vec<DagError>,
    dag_owned: HashMap<u32, Vec<u32>>,
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

impl From<DagError> for AnalyzerError {
    fn from(value: DagError) -> Self {
        let DagError::Cyclic(s, e) = value;
        AnalyzerError::cyclic_type_dependency(
            &s.token.to_string(),
            &e.token.to_string(),
            &e.token.into(),
        )
    }
}

impl TypeDag {
    #[allow(dead_code)]
    fn new() -> Self {
        let mut dag = Dag::<(), Context, u32>::new();
        let source = dag.add_node(()).index() as u32;
        Self {
            dag,
            nodes: BiMap::new(),
            paths: HashMap::default(),
            symbols: HashMap::default(),
            source,
            candidates: Vec::new(),
            errors: Vec::new(),
            dag_owned: HashMap::default(),
        }
    }

    fn add(&mut self, cand: TypeDagCandidate) {
        self.candidates.push(cand);
    }

    fn apply(&mut self) -> Vec<AnalyzerError> {
        let candidates: Vec<_> = self.candidates.drain(..).collect();

        // Process symbol declarations at first to construct dag_owned
        for cand in &candidates {
            if let TypeDagCandidate::Symbol {
                id,
                context,
                parent,
                import,
            } = cand
            {
                if let Some(symbol) = symbol_table::get(*id) {
                    if let Some(child) = self.insert_declaration_symbol(&symbol, parent) {
                        for x in import {
                            if let Ok(import) = symbol_table::resolve(x) {
                                if let Some(import) = self.insert_symbol(&import.found) {
                                    self.insert_dag_edge(child, import, *context);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Process symbol references using dag_owned
        for cand in &candidates {
            if let TypeDagCandidate::Path {
                path,
                namespace,
                parent,
            } = cand
            {
                self.insert_path(path, namespace, parent);
            }
        }

        self.errors.drain(..).map(|x| x.into()).collect()
    }

    fn insert_path(
        &mut self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
        parent: &Option<(SymbolId, Context)>,
    ) {
        if path.is_generic_reference() {
            return;
        }

        let mut path = path.clone();
        path.resolve_imported(namespace);

        for i in 0..path.len() {
            let base_path = path.base_path(i);

            if let Ok(symbol) = symbol_table::resolve((&base_path, namespace)) {
                let base = if let Some(x) = symbol.found.get_parent_package() {
                    x
                } else {
                    symbol.found
                };

                let generic_args: Vec<_> = path.paths[i]
                    .arguments
                    .iter()
                    .filter_map(|x| {
                        symbol_table::resolve((&x.generic_path(), namespace))
                            .map(|symbol| {
                                if let Some(x) = symbol.found.get_parent_package() {
                                    x
                                } else {
                                    symbol.found
                                }
                            })
                            .ok()
                    })
                    .collect();

                if let Some(base) = self.insert_symbol(&base) {
                    if let Some(parent) = parent {
                        let parent_symbol = symbol_table::get(parent.0).unwrap();
                        let parent_context = parent.1;
                        if let Some(parent) = self.insert_symbol(&parent_symbol) {
                            if !self.is_dag_owned(parent, base) {
                                self.insert_dag_edge(parent, base, parent_context);
                            }
                        }
                    }

                    for arg in generic_args {
                        if let Some(arg) = self.insert_symbol(&arg) {
                            self.insert_dag_edge(base, arg, Context::GenericInstance);
                        }
                    }
                }
            }
        }
    }

    fn insert_symbol(&mut self, symbol: &Symbol) -> Option<u32> {
        let is_dag_symbol = match symbol.kind {
            SymbolKind::Module(_)
            | SymbolKind::Interface(_)
            | SymbolKind::Modport(_)
            | SymbolKind::Package(_)
            | SymbolKind::Enum(_)
            | SymbolKind::TypeDef(_)
            | SymbolKind::Struct(_)
            | SymbolKind::Union(_)
            | SymbolKind::Function(_) => true,
            SymbolKind::Parameter(ref x) => matches!(x.kind, ParameterKind::Const),
            _ => false,
        };
        if !is_dag_symbol {
            return None;
        }

        let name = symbol.token.to_string();
        match self.insert_node(symbol.id, &name) {
            Ok(n) => Some(n),
            Err(x) => {
                self.errors.push(x);
                None
            }
        }
    }

    fn insert_declaration_symbol(
        &mut self,
        symbol: &Symbol,
        parent: &Option<(SymbolId, Context)>,
    ) -> Option<u32> {
        if let Some(child) = self.insert_symbol(symbol) {
            if let Some(parent) = parent {
                let parent_symbol = symbol_table::get(parent.0).unwrap();
                let parent_context = parent.1;
                if let Some(parent) = self.insert_symbol(&parent_symbol) {
                    self.insert_dag_owned(parent, child);
                    self.insert_dag_edge(child, parent, parent_context);
                }
            }
            Some(child)
        } else {
            None
        }
    }

    fn insert_dag_edge(&mut self, parent: u32, child: u32, context: Context) {
        // Reversing this order to make traversal work
        if let Err(x) = self.insert_edge(child, parent, context) {
            self.errors.push(x);
        }
    }

    fn insert_dag_owned(&mut self, parent: u32, child: u32) {
        // If there is already edge to owned type, remove it.
        // Argument order should be the same as insert_edge.
        if self.exist_edge(child, parent) {
            self.remove_edge(child, parent);
        }
        self.dag_owned
            .entry(parent)
            .and_modify(|x| x.push(child))
            .or_insert(vec![child]);
    }

    fn is_dag_owned(&self, parent: u32, child: u32) -> bool {
        if let Some(owned) = self.dag_owned.get(&parent) {
            owned.contains(&child)
        } else {
            false
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
                // Direct recursion of module/interface/function is allowed
                let is_allowed_direct_recursion = matches!(
                    edge,
                    Context::Module | Context::Interface | Context::Function
                ) && start == end;
                if is_allowed_direct_recursion {
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
        let mut nodes = algo::toposort(self.dag.graph(), None).unwrap();
        let mut ret = "".to_string();

        nodes.sort_by_key(|x| {
            let idx = x.index() as u32;
            if let Some(path) = self.paths.get(&idx) {
                path.name.clone()
            } else {
                "".to_string()
            }
        });

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
                let mut set = HashSet::default();
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

pub fn add(cand: TypeDagCandidate) {
    TYPE_DAG.with(|f| f.borrow_mut().add(cand))
}

pub fn apply() -> Vec<AnalyzerError> {
    TYPE_DAG.with(|f| f.borrow_mut().apply())
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
