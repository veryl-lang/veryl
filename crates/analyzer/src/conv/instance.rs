use crate::HashMap;
use crate::conv::context::Config;
use crate::ir::{Component, Signature};
use std::collections::hash_map::Entry;
use std::sync::Arc;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Default)]
pub struct InstanceHistory {
    /// The site is `None` for a component evaluated other than by
    /// instantiation.
    hierarchy: Vec<(Signature, Option<TokenRange>)>,
    /// `Arc`-wrapped so repeated `get` hands out references instead of
    /// deep-cloning the component tree — matters on testbench-heavy designs.
    full: HashMap<Signature, Option<(Arc<Component>, bool)>>,
}

impl InstanceHistory {
    pub fn get(&self, sig: &Signature) -> Option<(Arc<Component>, bool)> {
        self.full.get(sig).cloned().flatten()
    }

    pub fn set(&mut self, sig: &Signature, component: Arc<Component>, in_generic: bool) {
        if let Some(x) = self.full.get_mut(sig) {
            *x = Some((component, in_generic));
        }
    }

    pub fn remove(&mut self, sig: &Signature) {
        self.full.remove(sig);
    }

    pub fn get_current_signature(&self) -> Option<&Signature> {
        self.hierarchy.last().map(|(sig, _)| sig)
    }

    /// Outermost first. Stops at an entry evaluated at its defaults: nothing
    /// above it can have decided the values below.
    pub fn overridden_frames(&self) -> Vec<(&Signature, &TokenRange)> {
        let mut ret: Vec<_> = self
            .hierarchy
            .iter()
            .rev()
            .map_while(|(sig, site)| {
                site.as_ref()
                    .filter(|_| sig.has_overrides())
                    .map(|x| (sig, x))
            })
            .collect();
        ret.reverse();
        ret
    }

    pub fn push(
        &mut self,
        mut sig: Signature,
        site: Option<TokenRange>,
        config: &Config,
    ) -> Result<bool, InstanceHistoryError> {
        sig.normalize();
        if self.hierarchy.len() > config.instance_depth_limit {
            return Err(InstanceHistoryError::ExceedDepthLimit(self.hierarchy.len()));
        }
        if self.full.len() > config.instance_total_limit {
            return Err(InstanceHistoryError::ExceedTotalLimit(self.full.len()));
        }
        if self.hierarchy.iter().any(|(x, _)| *x == sig) {
            return Err(InstanceHistoryError::InfiniteRecursion);
        }
        // Pushed even when cached: the caller pops after every success.
        self.hierarchy.push((sig.clone(), site));
        match self.full.entry(sig) {
            Entry::Occupied(_) => Ok(false),
            Entry::Vacant(x) => {
                x.insert(None);
                Ok(true)
            }
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
