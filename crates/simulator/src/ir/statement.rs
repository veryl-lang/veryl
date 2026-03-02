use crate::cranelift::FuncPtr;
use crate::ir::{CombValue, Expression, FfValue, Value};
use indent::indent_all_by;
use std::fmt;
use veryl_analyzer::ir::VarId;
use veryl_analyzer::value::MaskCache;

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    If(IfStatement),
    Binary((FuncPtr, *const FfValue, *const CombValue)),
}

unsafe impl Send for Statement {}
unsafe impl Sync for Statement {}

impl Statement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        match self {
            Statement::Assign(x) => x.eval_step(mask_cache),
            Statement::If(x) => x.eval_step(mask_cache),
            Statement::Binary((func, ff_values, comb_values)) => unsafe {
                func(*ff_values, *comb_values);
            },
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<VarId>, outputs: &mut Vec<VarId>) {
        match self {
            Statement::Assign(x) => x.gather_variable(inputs, outputs),
            Statement::If(x) => x.gather_variable(inputs, outputs),
            Statement::Binary(_) => (),
        }
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Assign(x) => x.fmt(f),
            Statement::If(x) => x.fmt(f),
            Statement::Binary(_) => "".fmt(f),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssignDestination {
    pub id: VarId,
    pub index: Option<usize>,
    pub select: Option<(usize, usize)>,
}

impl fmt::Display for AssignDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("{}", self.id);
        if let Some(index) = self.index {
            ret.push_str(&format!("[{index}]"));
        }
        if let Some((beg, end)) = self.select {
            ret.push_str(&format!("[{beg}:{end}]"));
        }
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct AssignStatement {
    pub dst: *mut Value,
    pub dst_id: VarId,
    pub expr: Expression,
    pub expr_signed: bool,
}

impl AssignStatement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        let dst_width = unsafe { (*self.dst).width() };

        let value = self
            .expr
            .eval(Some(dst_width), self.expr_signed, mask_cache);
        unsafe {
            (*self.dst).set_value(value);
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<VarId>, outputs: &mut Vec<VarId>) {
        self.expr.gather_variable(inputs, outputs);
        outputs.push(self.dst_id);
    }
}

impl fmt::Display for AssignStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        //let mut ret = String::new();

        //if self.dst.len() == 1 {
        //    ret.push_str(&format!("{} = {};", self.dst[0], self.expr));
        //} else {
        //    ret.push_str(&format!("{{{}", self.dst[0]));
        //    for d in &self.dst[1..] {
        //        ret.push_str(&format!(", {}", d));
        //    }
        //    ret.push_str(&format!("}} = {};", self.expr));
        //}
        //ret.fmt(f)
        "".fmt(f)
    }
}

#[derive(Clone)]
pub struct IfStatement {
    pub cond: Option<Expression>,
    pub true_side: Vec<Statement>,
    pub false_side: Vec<Statement>,
}

impl IfStatement {
    pub fn eval_step(&self, mask_cache: &mut MaskCache) {
        let cond = if let Some(x) = &self.cond {
            let cond = x.eval(Some(1), false, mask_cache);
            cond.to_usize().unwrap_or(0) != 0
        } else {
            false
        };

        if cond {
            for x in &self.true_side {
                x.eval_step(mask_cache);
            }
        } else {
            for x in &self.false_side {
                x.eval_step(mask_cache);
            }
        }
    }

    pub fn gather_variable(&self, inputs: &mut Vec<VarId>, outputs: &mut Vec<VarId>) {
        if let Some(x) = &self.cond {
            x.gather_variable(inputs, outputs);
        }

        for x in &self.true_side {
            x.gather_variable(inputs, outputs);
        }
        for x in &self.false_side {
            x.gather_variable(inputs, outputs);
        }
    }
}

impl fmt::Display for IfStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = if let Some(x) = &self.cond {
            format!("if {} {{\n", x)
        } else {
            "if_reset {\n".to_string()
        };

        for x in &self.true_side {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }
        ret.push('}');
        if !self.false_side.is_empty() {
            ret.push_str(" else {\n");
            for x in &self.false_side {
                let text = format!("{}\n", x);
                ret.push_str(&indent_all_by(2, text));
            }
            ret.push('}');
        }

        ret.fmt(f)
    }
}
