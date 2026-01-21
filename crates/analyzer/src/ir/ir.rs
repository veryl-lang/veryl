use crate::conv::Context;
use crate::ir::{AssignDestination, Interface, Module};
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

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

#[derive(Clone, Debug)]
pub struct IrError {
    pub token: TokenRange,
    pub code: String,
}

#[macro_export]
macro_rules! ir_error {
    ($x:expr) => {
        Box::new($crate::ir::IrError {
            token: $x,
            code: format!("{}:{}:{}", file!(), line!(), column!()),
        })
    };
}

pub type IrResult<T> = Result<T, Box<IrError>>;

#[derive(Clone)]
pub enum Component {
    Module(Module),
    Interface(Interface),
    SystemVerilog(SystemVerilog),
}

impl Component {
    pub fn eval_assign(&self, context: &mut Context) {
        match self {
            Component::Module(x) => x.eval_assign(context),
            Component::Interface(_) => (),
            Component::SystemVerilog(_) => (),
        }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Component::Module(x) => x.fmt(f),
            Component::Interface(x) => x.fmt(f),
            Component::SystemVerilog(_) => "".fmt(f),
        }
    }
}

#[derive(Clone)]
pub struct SystemVerilog {
    pub name: StrId,
    pub connects: Vec<AssignDestination>,
}
