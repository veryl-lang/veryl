use crate::HashMap;
use crate::ir::{Event, Statement, VarPath};
use std::fmt;
use veryl_analyzer::ir::VarId;
use veryl_parser::resource_table::StrId;

#[derive(Clone)]
pub struct Module {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub event_statements: HashMap<Event, Vec<Statement>>,
    pub comb_statements: Vec<Statement>,
}

#[derive(Clone)]
pub struct Clock {
    pub id: VarId,
    pub index: Option<usize>,
    pub select: Option<usize>,
}

impl fmt::Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("{}", self.id);
        if let Some(index) = self.index {
            ret.push_str(&format!("[{index}]"));
        }
        if let Some(select) = self.select {
            ret.push_str(&format!("[{select}]"));
        }
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct Reset {
    pub id: VarId,
    pub index: Option<usize>,
    pub select: Option<usize>,
}

impl fmt::Display for Reset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("{}", self.id);
        if let Some(index) = self.index {
            ret.push_str(&format!("[{index}]"));
        }
        if let Some(select) = self.select {
            ret.push_str(&format!("[{select}]"));
        }
        ret.fmt(f)
    }
}
