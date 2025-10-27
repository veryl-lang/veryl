use crate::HashMap;
use crate::conv::context::Config;
use crate::ir::{Component, Signature};

#[derive(Clone, Default)]
pub struct InstanceHistory {
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

    pub fn push(
        &mut self,
        mut sig: Signature,
        config: &Config,
    ) -> Result<bool, InstanceHistoryError> {
        sig.normalize();
        if self.hierarchy.len() > config.instance_depth_limit {
            return Err(InstanceHistoryError::ExceedDepthLimit(self.hierarchy.len()));
        }
        if self.full.len() > config.instance_total_limit {
            return Err(InstanceHistoryError::ExceedTotalLimit(self.full.len()));
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
    ExceedDepthLimit(usize),
    ExceedTotalLimit(usize),
    InfiniteRecursion,
}
