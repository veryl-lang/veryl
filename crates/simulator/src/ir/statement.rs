use crate::cranelift::Context;
use crate::ir::{Expression, ExpressionProto, Value};
use cranelift::prelude::types::I64;
use cranelift::prelude::{FunctionBuilder, InstBuilder, MemFlags};
use indent::indent_all_by;
use std::fmt;
use veryl_analyzer::ir::VarId;

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    If(IfStatement),
}

impl Statement {
    pub fn eval_step(&self, reset: bool) {
        match self {
            Statement::Assign(x) => x.eval_step(),
            Statement::If(x) => x.eval_step(reset),
        }
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Assign(x) => x.fmt(f),
            Statement::If(x) => x.fmt(f),
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
    pub expr: Expression,
}

impl AssignStatement {
    pub fn eval_step(&self) {
        let value = self.expr.eval();
        unsafe {
            *self.dst = value;
        }
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
    pub fn eval_step(&self, reset: bool) {
        let cond = if let Some(x) = &self.cond {
            let cond = x.eval();
            //cond.to_usize() != 0
            cond != 0
        } else {
            reset
        };

        if cond {
            for x in &self.true_side {
                x.eval_step(false);
            }
        } else {
            for x in &self.false_side {
                x.eval_step(false);
            }
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StatementProto {
    Assign(AssignStatementProto),
    If(IfStatementProto),
}

impl StatementProto {
    pub fn build_binary(
        &self,
        context: &mut Context,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) {
        match self {
            StatementProto::Assign(x) => x.build_binary(context, builder),
            StatementProto::If(x) => x.build_binary(context, builder, is_last),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssignStatementProto {
    pub dst: usize,
    pub expr: ExpressionProto,
}

impl AssignStatementProto {
    pub fn build_binary(&self, context: &mut Context, builder: &mut FunctionBuilder) {
        let val = self.expr.build_binary(context, builder);

        let load_mem_flag = MemFlags::trusted().with_readonly();
        let store_mem_flag = MemFlags::trusted();

        if let Some(ptr) = context.value_map.get(&self.dst) {
            builder.ins().store(store_mem_flag, val, *ptr, 0);
        } else {
            let offset = (self.dst * size_of::<usize>()) as i32;
            let ptr = builder.ins().load(I64, load_mem_flag, context.args, offset);
            context.value_map.insert(self.dst, ptr);
            builder.ins().store(store_mem_flag, val, ptr, 0);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IfStatementProto {
    pub cond: Option<ExpressionProto>,
    pub true_side: Vec<StatementProto>,
    pub false_side: Vec<StatementProto>,
}

impl IfStatementProto {
    pub fn build_binary(
        &self,
        context: &mut Context,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) {
        let true_block = builder.create_block();
        let false_block = builder.create_block();
        let final_block = builder.create_block();

        if let Some(x) = &self.cond {
            let cond = x.build_binary(context, builder);
            builder.ins().brif(cond, true_block, &[], false_block, &[]);
        } else {
            builder
                .ins()
                .brif(context.reset, true_block, &[], false_block, &[]);
        }

        builder.switch_to_block(true_block);
        context.value_map.clear();
        let len = self.true_side.len();
        for (i, x) in self.true_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last);
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        builder.switch_to_block(false_block);
        context.value_map.clear();
        let len = self.true_side.len();
        for (i, x) in self.false_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last);
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        builder.switch_to_block(final_block);
        context.value_map.clear();
    }
}
