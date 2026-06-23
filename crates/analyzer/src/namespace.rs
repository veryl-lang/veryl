use crate::attribute::Attribute;
use crate::attribute_table;
use crate::{HashSet, SVec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_token::{Token, VerylToken};

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DefineContext {
    pos: BTreeSet<StrId>,
    neg: BTreeSet<StrId>,
}

impl DefineContext {
    pub fn exclusive(&self, value: &DefineContext) -> bool {
        !self.pos.is_disjoint(&value.neg) || !self.neg.is_disjoint(&value.pos)
    }

    pub fn is_default(&self) -> bool {
        self.pos.is_empty()
    }

    pub fn is_active(&self, defines: &HashSet<StrId>) -> bool {
        self.pos.iter().all(|x| defines.contains(x))
            && self.neg.iter().all(|x| !defines.contains(x))
    }
}

impl From<Token> for DefineContext {
    fn from(token: Token) -> Self {
        let attrs = attribute_table::get(&token);
        attrs.as_slice().into()
    }
}

impl From<&VerylToken> for DefineContext {
    fn from(token: &VerylToken) -> Self {
        let attrs = attribute_table::get(&token.token);
        attrs.as_slice().into()
    }
}

impl From<&[Attribute]> for DefineContext {
    fn from(value: &[Attribute]) -> Self {
        let mut ret = DefineContext::default();
        for x in value {
            match x {
                Attribute::Ifdef(x) => {
                    ret.pos.insert(*x);
                }
                Attribute::Ifndef(x) => {
                    ret.neg.insert(*x);
                }
                Attribute::Elsif(x, y, z) => {
                    ret.pos.insert(*x);
                    for y in y {
                        ret.pos.insert(*y);
                    }
                    for z in z {
                        ret.neg.insert(*z);
                    }
                }
                Attribute::Else(x, y) => {
                    for x in x {
                        ret.pos.insert(*x);
                    }
                    for y in y {
                        ret.neg.insert(*y);
                    }
                }
                _ => (),
            }
        }
        ret
    }
}

impl fmt::Display for DefineContext {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        for x in &self.pos {
            text.push_str(&format!("+{x}"));
        }
        for x in &self.neg {
            text.push_str(&format!("-{x}"));
        }
        text.fmt(f)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Namespace {
    pub paths: SVec<StrId>,
    pub define_context: DefineContext,
}

impl Namespace {
    pub fn new() -> Self {
        Self {
            paths: SVec::new(),
            define_context: DefineContext::default(),
        }
    }

    pub fn push(&mut self, path: StrId) {
        self.paths.push(resource_table::canonical_str_id(path));
    }

    pub fn pop(&mut self) -> Option<StrId> {
        self.paths.pop()
    }

    pub fn depth(&self) -> usize {
        self.paths.len()
    }

    pub fn included(&self, x: &Namespace) -> bool {
        let exclusive = self.define_context.exclusive(&x.define_context);
        for (i, x) in x.paths.iter().enumerate() {
            if let Some(path) = self.paths.get(i) {
                if path != x {
                    return false;
                }
            } else {
                return false;
            }
        }
        !exclusive
    }

    pub fn matched(&self, x: &Namespace) -> bool {
        if self.paths.len() != x.paths.len() {
            false
        } else {
            self.included(x)
        }
    }

    pub fn strip_anonymous_path(&mut self) {
        self.paths.retain(|x| x.to_string().find('@').is_none());
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        if let Some(first) = self.paths.first() {
            text.push_str(&format!("{first}"));
            for path in &self.paths[1..] {
                text.push_str(&format!("::{path}"));
            }
        }
        text.push_str(&self.define_context.to_string());
        text.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_context() {
        let mut a = DefineContext::default();
        a.pos.insert(StrId(0));
        a.pos.insert(StrId(1));
        a.neg.insert(StrId(2));
        let mut b = DefineContext::default();
        b.pos.insert(StrId(2));

        assert!(a.exclusive(&b));

        let mut a = DefineContext::default();
        a.pos.insert(StrId(0));
        a.pos.insert(StrId(1));
        a.neg.insert(StrId(2));
        let mut b = DefineContext::default();
        b.pos.insert(StrId(1));

        assert!(!a.exclusive(&b));

        let mut a = DefineContext::default();
        a.pos.insert(StrId(0));
        a.pos.insert(StrId(1));
        a.neg.insert(StrId(2));
        let mut b = DefineContext::default();
        b.neg.insert(StrId(0));

        assert!(a.exclusive(&b));

        let a = DefineContext::default();
        let b = DefineContext::default();

        assert!(!a.exclusive(&b));
    }
}
