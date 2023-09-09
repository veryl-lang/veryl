use crate::{symbol::{Symbol, SymbolId}, symbol_table::{SymbolPathNamespace, self}};
use std::{cell::RefCell, collections::HashMap};
use bimap::BiMap;

use daggy::Dag;
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

#[derive(Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq)]
pub enum Context {
    Irrelevant,
    Struct,
    Union,
    Enum,
    TypeDef,
}

impl Default for Context {
    fn default() -> Self {
        Context::Irrelevant
    }
}

#[derive(Debug, Clone)]
pub enum DagError {
    Cyclic(Symbol, Symbol),
    UnableToResolve(TypeResolveInfo),
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

    fn insert_node(&mut self, path: &SymbolPathNamespace, id: &str, token: &VerylToken) -> Result<u32, DagError> {
        let trinfo = TypeResolveInfo {
            path: path.clone(),
            name: id.into(),
            token: token.clone(),
        };
        let sym = match symbol_table::get(&trinfo.path.0, &trinfo.path.1) {
            Ok(rr) => {
                if let Some(sym) = rr.found {
                    sym
                } else {
                    return Err(DagError::UnableToResolve(trinfo));
                }
            }
            Err(_) => {
                let e = DagError::UnableToResolve(trinfo);
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
            Some(TypeResolveInfo { path, .. }) => {
                match symbol_table::get(&path.0, &path.1) {
                    Ok(rr) => {
                        match rr.found {
                            Some(sym) => sym,
                            None => { unreachable!(); }
                        }
                    }
                    Err(_) => { unreachable!(); }
                }
            },
            None => { panic!("Must insert node before accessing"); }
        }
    }

    fn insert_edge(&mut self, start: u32, end: u32, edge: Context) -> Result<(), DagError> {
        match self.dag.add_edge(start.into(), end.into(), edge) {
            Ok(_) => Ok(()),
            Err(_) => {
                let ssym = self.get_symbol(start);
                let esym = self.get_symbol(end);
                Err(DagError::Cyclic(ssym, esym))
            }
        }
    }

    // fn evaluate(&self) {
    //     fn rec_eval(td: &TypeDag, node: u32) -> Evaluated {
    //         let nbors: Vec<(Context, u32)> = td.dag.children(
    //             node.into()).iter(&td.dag)
    //             .map(|(e, n)| (*td.dag.edge_weight(e).unwrap(), n.index() as u32))
    //             .collect();
    //         // Evaluate children
    //         for (_, nbor) in nbors.iter() {
    //             rec_eval(td, *nbor);
    //         }
    //         // Evaluate node
    //         let node_sym = td.paths.get(&node).unwrap();
    //         Evaluated::Unknown
    //     }
    // }
}

thread_local!(static TYPE_DAG: RefCell<TypeDag> = RefCell::new(TypeDag::new()));

pub fn insert_edge(start: u32, end: u32, context: Context) -> Result<(), DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_edge(start, end, context))
}

pub fn insert_node(start: &SymbolPathNamespace, id: &str, token:  &VerylToken) -> Result<u32, DagError> {
    TYPE_DAG.with(|f| f.borrow_mut().insert_node(start, id, token))
}

// pub fn evaluate() {
//     let dag = TYPE_DAG.with(|f| f.borrow_mut());
// }

pub fn get_symbol(node: u32) -> Symbol {
    TYPE_DAG.with(|f| f.borrow().get_symbol(node))
}
