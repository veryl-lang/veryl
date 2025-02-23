use crate::evaluator::EvaluatedValue;
use crate::symbol::SymbolId;
use std::cell::RefCell;
use std::collections::HashSet;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct InstanceSignature {
    symbol: SymbolId,
    params: Vec<(StrId, EvaluatedValue)>,
}

impl InstanceSignature {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            params: Vec::new(),
        }
    }

    pub fn add_param(&mut self, id: StrId, value: EvaluatedValue) {
        self.params.push((id, value));
    }

    fn normalize(&mut self) {
        self.params.sort();
    }
}

#[derive(Default)]
pub struct InstanceHistory {
    pub depth_limit: usize,
    pub total_limit: usize,
    pub hierarchy: Vec<InstanceSignature>,
    full: HashSet<InstanceSignature>,
}

impl InstanceHistory {
    fn set_depth_limit(&mut self, value: usize) {
        self.depth_limit = value;
    }

    fn set_total_limit(&mut self, value: usize) {
        self.total_limit = value;
    }

    fn push(&mut self, mut sig: InstanceSignature) -> Result<bool, InstanceHistoryError> {
        sig.normalize();
        if self.hierarchy.len() > self.depth_limit {
            return Err(InstanceHistoryError::ExceedDepthLimit);
        }
        if self.full.len() > self.total_limit {
            return Err(InstanceHistoryError::ExceedTotalLimit);
        }
        if self.hierarchy.iter().any(|x| *x == sig)
            && sig.params.iter().all(|x| x.1.get_value().is_some())
        {
            return Err(InstanceHistoryError::InfiniteRecursion);
        }
        if self.full.contains(&sig) {
            Ok(false)
        } else {
            self.hierarchy.push(sig.clone());
            self.full.insert(sig);
            Ok(true)
        }
    }

    fn pop(&mut self) {
        self.hierarchy.pop();
    }

    fn clear(&mut self) {
        self.hierarchy.clear();
        self.full.clear();
    }
}

#[derive(Debug)]
pub enum InstanceHistoryError {
    ExceedDepthLimit,
    ExceedTotalLimit,
    InfiniteRecursion,
}

impl InstanceHistoryError {}

thread_local!(static INSTANCE_HISTORY: RefCell<InstanceHistory> = RefCell::new(InstanceHistory::default()));

pub fn set_depth_limit(value: usize) {
    INSTANCE_HISTORY.with(|f| f.borrow_mut().set_depth_limit(value))
}

pub fn set_total_limit(value: usize) {
    INSTANCE_HISTORY.with(|f| f.borrow_mut().set_total_limit(value))
}

pub fn push(sig: InstanceSignature) -> Result<bool, InstanceHistoryError> {
    INSTANCE_HISTORY.with(|f| f.borrow_mut().push(sig))
}

pub fn pop() {
    INSTANCE_HISTORY.with(|f| f.borrow_mut().pop())
}

pub fn clear() {
    INSTANCE_HISTORY.with(|f| f.borrow_mut().clear())
}
