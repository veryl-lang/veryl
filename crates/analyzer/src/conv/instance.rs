use crate::HashMap;
use crate::conv::context::Config;
use crate::ir::{Component, Signature};
use std::collections::hash_map::Entry;
use std::sync::Arc;
use veryl_parser::token_range::TokenRange;

#[derive(Clone)]
struct Frame {
    /// `None` for a component whose diagnostics are replayed from the unit
    /// cache instead of converting it.
    sig: Option<Signature>,
    key: Option<String>,
    overridden: Option<String>,
    has_overrides: bool,
    /// `None` for a component evaluated other than by instantiation.
    site: Option<TokenRange>,
}

impl Frame {
    fn overridden_string(&self) -> String {
        match (&self.sig, &self.overridden) {
            (_, Some(x)) => x.clone(),
            (Some(x), None) => x.to_overridden_string(),
            (None, None) => String::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct InstanceHistory {
    hierarchy: Vec<Frame>,
    /// `Arc`-wrapped so repeated `get` hands out references instead of
    /// deep-cloning the component tree — matters on testbench-heavy designs.
    full: HashMap<Signature, Option<(Arc<Component>, bool)>>,
    /// Stable keys of the memoized signatures, kept only while the unit cache
    /// is in use. The flag is the `in_generic` of the memoized result.
    keys: Option<HashMap<String, bool>>,
    /// Reached only by replay, converted on demand. The flag tells whether
    /// the replay finished on a conversion that succeeded, which is when a
    /// conversion would have been memoized.
    ghosts: HashMap<String, bool>,
}

pub enum Reach {
    New,
    Cached,
}

impl InstanceHistory {
    pub fn track_keys(&mut self) {
        self.keys.get_or_insert_with(HashMap::default);
    }

    pub fn tracks_keys(&self) -> bool {
        self.keys.is_some()
    }

    pub fn get(&self, sig: &Signature) -> Option<(Arc<Component>, bool)> {
        self.full.get(sig).cloned().flatten()
    }

    pub fn set(&mut self, sig: &Signature, component: Arc<Component>, in_generic: bool) {
        if let Some(x) = self.full.get_mut(sig) {
            *x = Some((component, in_generic));
            if let Some(keys) = &mut self.keys {
                keys.insert(sig.stable_key(), in_generic);
            }
        }
    }

    pub fn remove(&mut self, sig: &Signature) {
        self.full.remove(sig);
    }

    /// Whether a signature with `key` was reached in a context where its
    /// memoized result can be reused, as `get_component_at` decides.
    pub fn is_reached(&self, key: &str, in_generic: bool) -> bool {
        self.ghosts.get(key).copied().unwrap_or(false)
            || self
                .keys
                .as_ref()
                .and_then(|x| x.get(key))
                .is_some_and(|cached_in_generic| !cached_in_generic || in_generic)
    }

    /// A ghost the replay memoized, to be converted without its diagnostics.
    pub fn is_memoized_ghost(&self, key: &str) -> bool {
        self.ghosts.get(key).copied().unwrap_or(false)
    }

    /// The stable keys of the hierarchy, outermost first.
    pub fn keys(&self) -> Vec<String> {
        self.hierarchy
            .iter()
            .filter_map(|x| x.key.clone())
            .collect()
    }

    pub fn get_current_site(&self) -> Option<TokenRange> {
        self.hierarchy.last().and_then(|x| x.site)
    }

    pub fn get_current_signature(&self) -> Option<&Signature> {
        self.hierarchy.last().and_then(|x| x.sig.as_ref())
    }

    /// Outermost first. Stops at an entry evaluated at its defaults: nothing
    /// above it can have decided the values below.
    pub fn overridden_frames(&self) -> Vec<(String, &TokenRange)> {
        let mut ret: Vec<_> = self
            .hierarchy
            .iter()
            .rev()
            .map_while(|x| {
                x.site
                    .as_ref()
                    .filter(|_| x.has_overrides)
                    .map(|site| (x.overridden_string(), site))
            })
            .collect();
        ret.reverse();
        ret
    }

    fn check_limits(
        &self,
        key: Option<&str>,
        sig: Option<&Signature>,
        config: &Config,
    ) -> Result<(), InstanceHistoryError> {
        if self.hierarchy.len() > config.instance_depth_limit {
            return Err(InstanceHistoryError::ExceedDepthLimit(self.hierarchy.len()));
        }
        let total = self.full.len() + self.ghosts.len();
        if total > config.instance_total_limit {
            return Err(InstanceHistoryError::ExceedTotalLimit(total));
        }
        let recursive = match (key, sig) {
            (Some(key), _) => self.hierarchy.iter().any(|x| x.key.as_deref() == Some(key)),
            (None, Some(sig)) => self.hierarchy.iter().any(|x| x.sig.as_ref() == Some(sig)),
            (None, None) => false,
        };
        if recursive {
            return Err(InstanceHistoryError::InfiniteRecursion);
        }
        Ok(())
    }

    pub fn push(
        &mut self,
        mut sig: Signature,
        site: Option<TokenRange>,
        config: &Config,
    ) -> Result<Reach, InstanceHistoryError> {
        sig.normalize();
        let key = self.keys.is_some().then(|| sig.stable_key());
        self.check_limits(key.as_deref(), Some(&sig), config)?;
        // A ghost whose replay was not memoized is reached again, as an
        // unmemoized conversion would be; its count moves to `full`.
        if let Some(key) = &key {
            self.ghosts.remove(key);
        }
        Ok(self.push_unchecked(sig, key, site))
    }

    /// Pushes a ghost being materialized: it was counted and checked when
    /// first reached by replay.
    pub fn push_materialized(&mut self, mut sig: Signature, site: Option<TokenRange>) {
        sig.normalize();
        let key = sig.stable_key();
        self.ghosts.remove(&key);
        self.push_unchecked(sig, Some(key), site);
    }

    fn push_unchecked(
        &mut self,
        sig: Signature,
        key: Option<String>,
        site: Option<TokenRange>,
    ) -> Reach {
        let has_overrides = sig.has_overrides();
        // Pushed even when cached: the caller pops after every success.
        self.hierarchy.push(Frame {
            sig: Some(sig.clone()),
            key,
            overridden: None,
            has_overrides,
            site,
        });
        match self.full.entry(sig) {
            Entry::Occupied(_) => Reach::Cached,
            Entry::Vacant(x) => {
                x.insert(None);
                Reach::New
            }
        }
    }

    pub fn push_ghost(
        &mut self,
        key: &str,
        overridden: &str,
        has_overrides: bool,
        site: Option<TokenRange>,
        config: &Config,
    ) -> Result<(), InstanceHistoryError> {
        self.check_limits(Some(key), None, config)?;
        self.hierarchy.push(Frame {
            sig: None,
            key: Some(key.to_string()),
            overridden: Some(overridden.to_string()),
            has_overrides,
            site,
        });
        self.ghosts.entry(key.to_string()).or_insert(false);
        Ok(())
    }

    pub fn pop_ghost(&mut self, memoized: bool) {
        if let Some(Frame { key: Some(key), .. }) = self.hierarchy.pop()
            && memoized
        {
            self.ghosts.insert(key, true);
        }
    }

    pub fn pop(&mut self) {
        self.hierarchy.pop();
    }

    pub fn clear(&mut self) {
        self.hierarchy.clear();
        self.full.clear();
        if let Some(x) = &mut self.keys {
            x.clear();
        }
        self.ghosts.clear();
    }
}

#[derive(Debug)]
pub enum InstanceHistoryError {
    ExceedDepthLimit(usize),
    ExceedTotalLimit(usize),
    InfiniteRecursion,
}
