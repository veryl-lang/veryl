use crate::conv::Context;
use crate::ir::{Interface, Module};
use std::fmt;

#[derive(Clone, Default)]
pub struct Ir {
    pub components: Vec<Component>,
}

impl Ir {
    pub fn append(&mut self, x: &mut Ir) {
        self.components.append(&mut x.components);
    }

    pub fn eval_assign(&self, context: &mut Context) {
        for x in &self.components {
            x.eval_assign(context);
        }
    }
}

impl fmt::Display for Ir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();
        for x in &self.components {
            ret.push_str(&format!("{}\n", x));
        }
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub enum Component {
    Module(Module),
    Interface(Interface),
}

impl Component {
    pub fn eval_assign(&self, context: &mut Context) {
        match self {
            Component::Module(x) => x.eval_assign(context),
            Component::Interface(_) => (),
        }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Component::Module(x) => x.fmt(f),
            Component::Interface(x) => x.fmt(f),
        }
    }
}
