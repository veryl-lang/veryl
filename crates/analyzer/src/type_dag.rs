use crate::{symbol::{Symbol, SymbolId}, AnalyzerError};
use std::{cell::RefCell, collections::HashMap};

use daggy::Dag;

#[derive(Debug, Default)]
pub struct TypeDag {
    dag: Dag<(), (), u32>,
    /// Map between SymbolId and DAG NodeIdx
    nodes: HashMap<SymbolId, u32>,
}

#[derive(Clone, Debug)]
pub enum DagError {
    Cyclic(Symbol, Symbol),
}

impl TypeDag {
    #[allow(dead_code)]
    fn new() -> Self {
        Self {
            dag: Dag::<(), (), u32>::new(),
            nodes: HashMap::<SymbolId, u32>::new(),
        }
    }
    fn insert_node(&mut self, sym: &Symbol) -> u32 {
        match self.nodes.get(&sym.id) {
            Some(node_idx) => *node_idx,
            None => {
                let node_idx = self.dag.add_node(()).index() as u32;
                self.nodes.insert(sym.id, node_idx);
                node_idx
            }
        }
    }

    fn insert_edge(&mut self, start: &Symbol, end: &Symbol) -> Result<(), DagError> {
        let s = self.insert_node(start);
            let e = self.insert_node(end);
            match self.dag.add_edge(s.into(), e.into(), ()) {
            Ok(_) => Ok(()),
            Err(_) => Err(DagError::Cyclic(start.clone(), end.clone())),
        }
    }
}

thread_local!(static TYPE_DAG: RefCell<TypeDag> = RefCell::new(TypeDag::new()));

pub fn insert_edge(start: &Symbol, end: &Symbol) -> Result<(), DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_edge(start, end))
}

pub fn insert_node(start: &Symbol) -> u32 {
    TYPE_DAG.with(|f| f.borrow_mut().insert_node(start))
}
