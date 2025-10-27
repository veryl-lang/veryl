use crate::HashMap;
use crate::ir::{Component, Signature};

#[derive(Clone, Default)]
pub struct InstanceHistory {
    pub depth_limit: usize,
    pub total_limit: usize,
    pub hierarchy: Vec<Signature>,
    full: HashMap<Signature, Option<Component>>,
}

impl InstanceHistory {
    pub fn get(&self, sig: &Signature) -> Option<Component> {
        self.full.get(sig).cloned().flatten()
    }

    pub fn set(&mut self, sig: &Signature, component: Component) {
        if let Some(x) = self.full.get_mut(sig) {
            *x = Some(component);
        }
    }

    pub fn get_current_signature(&self) -> Option<&Signature> {
        self.hierarchy.last()
    }

    pub fn push(&mut self, mut sig: Signature) -> Result<bool, InstanceHistoryError> {
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
