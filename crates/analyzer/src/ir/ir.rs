use crate::ir::Module;
use std::fmt;

#[derive(Clone, Default)]
pub struct Ir {
    pub components: Vec<Component>,
}

impl Ir {
    pub fn append(&mut self, x: &mut Ir) {
        self.components.append(&mut x.components);
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
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Component::Module(x) => x.fmt(f),
        }
    }
}
