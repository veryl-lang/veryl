use crate::cranelift::Context;
use crate::ir::{Op, Value};
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::I64;
use cranelift::prelude::{FunctionBuilder, InstBuilder, MemFlags};
use std::fmt;

#[derive(Clone, Debug)]
pub enum Expression {
    Value(*const Value),
    //Unary(Op, Box<Expression>),
    Binary(Box<Expression>, Op, Box<Expression>),
}

impl Expression {
    pub fn eval(&self) -> Value {
        match self {
            Expression::Value(x) => unsafe { **x },
            //Expression::Unary(_op, x) => {
            //    let x = x.eval();
            //    x
            //}
            Expression::Binary(x, _op, y) => {
                let x = x.eval();
                let y = y.eval();
                //Value::new(x.payload + y.payload, x.width, false)
                x + y
            }
        }
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            Expression::Value(_) => "var".to_string(),
            //Expression::Unary(x, y) => {
            //    format!("({x} {y})")
            //}
            Expression::Binary(x, y, z) => {
                format!("({x} {y} {z})")
            }
        };

        ret.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ExpressionProto {
    Immediate(u64),
    Value(usize),
    //Unary(Op, Box<Expression>),
    Binary(Box<ExpressionProto>, Op, Box<ExpressionProto>),
}

impl ExpressionProto {
    pub fn build_binary(
        &self,
        context: &mut Context,
        builder: &mut FunctionBuilder,
    ) -> CraneliftValue {
        match self {
            ExpressionProto::Immediate(x) => {
                let x = *x as i64;
                builder.ins().iconst(I64, x)
            }
            ExpressionProto::Value(x) => {
                let load_mem_flag = MemFlags::trusted().with_readonly();

                if let Some(ptr) = context.value_map.get(x) {
                    builder.ins().load(I64, load_mem_flag, *ptr, 0)
                } else {
                    let offset = (x * size_of::<usize>()) as i32;
                    let ptr = builder.ins().load(I64, load_mem_flag, context.args, offset);
                    context.value_map.insert(*x, ptr);
                    builder.ins().load(I64, load_mem_flag, ptr, 0)
                }
            }
            ExpressionProto::Binary(x, _op, y) => {
                let x = x.build_binary(context, builder);
                let y = y.build_binary(context, builder);
                builder.ins().iadd(x, y)
            }
        }
    }
}
