use crate::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{GenericMap, ParameterKind, Symbol, SymbolId, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, GenericSymbolPathNamesapce, SymbolPath};
use crate::symbol_table;
use crate::{HashMap, HashSet};
use bimap::BiMap;
use daggy::petgraph::visit::Dfs;
use daggy::{Dag, NodeIndex, Walker, petgraph::algo};
use std::cell::RefCell;
use veryl_parser::resource_table::PathId;
use veryl_parser::veryl_token::Token;

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum TypeDagCandidate {
    Path {
        path: GenericSymbolPath,
        namespace: Namespace,
        project_namespace: Namespace,
        parent: Option<(SymbolId, Context)>,
    },
    Symbol {
        id: SymbolId,
        context: Context,
        parent: Option<(SymbolId, Context)>,
        import: Vec<GenericSymbolPathNamesapce>,
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
    file_dag: Dag<(), (), u32>,
    file_nodes: BiMap<PathId, u32>,
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
    Alias,
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
            file_dag: Dag::new(),
            file_nodes: BiMap::new(),
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
                && let Some(symbol) = symbol_table::get(*id)
                && let Some(child) = self.insert_declaration_symbol(&symbol, parent)
            {
                for x in import {
                    if let Ok(import) = symbol_table::resolve(x)
                        && let Some(import) = self.insert_symbol(&import.found)
                    {
                        self.insert_dag_edge(child, import, *context);
                    }
                }
            }
        }

        // Process symbol references using dag_owned
        for cand in &candidates {
            if let TypeDagCandidate::Path {
                path,
                namespace,
                project_namespace,
                parent,
            } = cand
            {
                self.insert_path(path, namespace, project_namespace, parent);
            }
        }

        self.errors.drain(..).map(|x| x.into()).collect()
    }

    fn insert_path(
        &mut self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
        project_namespace: &Namespace,
        parent: &Option<(SymbolId, Context)>,
    ) {
        namespace_table::set_default(&project_namespace.paths);
        if let Some((parent_id, parent_context)) = parent {
            let parent_symbol = symbol_table::get(*parent_id).unwrap();
            let parent_package = parent_symbol.get_parent_package();
            for generic_map in parent_symbol.generic_maps() {
                self.insert_path_with_generic_map(
                    path,
                    namespace,
                    Some((&parent_symbol, parent_context)),
                    parent_package.as_ref(),
                    Some(generic_map),
                );
            }
        } else {
            self.insert_path_with_generic_map(path, namespace, None, None, None);
        }
    }

    fn insert_path_with_generic_map(
        &mut self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
        parent: Option<(&Symbol, &Context)>,
        parent_package: Option<&Symbol>,
        generic_map: Option<GenericMap>,
    ) {
        let mut path = path.clone();

        let maps = generic_map.map(|map| vec![map]);
        path.resolve_imported(namespace, maps.as_ref());
        if let Some(maps) = maps.as_ref() {
            path.apply_map(maps);
        }

        for i in 0..path.len() {
            let Some(base_symbol) = Self::resolve_symbol_path(&path.base_path(i), namespace) else {
                continue;
            };
            let Some(base) = self.insert_symbol(&base_symbol) else {
                continue;
            };

            if let Some((parent_symbol, parent_context)) = parent {
                let (parent_symbol, parent_context) = if parent_package
                    .map(|x| x.id != base_symbol.id)
                    .unwrap_or(false)
                {
                    (parent_package.unwrap(), Context::Package)
                } else {
                    (parent_symbol, *parent_context)
                };
                if let Some(parent) = self.insert_symbol(parent_symbol)
                    && !self.is_dag_owned(parent, base)
                {
                    self.insert_dag_edge(parent, base, parent_context);
                }
            }

            for arg in path.paths[i]
                .arguments
                .iter()
                .filter_map(|x| Self::resolve_symbol_path(&x.generic_path(), namespace))
            {
                let Some(arg) = self.insert_symbol(&arg) else {
                    continue;
                };
                self.insert_dag_edge(base, arg, Context::GenericInstance);
            }
        }
    }

    fn resolve_symbol_path(path: &SymbolPath, namespace: &Namespace) -> Option<Symbol> {
        let symbol = symbol_table::resolve((path, namespace)).ok()?;
        let symbol = if let Some(alias_path) = symbol.found.alias_target() {
            // alias referenced as generic arg for generic instance put on the same namespace
            // causes cyclic dependency error.
            // https://github.com/veryl-lang/veryl/blob/52b46337148340b43f8ab1c8f2ab67f58cd3c943/crates/analyzer/src/tests.rs#L3740-L3743
            // Need to use the target symbol of the alias instead of it to prevent this situation.
            Self::resolve_symbol_path(&alias_path.generic_path(), &symbol.found.namespace)?
        } else {
            symbol.found
        };

        if let Some(pacakge) = symbol.get_parent_package() {
            Some(pacakge)
        } else {
            Some(symbol)
        }
    }

    fn insert_symbol(&mut self, symbol: &Symbol) -> Option<u32> {
        let is_dag_symbol = match symbol.kind {
            SymbolKind::Module(_)
            | SymbolKind::AliasModule(_)
            | SymbolKind::Interface(_)
            | SymbolKind::AliasInterface(_)
            | SymbolKind::Modport(_)
            | SymbolKind::Package(_)
            | SymbolKind::AliasPackage(_)
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

    #[allow(clippy::result_large_err)]
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
            if let Some(path) = symbol.token.source.get_path() {
                let file_node = self.file_dag.add_node(()).index() as u32;
                self.file_nodes.insert(path, file_node);
            }

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

    #[allow(clippy::result_large_err)]
    fn insert_edge(&mut self, start: u32, end: u32, edge: Context) -> Result<(), DagError> {
        match self.dag.add_edge(start.into(), end.into(), edge) {
            Ok(_) => {
                self.insert_file_edge(start, end);
                Ok(())
            }
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

    fn insert_file_edge(&mut self, start: u32, end: u32) {
        if start != self.source {
            let start = self.get_symbol(start);
            let end = self.get_symbol(end);
            if let (Some(start), Some(end)) =
                (start.token.source.get_path(), end.token.source.get_path())
            {
                let start = self.file_nodes.get_by_left(&start).unwrap();
                let end = self.file_nodes.get_by_left(&end).unwrap();
                let start: NodeIndex = (*start).into();
                let end: NodeIndex = (*end).into();
                if start != end && self.file_dag.find_edge(start, end).is_none() {
                    let err = self.file_dag.add_edge(start, end, ());
                    // cyclic error should be caught by dag
                    err.unwrap();
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

    fn dependent_files(&self) -> HashMap<PathId, Vec<PathId>> {
        let mut ret = HashMap::default();
        let graph = self.file_dag.graph().clone();

        for node in self.file_nodes.right_values() {
            let mut dependents = Vec::new();
            let mut dfs = Dfs::new(&graph, (*node).into());
            while let Some(x) = dfs.next(&graph) {
                let index = x.index() as u32;
                if index != *node
                    && let Some(x) = self.file_nodes.get_by_right(&index)
                {
                    dependents.push(*x);
                }
            }
            if !dependents.is_empty()
                && let Some(x) = self.file_nodes.get_by_right(node)
            {
                ret.insert(*x, dependents);
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

    fn dump_file(&self) -> String {
        let nodes = algo::toposort(self.file_dag.graph(), None).unwrap();
        let mut ret = "".to_string();

        for node in &nodes {
            let idx = node.index() as u32;
            if let Some(path) = self.file_nodes.get_by_right(&idx) {
                ret.push_str(&format!("{path}\n"));
                for parent in self.file_dag.parents(*node).iter(&self.file_dag) {
                    let idx = parent.1.index() as u32;
                    if let Some(path) = self.file_nodes.get_by_right(&idx) {
                        ret.push_str(&format!(" |- {path}\n"));
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

pub fn dependent_files() -> HashMap<PathId, Vec<PathId>> {
    TYPE_DAG.with(|f| f.borrow().dependent_files())
}

pub fn dump() -> String {
    TYPE_DAG.with(|f| f.borrow().dump())
}

pub fn dump_file() -> String {
    TYPE_DAG.with(|f| f.borrow().dump_file())
}

pub fn clear() {
    TYPE_DAG.with(|f| f.borrow_mut().clear())
}
