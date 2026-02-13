use crate::conv::Context;
use crate::ir::{AssignDestination, Interface, Module};
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

    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = String::new();
        for x in &self.components {
            ret.push_str(&format!("{}\n", x.to_string(context)));
        }
        ret
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

    pub fn to_string(&self, context: &Context) -> String {
        match self {
            Component::Module(x) => x.to_string(context),
            Component::Interface(x) => x.to_string(context),
            Component::SystemVerilog(_) => "".to_string(),
        }
    }
}

#[derive(Clone)]
pub struct SystemVerilog {
    pub name: StrId,
    pub connects: Vec<AssignDestination>,
}
