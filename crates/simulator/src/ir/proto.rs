use crate::HashMap;
use crate::cranelift::Context;
use crate::ir::{AssignStatement, Event, Expression, IfStatement, Op, Statement};
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::Value as CraneliftValue;
use cranelift::prelude::types::I64;
use cranelift::prelude::{FunctionBuilder, InstBuilder, IntCC, MemFlags};
use veryl_analyzer::value::ValueU64;

pub struct ConvStatement {
    pub value: Statement,
    pub proto: Option<ProtoStatement>,
}

impl ConvStatement {
    pub fn split_if_reset(self) -> Option<(Vec<ConvStatement>, Vec<ConvStatement>)> {
        if let Statement::If(x) = self.value {
            if x.cond.is_some() {
                return None;
            }

            if let Some(ProtoStatement::If(proto)) = self.proto {
                let true_side = x.true_side;
                let false_side = x.false_side;
                let true_side: Vec<_> = true_side
                    .into_iter()
                    .zip(proto.true_side)
                    .map(|(value, _proto)| ConvStatement {
                        value,
                        proto: None, //Some(proto),
                    })
                    .collect();
                let false_side: Vec<_> = false_side
                    .into_iter()
                    .zip(proto.false_side)
                    .map(|(value, proto)| ConvStatement {
                        value,
                        proto: Some(proto),
                    })
                    .collect();
                Some((true_side, false_side))
            } else {
                let true_side = x.true_side;
                let false_side = x.false_side;
                let true_side: Vec<_> = true_side
                    .into_iter()
                    .map(|value| ConvStatement { value, proto: None })
                    .collect();
                let false_side: Vec<_> = false_side
                    .into_iter()
                    .map(|value| ConvStatement { value, proto: None })
                    .collect();
                Some((true_side, false_side))
            }
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProtoStatement {
    Assign(ProtoAssignStatement),
    If(ProtoIfStatement),
}

impl ProtoStatement {
    pub fn build_binary(
        &self,
        context: &mut Context,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) -> Option<()> {
        match self {
            ProtoStatement::Assign(x) => x.build_binary(context, builder),
            ProtoStatement::If(x) => x.build_binary(context, builder, is_last),
        }
    }
}

pub struct ConvAssignStatement {
    pub value: AssignStatement,
    pub proto: Option<ProtoAssignStatement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProtoAssignStatement {
    pub dst_offset: i32,
    pub dst_is_ff: bool,
    pub dst_width: usize,
    pub expr: ProtoExpression,
    pub expr_signed: bool,
}

impl ProtoAssignStatement {
    pub fn build_binary(&self, context: &mut Context, builder: &mut FunctionBuilder) -> Option<()> {
        let (payload, mask_xz) =
            self.expr
                .build_binary(context, Some(self.dst_width), self.expr_signed, builder)?;

        //let load_mem_flag = MemFlags::trusted().with_readonly();
        let store_mem_flag = MemFlags::trusted();

        let base_addr = if self.dst_is_ff {
            context.ff_values
        } else {
            context.comb_values
        };

        match self.dst_width {
            8 => {
                builder
                    .ins()
                    .istore8(store_mem_flag, payload, base_addr, self.dst_offset);
                if let Some(mask_xz) = mask_xz {
                    builder
                        .ins()
                        .istore8(store_mem_flag, mask_xz, base_addr, self.dst_offset + 8);
                }
            }
            16 => {
                builder
                    .ins()
                    .istore16(store_mem_flag, payload, base_addr, self.dst_offset);
                if let Some(mask_xz) = mask_xz {
                    builder
                        .ins()
                        .istore16(store_mem_flag, mask_xz, base_addr, self.dst_offset + 8);
                }
            }
            32 => {
                builder
                    .ins()
                    .istore32(store_mem_flag, payload, base_addr, self.dst_offset);
                if let Some(mask_xz) = mask_xz {
                    builder
                        .ins()
                        .istore32(store_mem_flag, mask_xz, base_addr, self.dst_offset + 8);
                }
            }
            64 => {
                builder
                    .ins()
                    .store(store_mem_flag, payload, base_addr, self.dst_offset);
                if let Some(mask_xz) = mask_xz {
                    builder
                        .ins()
                        .store(store_mem_flag, mask_xz, base_addr, self.dst_offset + 8);
                }
            }
            _ => {
                let mask = (1u64 << self.dst_width) - 1;
                let payload = builder.ins().band_imm(payload, mask as i64);

                builder
                    .ins()
                    .store(store_mem_flag, payload, base_addr, self.dst_offset);
                if let Some(mask_xz) = mask_xz {
                    let mask_xz = builder.ins().band_imm(mask_xz, mask as i64);
                    builder
                        .ins()
                        .store(store_mem_flag, mask_xz, base_addr, self.dst_offset + 8);
                }
            }
        }

        Some(())
    }
}

pub struct ConvIfStatement {
    pub value: IfStatement,
    pub proto: Option<ProtoIfStatement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProtoIfStatement {
    pub cond: Option<ProtoExpression>,
    pub true_side: Vec<ProtoStatement>,
    pub false_side: Vec<ProtoStatement>,
}

impl ProtoIfStatement {
    pub fn build_binary(
        &self,
        context: &mut Context,
        builder: &mut FunctionBuilder,
        is_last: bool,
    ) -> Option<()> {
        let true_block = builder.create_block();
        let false_block = builder.create_block();
        let final_block = builder.create_block();

        if let Some(x) = &self.cond {
            // TODO 4-state
            let (cond_payload, _cond_mask_xz) = x.build_binary(context, None, false, builder)?;
            builder
                .ins()
                .brif(cond_payload, true_block, &[], false_block, &[]);
        }

        builder.switch_to_block(true_block);
        let len = self.true_side.len();
        for (i, x) in self.true_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        builder.switch_to_block(false_block);
        let len = self.true_side.len();
        for (i, x) in self.false_side.iter().enumerate() {
            let is_last = is_last && (i + 1 == len);
            x.build_binary(context, builder, is_last)?;
        }
        if is_last {
            builder.ins().return_(&[]);
        } else {
            builder.ins().jump(final_block, &[]);
        }

        builder.switch_to_block(final_block);

        Some(())
    }
}

fn expand_sign(
    dst_width: usize,
    src_width: usize,
    mut payload: CraneliftValue,
    mut mask_xz: Option<CraneliftValue>,
    builder: &mut FunctionBuilder,
) -> (CraneliftValue, Option<CraneliftValue>) {
    if dst_width != src_width {
        let mask = ValueU64::gen_mask(dst_width) ^ ValueU64::gen_mask(src_width);
        let msb = builder.ins().ushr_imm(payload, (src_width - 1) as i64);
        let ext = builder.ins().bor_imm(payload, mask as i64);
        payload = builder.ins().select(msb, ext, payload);
        if let Some(x) = mask_xz {
            let msb_xz = builder.ins().ushr_imm(x, (src_width - 1) as i64);
            let ext_xz = builder.ins().bor_imm(x, mask as i64);
            mask_xz = Some(builder.ins().select(msb_xz, ext_xz, x));
        }
    }
    (payload, mask_xz)
}

pub struct ConvExpression {
    pub value: Expression,
    pub proto: Option<ProtoExpression>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProtoExpression {
    Variable(i32, bool),
    Value(u64, Option<u64>),
    Unary(Op, Box<ProtoExpression>, usize),
    Binary(Box<ProtoExpression>, usize, Op, Box<ProtoExpression>, usize),
}

impl ProtoExpression {
    pub fn build_binary(
        &self,
        context: &mut Context,
        context_width: Option<usize>,
        signed: bool,
        builder: &mut FunctionBuilder,
    ) -> Option<(CraneliftValue, Option<CraneliftValue>)> {
        match self {
            ProtoExpression::Value(payload, mask_xz) => {
                let payload = *payload as i64;
                let payload = builder.ins().iconst(I64, payload);

                let mask_xz = if let Some(mask_xz) = mask_xz {
                    let mask_xz = *mask_xz as i64;
                    let mask_xz = builder.ins().iconst(I64, mask_xz);
                    Some(mask_xz)
                } else {
                    None
                };
                Some((payload, mask_xz))
            }
            ProtoExpression::Variable(offset, is_ff) => {
                let load_mem_flag = MemFlags::trusted().with_readonly();

                let base_addr = if *is_ff {
                    context.ff_values
                } else {
                    context.comb_values
                };

                let payload = builder.ins().load(I64, load_mem_flag, base_addr, *offset);
                let mask_xz = if context.config.use_4state {
                    let mask_xz = builder
                        .ins()
                        .load(I64, load_mem_flag, base_addr, *offset + 8);
                    Some(mask_xz)
                } else {
                    None
                };
                Some((payload, mask_xz))
            }
            ProtoExpression::Unary(op, x, x_width) => {
                let (mut x_payload, mut x_mask_xz) =
                    x.build_binary(context, context_width, signed, builder)?;

                let width = op.eval_unary_width_usize(*x_width, context_width);
                if signed {
                    (x_payload, x_mask_xz) =
                        expand_sign(width, *x_width, x_payload, x_mask_xz, builder);
                }

                let payload = match op {
                    Op::Add => x_payload,
                    Op::Sub => {
                        let mask = ValueU64::gen_mask(width);
                        let x0 = builder.ins().bxor_imm(x_payload, mask as i64);
                        builder.ins().iadd_imm(x0, 1)
                    }
                    Op::BitNot => {
                        let mask = ValueU64::gen_mask(width);
                        builder.ins().bxor_imm(x_payload, mask as i64)
                    }
                    Op::BitAnd => {
                        let mask = ValueU64::gen_mask(*x_width);
                        let ret = builder.ins().icmp_imm(IntCC::Equal, x_payload, mask as i64);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitNand => {
                        let mask = ValueU64::gen_mask(*x_width);
                        let ret = builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, x_payload, mask as i64);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitOr => {
                        let ret = builder.ins().icmp_imm(IntCC::NotEqual, x_payload, 0);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitNor | Op::LogicNot => {
                        let ret = builder.ins().icmp_imm(IntCC::Equal, x_payload, 0);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::BitXor => {
                        let x0 = builder.ins().popcnt(x_payload);
                        builder.ins().urem_imm(x0, 2)
                    }
                    Op::BitXnor => {
                        let x0 = builder.ins().popcnt(x_payload);
                        let x1 = builder.ins().icmp_imm(IntCC::Equal, x0, 0);
                        builder.ins().uextend(I64, x1)
                    }
                    _ => return None,
                };

                match op {
                    Op::Add => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            Some((payload, Some(x_mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::Sub => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = builder.ins().iconst(I64, 0xffffffff);
                            let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);

                            let payload = builder.ins().select(is_xz, context.zero, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitNot => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = ValueU64::gen_mask(width);
                            let x0 = builder.ins().bxor_imm(x_mask_xz, mask as i64);
                            let payload = builder.ins().band(payload, x0);

                            Some((payload, Some(x_mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitAnd | Op::BitNand | Op::BitOr | Op::BitNor | Op::LogicNot => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = ValueU64::gen_mask(*x_width);

                            let (is_one, is_zero, is_x) = match op {
                                Op::BitAnd => {
                                    let x0 = builder.ins().bor(x_payload, x_mask_xz);
                                    let x1 =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x0, mask as i64);
                                    let is_zero = x1;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (None, Some(is_zero), is_x)
                                }
                                Op::BitNand => {
                                    let x0 = builder.ins().bor(x_payload, x_mask_xz);
                                    let x1 =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x0, mask as i64);
                                    let is_one = x1;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (Some(is_one), None, is_x)
                                }
                                Op::BitOr => {
                                    let x0 = builder.ins().bxor_imm(x_mask_xz, mask as i64);
                                    let x1 = builder.ins().band(x_payload, x0);
                                    let x2 = builder.ins().icmp_imm(IntCC::NotEqual, x1, 0);
                                    let is_one = x2;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (Some(is_one), None, is_x)
                                }
                                Op::BitNor | Op::LogicNot => {
                                    let x0 = builder.ins().bxor_imm(x_mask_xz, mask as i64);
                                    let x1 = builder.ins().band(x_payload, x0);
                                    let x2 = builder.ins().icmp_imm(IntCC::NotEqual, x1, 0);
                                    let is_zero = x2;
                                    let is_x =
                                        builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                    (None, Some(is_zero), is_x)
                                }
                                _ => unreachable!(),
                            };

                            if let Some(is_one) = is_one {
                                let one = builder.ins().iconst(I64, 1);
                                let payload = builder.ins().select(is_one, one, context.zero);

                                let x1 = builder.ins().bnot(is_one);
                                let x2 = builder.ins().band(x1, is_x);
                                let mask_xz = builder.ins().select(x2, one, context.zero);

                                Some((payload, Some(mask_xz)))
                            } else if let Some(is_zero) = is_zero {
                                let one = builder.ins().iconst(I64, 1);
                                let x0 = builder.ins().bor(is_zero, is_x);
                                let payload = builder.ins().select(x0, context.zero, one);

                                let x1 = builder.ins().bnot(is_zero);
                                let x2 = builder.ins().band(x1, is_x);
                                let mask_xz = builder.ins().select(x2, one, context.zero);

                                Some((payload, Some(mask_xz)))
                            } else {
                                unreachable!();
                            }
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitXor | Op::BitXnor => {
                        if let Some(x_mask_xz) = x_mask_xz {
                            let mask = builder.ins().iconst(I64, 1);
                            let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);

                            let payload = builder.ins().select(is_xz, context.zero, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            ProtoExpression::Binary(x, x_width, op, y, y_width) => {
                let (mut x_payload, mut x_mask_xz) =
                    x.build_binary(context, context_width, signed, builder)?;
                let (mut y_payload, mut y_mask_xz) =
                    y.build_binary(context, context_width, signed, builder)?;

                if signed {
                    let width = if matches!(
                        op,
                        Op::Div | Op::Rem | Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq
                    ) {
                        64
                    } else {
                        op.eval_binary_width_usize(*x_width, *y_width, context_width)
                    };
                    (x_payload, x_mask_xz) =
                        expand_sign(width, *x_width, x_payload, x_mask_xz, builder);
                    (y_payload, y_mask_xz) =
                        expand_sign(width, *y_width, y_payload, y_mask_xz, builder);
                }

                let payload = match op {
                    Op::Add => builder.ins().iadd(x_payload, y_payload),
                    Op::Sub => builder.ins().isub(x_payload, y_payload),
                    Op::Mul => builder.ins().imul(x_payload, y_payload),
                    Op::Div => {
                        let block0 = builder.create_block();
                        let block1 = builder.create_block();
                        let block2 = builder.create_block();
                        builder.append_block_param(block2, I64);

                        let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                        builder.ins().brif(zero_div, block0, &[], block1, &[]);

                        builder.switch_to_block(block0);
                        let ret = builder.ins().iconst(I64, 0);
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);

                        builder.switch_to_block(block1);
                        let ret = if signed {
                            builder.ins().sdiv(x_payload, y_payload)
                        } else {
                            builder.ins().udiv(x_payload, y_payload)
                        };
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);
                        builder.switch_to_block(block2);
                        builder.block_params(block2)[0]
                    }
                    Op::Rem => {
                        let block0 = builder.create_block();
                        let block1 = builder.create_block();
                        let block2 = builder.create_block();
                        builder.append_block_param(block2, I64);

                        let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                        builder.ins().brif(zero_div, block0, &[], block1, &[]);

                        builder.switch_to_block(block0);
                        let ret = builder.ins().iconst(I64, 0);
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);

                        builder.switch_to_block(block1);
                        let ret = if signed {
                            builder.ins().srem(x_payload, y_payload)
                        } else {
                            builder.ins().urem(x_payload, y_payload)
                        };
                        builder.ins().jump(block2, &[BlockArg::Value(ret)]);
                        builder.switch_to_block(block2);
                        builder.block_params(block2)[0]
                    }
                    Op::BitAnd => builder.ins().band(x_payload, y_payload),
                    Op::BitOr => builder.ins().bor(x_payload, y_payload),
                    Op::BitXor => builder.ins().bxor(x_payload, y_payload),
                    Op::BitXnor => builder.ins().bxor_not(x_payload, y_payload),
                    Op::Eq => {
                        let ret = builder.ins().icmp(IntCC::Equal, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::Ne => {
                        let ret = builder.ins().icmp(IntCC::NotEqual, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::Greater => {
                        let cc = if signed {
                            IntCC::SignedGreaterThan
                        } else {
                            IntCC::UnsignedGreaterThan
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::GreaterEq => {
                        let cc = if signed {
                            IntCC::SignedGreaterThanOrEqual
                        } else {
                            IntCC::UnsignedGreaterThanOrEqual
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::Less => {
                        let cc = if signed {
                            IntCC::SignedLessThan
                        } else {
                            IntCC::UnsignedLessThan
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    Op::LessEq => {
                        let cc = if signed {
                            IntCC::SignedLessThanOrEqual
                        } else {
                            IntCC::UnsignedLessThanOrEqual
                        };
                        let ret = builder.ins().icmp(cc, x_payload, y_payload);
                        builder.ins().uextend(I64, ret)
                    }
                    _ => return None,
                };

                match op {
                    Op::Add
                    | Op::Sub
                    | Op::Mul
                    | Op::Div
                    | Op::Rem
                    | Op::Greater
                    | Op::GreaterEq
                    | Op::Less
                    | Op::LessEq => {
                        let mut is_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                let x_is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                let y_is_xz = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                let is_xz = builder.ins().bor(x_is_xz, y_is_xz);
                                Some(is_xz)
                            }
                            (Some(x_mask_xz), None) => {
                                let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                Some(is_xz)
                            }
                            (None, Some(y_mask_xz)) => {
                                let is_xz = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                Some(is_xz)
                            }
                            (None, None) => None,
                        };

                        if matches!(op, Op::Div | Op::Rem) && context.config.use_4state {
                            let zero_div = builder.ins().icmp_imm(IntCC::Equal, y_payload, 0);
                            if let Some(x) = is_xz {
                                is_xz = Some(builder.ins().bor(x, zero_div));
                            } else {
                                is_xz = Some(zero_div);
                            }
                        }

                        if let Some(is_xz) = is_xz {
                            let mask = if matches!(
                                op,
                                Op::Greater | Op::GreaterEq | Op::Less | Op::LessEq
                            ) {
                                builder.ins().iconst(I64, 1)
                            } else {
                                builder.ins().iconst(I64, 0xffffffff)
                            };

                            let payload = builder.ins().select(is_xz, context.zero, payload);
                            let mask_xz = builder.ins().select(is_xz, mask, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitAnd => {
                        let mask_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // (x_mask_xz & y_mask_xz) | (x_mask_xz & !y_mask_xz & y_payload) | (y_mask_xz & !x_mask_xz & x_payload)
                                let x0 = builder.ins().band(x_mask_xz, y_mask_xz);
                                let x1 = builder.ins().bnot(y_mask_xz);
                                let x2 = builder.ins().band(x_mask_xz, x1);
                                let x3 = builder.ins().band(x2, y_payload);
                                let x4 = builder.ins().bnot(x_mask_xz);
                                let x5 = builder.ins().band(y_mask_xz, x4);
                                let x6 = builder.ins().band(x5, x_payload);
                                let x7 = builder.ins().bor(x0, x3);
                                let x8 = builder.ins().bor(x7, x6);
                                Some(x8)
                            }
                            (Some(x_mask_xz), None) => {
                                // (x_mask_xz & y_payload)
                                let x0 = builder.ins().band(x_mask_xz, y_payload);
                                Some(x0)
                            }
                            (None, Some(y_mask_xz)) => {
                                // (y_mask_xz & x_payload)
                                let x0 = builder.ins().band(y_mask_xz, x_payload);
                                Some(x0)
                            }
                            (None, None) => None,
                        };
                        if let Some(mask_xz) = mask_xz {
                            let x0 = builder.ins().bnot(mask_xz);
                            let payload = builder.ins().band(payload, x0);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitOr => {
                        let mask_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // (x_mask_xz & y_mask_xz) | (x_mask_xz & !y_mask_xz & !y_payload) | (y_mask_xz & !x_mask_xz & !x_payload)
                                let x0 = builder.ins().band(x_mask_xz, y_mask_xz);
                                let x1 = builder.ins().bnot(y_mask_xz);
                                let x2 = builder.ins().bnot(y_payload);
                                let x3 = builder.ins().band(x_mask_xz, x1);
                                let x4 = builder.ins().band(x3, x2);
                                let x5 = builder.ins().bnot(x_mask_xz);
                                let x6 = builder.ins().bnot(x_payload);
                                let x7 = builder.ins().band(y_mask_xz, x5);
                                let x8 = builder.ins().band(x7, x6);
                                let x9 = builder.ins().bor(x0, x4);
                                let x10 = builder.ins().bor(x9, x8);
                                Some(x10)
                            }
                            (Some(x_mask_xz), None) => {
                                // (x_mask_xz & !y_payload)
                                let x0 = builder.ins().bnot(y_payload);
                                let x1 = builder.ins().band(x_mask_xz, x0);
                                Some(x1)
                            }
                            (None, Some(y_mask_xz)) => {
                                // (y_mask_xz & !x_payload)
                                let x0 = builder.ins().bnot(x_payload);
                                let x1 = builder.ins().band(y_mask_xz, x0);
                                Some(x1)
                            }
                            (None, None) => None,
                        };
                        if let Some(mask_xz) = mask_xz {
                            let x0 = builder.ins().bnot(mask_xz);
                            let payload = builder.ins().band(payload, x0);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::BitXor | Op::BitXnor => {
                        let mask_xz = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // x_mask_xz | y_mask_xz
                                let x0 = builder.ins().bor(x_mask_xz, y_mask_xz);
                                Some(x0)
                            }
                            (Some(x_mask_xz), None) => Some(x_mask_xz),
                            (None, Some(y_mask_xz)) => Some(y_mask_xz),
                            (None, None) => None,
                        };
                        if let Some(mask_xz) = mask_xz {
                            let x0 = builder.ins().bnot(mask_xz);
                            let payload = builder.ins().band(payload, x0);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    Op::Eq | Op::Ne => {
                        let is_onezero_x = match (x_mask_xz, y_mask_xz) {
                            (Some(x_mask_xz), Some(y_mask_xz)) => {
                                // (x_payload & !x_mask_xz) != (y_payload & !y_mask_xz)
                                let x0 = builder.ins().bnot(x_mask_xz);
                                let x1 = builder.ins().band(x_payload, x0);
                                let x2 = builder.ins().bnot(y_mask_xz);
                                let x3 = builder.ins().band(y_payload, x2);
                                let x4 = builder.ins().icmp(IntCC::NotEqual, x1, x3);

                                // x_mask_xz != 0 | y_mask_xz != 0
                                let x5 = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                let x6 = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                let x7 = builder.ins().bor(x5, x6);
                                Some((x4, x7))
                            }
                            (Some(x_mask_xz), None) => {
                                // (x_payload & !x_mask_xz) != y_payload
                                let x0 = builder.ins().bnot(x_mask_xz);
                                let x1 = builder.ins().band(x_payload, x0);
                                let x2 = builder.ins().icmp(IntCC::NotEqual, x1, y_payload);

                                // x_mask_xz != 0
                                let x3 = builder.ins().icmp_imm(IntCC::NotEqual, x_mask_xz, 0);
                                Some((x2, x3))
                            }
                            (None, Some(y_mask_xz)) => {
                                // x_payload != (y_payload & !y_mask_xz)
                                let x0 = builder.ins().bnot(y_mask_xz);
                                let x1 = builder.ins().band(y_payload, x0);
                                let x2 = builder.ins().icmp(IntCC::NotEqual, x_payload, x1);

                                // y_mask_xz != 0
                                let x3 = builder.ins().icmp_imm(IntCC::NotEqual, y_mask_xz, 0);
                                Some((x2, x3))
                            }
                            (None, None) => None,
                        };
                        if let Some((is_onezero, is_x)) = is_onezero_x {
                            let one = builder.ins().iconst(I64, 1);

                            let payload = match op {
                                Op::Eq => {
                                    let is_zero = is_onezero;
                                    let x0 = builder.ins().bor(is_zero, is_x);
                                    builder.ins().select(x0, context.zero, one)
                                }
                                Op::Ne => {
                                    let is_one = is_onezero;
                                    builder.ins().select(is_one, one, context.zero)
                                }
                                _ => unreachable!(),
                            };

                            let x1 = builder.ins().bnot(is_onezero);
                            let x2 = builder.ins().band(x1, is_x);
                            let mask_xz = builder.ins().select(x2, one, context.zero);
                            Some((payload, Some(mask_xz)))
                        } else {
                            Some((payload, None))
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}

pub struct ConvDeclaration {
    pub event_statements: HashMap<Event, Vec<ConvStatement>>,
    pub comb_statements: Vec<ConvStatement>,
}
