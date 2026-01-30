mod declaration;
mod event;
mod expression;
mod proto;
mod statement;
mod variable;

pub use crate::conv::build_ir;
pub use declaration::{Clock, Reset};
pub use event::Event;
pub use expression::Expression;
pub use proto::{
    ConvAssignStatement, ConvDeclaration, ConvExpression, ConvIfStatement, ConvStatement,
    ProtoAssignStatement, ProtoExpression, ProtoIfStatement, ProtoStatement,
};
pub use statement::{AssignDestination, AssignStatement, IfStatement, Statement};
pub use variable::{CombValue, FfValue, Variable};
pub use veryl_analyzer::ir::{Op, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::HashMap;
use indent::indent_all_by;
use memmap2::Mmap;
//use rayon::prelude::*;
use std::fmt;
use veryl_analyzer::ir::Function;
use veryl_analyzer::value::MaskCache;
use veryl_parser::resource_table::StrId;

#[derive(Default)]
pub struct Ir {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_values: Box<[FfValue]>,
    pub comb_values: Box<[CombValue]>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub binary: Vec<Mmap>,
    pub event_statements: HashMap<Event, Vec<Statement>>,
    pub comb_statements: Vec<Statement>,
}

impl Ir {
    pub fn eval_step(&self, event: &Event, mask_cache: &mut MaskCache) {
        for x in &self.comb_statements {
            x.eval_step(mask_cache);
        }

        if let Some(statements) = self.event_statements.get(event) {
            //let _: Vec<_> = statements
            //    .par_iter()
            //    .map(|x: &Statement| x.eval_step())
            //    .collect();

            for x in statements {
                x.eval_step(mask_cache);
            }
        }
    }

    pub fn dump_variables(&self) -> String {
        let mut ret = String::new();

        let mut variables: Vec<_> = self.variables.iter().collect();
        variables.sort_by(|a, b| a.0.cmp(b.0));

        for (_, x) in variables {
            ret.push_str(&format!("{}\n", x));
        }

        ret
    }
}

impl fmt::Display for Ir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("module {} {{\n", self.name);

        let mut variables: Vec<_> = self.variables.iter().collect();
        variables.sort_by(|a, b| a.0.cmp(b.0));

        for (_, x) in variables {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('\n');

        //for x in &self.declarations {
        //    let text = format!("{}\n", x);
        //    ret.push_str(&indent_all_by(2, text));
        //}

        ret.push('}');
        ret.fmt(f)
    }
}
