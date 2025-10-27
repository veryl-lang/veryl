use crate::HashMap;
use crate::ir::{Component, ValueVariant};
use crate::symbol::SymbolId;
use crate::symbol_path::GenericSymbolPath;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct InstanceSignature {
    symbol: SymbolId,
    parameters: Vec<(StrId, ValueVariant)>,
    generic_parameters: Vec<(StrId, GenericSymbolPath)>,
}

impl InstanceSignature {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            parameters: vec![],
            generic_parameters: vec![],
        }
    }

    pub fn add_parameter(&mut self, id: StrId, value: ValueVariant) {
        self.parameters.push((id, value));
    }

    pub fn add_generic_parameter(&mut self, id: StrId, value: GenericSymbolPath) {
        self.generic_parameters.push((id, value));
    }

    pub fn normalize(&mut self) {
        self.parameters.sort();
        self.generic_parameters.sort();
    }
}

#[derive(Clone, Default)]
pub struct InstanceHistory {
    pub depth_limit: usize,
    pub total_limit: usize,
    pub hierarchy: Vec<InstanceSignature>,
    full: HashMap<InstanceSignature, Option<Component>>,
}

impl InstanceHistory {
    pub fn get(&self, sig: &InstanceSignature) -> Option<Component> {
        self.full.get(sig).cloned().flatten()
    }

    pub fn set(&mut self, sig: &InstanceSignature, component: Component) {
        if let Some(x) = self.full.get_mut(sig) {
            *x = Some(component);
        }
    }

    pub fn push(&mut self, mut sig: InstanceSignature) -> Result<bool, InstanceHistoryError> {
        sig.normalize();
        if self.hierarchy.len() > self.depth_limit {
            return Err(InstanceHistoryError::ExceedDepthLimit);
        }
        if self.full.len() > self.total_limit {
            return Err(InstanceHistoryError::ExceedTotalLimit);
        }
        if self.hierarchy.contains(&sig) {
            return Err(InstanceHistoryError::InfiniteRecursion);
        }
        if self.full.contains_key(&sig) {
            Ok(false)
        } else {
            self.hierarchy.push(sig.clone());
            self.full.insert(sig, None);
            Ok(true)
        }
    }

    pub fn pop(&mut self) {
        self.hierarchy.pop();
    }

    pub fn clear(&mut self) {
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
