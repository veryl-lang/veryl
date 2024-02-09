use crate::symbol::{Type, TypeKind};
use crate::symbol_table::{self, ResolveSymbol};
use veryl_parser::veryl_grammar_trait::*;

#[derive(Clone, Copy, Debug)]
pub enum Evaluated {
    Fixed { width: usize, value: isize },
    Variable { width: usize },
    Unknown,
}

impl Evaluated {
    fn binary_op<T: Fn(usize, usize) -> usize, U: Fn(isize, isize) -> Option<isize>>(
        left: Evaluated,
        right: Evaluated,
        width: T,
        value: U,
    ) -> Evaluated {
        if let (
            Evaluated::Fixed {
                width: width0,
                value: value0,
            },
            Evaluated::Fixed {
                width: width1,
                value: value1,
            },
        ) = (left, right)
        {
            let value = value(value0, value1);
            if let Some(value) = value {
                Evaluated::Fixed {
                    width: width(width0, width1),
                    value,
                }
            } else {
                Evaluated::Variable {
                    width: width(width0, width1),
                }
            }
        } else if let (
            Evaluated::Fixed { width: width0, .. },
            Evaluated::Variable { width: width1 },
        ) = (left, right)
        {
            Evaluated::Variable {
                width: width(width0, width1),
            }
        } else if let (
            Evaluated::Variable { width: width0 },
            Evaluated::Fixed { width: width1, .. },
        ) = (left, right)
        {
            Evaluated::Variable {
                width: width(width0, width1),
            }
        } else if let (
            Evaluated::Variable { width: width0 },
            Evaluated::Variable { width: width1 },
        ) = (left, right)
        {
            Evaluated::Variable {
                width: width(width0, width1),
            }
        } else {
            Evaluated::Unknown
        }
    }

    fn unary_op<T: Fn(usize) -> usize, U: Fn(isize) -> Option<isize>>(
        left: Evaluated,
        width: T,
        value: U,
    ) -> Evaluated {
        if let Evaluated::Fixed {
            width: width0,
            value: value0,
        } = left
        {
            let value = value(value0);
            if let Some(value) = value {
                Evaluated::Fixed {
                    width: width(width0),
                    value,
                }
            } else {
                Evaluated::Variable {
                    width: width(width0),
                }
            }
        } else if let Evaluated::Variable { width: width0 } = left {
            Evaluated::Variable {
                width: width(width0),
            }
        } else {
            Evaluated::Unknown
        }
    }

    fn pow(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            |x, _| x,
            |x, y| y.try_into().map(|y| x.checked_pow(y)).ok().flatten(),
        )
    }

    fn div(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| x.checked_div(y))
    }

    fn rem(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| x.checked_rem(y))
    }

    fn mul(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| x.checked_mul(y))
    }

    fn add(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| x.checked_add(y))
    }

    fn sub(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| x.checked_sub(y))
    }

    fn unsigned_shl(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            |x, _| x,
            |x, y| {
                y.try_into()
                    .map(|y| (x as usize).checked_shl(y).map(|x| x as isize))
                    .ok()
                    .flatten()
            },
        )
    }

    fn unsigned_shr(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            |x, _| x,
            |x, y| {
                y.try_into()
                    .map(|y| (x as usize).checked_shr(y).map(|x| x as isize))
                    .ok()
                    .flatten()
            },
        )
    }

    fn signed_shl(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            |x, _| x,
            |x, y| y.try_into().map(|y| x.checked_shl(y)).ok().flatten(),
        )
    }

    fn signed_shr(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            |x, _| x,
            |x, y| y.try_into().map(|y| x.checked_shr(y)).ok().flatten(),
        )
    }

    fn le(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |_, _| 1, |x, y| Some(x.cmp(&y).is_le() as isize))
    }

    fn ge(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |_, _| 1, |x, y| Some(x.cmp(&y).is_ge() as isize))
    }

    fn lt(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |_, _| 1, |x, y| Some(x.cmp(&y).is_lt() as isize))
    }

    fn gt(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |_, _| 1, |x, y| Some(x.cmp(&y).is_gt() as isize))
    }

    fn eq(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |_, _| 1, |x, y| Some(x.cmp(&y).is_eq() as isize))
    }

    fn ne(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |_, _| 1, |x, y| Some(x.cmp(&y).is_ne() as isize))
    }

    fn andand(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            |_, _| 1,
            |x, y| Some((x != 0 && y != 0) as isize),
        )
    }

    fn oror(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            |_, _| 1,
            |x, y| Some((x != 0 || y != 0) as isize),
        )
    }

    fn and(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| Some(x & y))
    }

    fn or(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| Some(x | y))
    }

    fn xor(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| Some(x ^ y))
    }

    fn xnor(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, |x, y| x.max(y), |x, y| Some(!(x ^ y)))
    }

    fn plus(self) -> Evaluated {
        Self::unary_op(self, |x| x, Some)
    }

    fn minus(self) -> Evaluated {
        Self::unary_op(self, |x| x, |x| Some(-x))
    }

    fn not(self) -> Evaluated {
        Self::unary_op(self, |_| 1, |x| Some((x == 0) as isize))
    }

    fn inv(self) -> Evaluated {
        Self::unary_op(self, |x| x, |x| Some(!x))
    }

    fn reduction_and(self) -> Evaluated {
        Self::unary_op(self, |_| 1, |_| None)
    }

    fn reduction_or(self) -> Evaluated {
        Self::unary_op(self, |_| 1, |_| None)
    }

    fn reduction_nand(self) -> Evaluated {
        Self::unary_op(self, |_| 1, |_| None)
    }

    fn reduction_nor(self) -> Evaluated {
        Self::unary_op(self, |_| 1, |_| None)
    }

    fn reduction_xor(self) -> Evaluated {
        Self::unary_op(self, |_| 1, |_| None)
    }

    fn reduction_xnor(self) -> Evaluated {
        Self::unary_op(self, |_| 1, |_| None)
    }
}

#[derive(Default)]
pub struct Evaluator {
    pub context_width: Vec<usize>,
}

impl Evaluator {
    pub fn new() -> Self {
        Default::default()
    }

    fn binary_operator(&mut self, operator: &str, left: Evaluated, right: Evaluated) -> Evaluated {
        match operator {
            "**" => left.pow(right),
            "/" => left.div(right),
            "*" => left.mul(right),
            "%" => left.rem(right),
            "+" => left.add(right),
            "-" => left.sub(right),
            "<<<" => left.signed_shl(right),
            ">>>" => left.signed_shr(right),
            "<<" => left.unsigned_shl(right),
            ">>" => left.unsigned_shr(right),
            "<=" => left.le(right),
            ">=" => left.ge(right),
            "<" => left.lt(right),
            ">" => left.gt(right),
            "===" => left.eq(right),
            "==?" => left.eq(right),
            "!==" => left.ne(right),
            "!=?" => left.ne(right),
            "==" => left.eq(right),
            "!=" => left.ne(right),
            "&&" => left.andand(right),
            "||" => left.oror(right),
            "&" => left.and(right),
            "^~" => left.xnor(right),
            "^" => left.xor(right),
            "~^" => left.xnor(right),
            "|" => left.or(right),
            _ => Evaluated::Unknown,
        }
    }

    fn unary_operator(&mut self, operator: &str, left: Evaluated) -> Evaluated {
        match operator {
            "+" => left.plus(),
            "-" => left.minus(),
            "!" => left.not(),
            "~" => left.inv(),
            "~&" => left.reduction_nand(),
            "~|" => left.reduction_nor(),
            "&" => left.reduction_and(),
            "|" => left.reduction_or(),
            "^" => left.reduction_xor(),
            "~^" => left.reduction_xnor(),
            "^~" => left.reduction_xnor(),
            _ => Evaluated::Unknown,
        }
    }

    pub fn type_width(&mut self, x: Type) -> Option<usize> {
        match x.kind {
            TypeKind::U32 | TypeKind::I32 | TypeKind::F32 => {
                if x.width.is_empty() {
                    Some(32)
                } else {
                    None
                }
            }
            TypeKind::U64 | TypeKind::I64 | TypeKind::F64 => {
                if x.width.is_empty() {
                    Some(64)
                } else {
                    None
                }
            }
            TypeKind::Bit | TypeKind::Logic => {
                if x.width.len() == 1 {
                    let width = self.expression(&x.width[0]);
                    if let Evaluated::Fixed { value, .. } = width {
                        if let Ok(width) = value.try_into() {
                            Some(width)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn exponent(&mut self, _arg: &Exponent) -> Evaluated {
        Evaluated::Unknown
    }

    fn fixed_point(&mut self, _arg: &FixedPoint) -> Evaluated {
        Evaluated::Unknown
    }

    fn based(&mut self, arg: &Based) -> Evaluated {
        let text = arg.based_token.text().replace('_', "");
        if let Some((width, rest)) = text.split_once('\'') {
            let (base, value) = rest.split_at(1);
            let radix = match base {
                "b" => 2,
                "o" => 8,
                "d" => 10,
                "h" => 16,
                _ => unreachable!(),
            };
            let width = width.parse();
            let value = isize::from_str_radix(value, radix);
            if let (Ok(width), Ok(value)) = (width, value) {
                Evaluated::Fixed { width, value }
            } else {
                Evaluated::Unknown
            }
        } else {
            Evaluated::Unknown
        }
    }

    fn base_less(&mut self, arg: &BaseLess) -> Evaluated {
        let text = arg.base_less_token.text().replace('_', "");
        if let Ok(value) = text.parse() {
            Evaluated::Fixed { width: 32, value }
        } else {
            Evaluated::Unknown
        }
    }

    fn all_bit(&mut self, arg: &AllBit) -> Evaluated {
        let text = arg.all_bit_token.text();
        let mut unknown = false;
        let value = match text.as_str() {
            "'1" => {
                let mut ret: isize = 0;
                for _ in 0..*self.context_width.last().unwrap_or(&0) {
                    if let Some(x) = ret.checked_shl(1) {
                        ret = x;
                    } else {
                        unknown = true;
                    }
                    ret |= 1;
                }
                ret
            }
            "'0" => 0,
            _ => {
                unknown = true;
                0
            }
        };
        if unknown {
            Evaluated::Unknown
        } else {
            Evaluated::Fixed {
                width: *self.context_width.last().unwrap_or(&0),
                value,
            }
        }
    }

    fn number(&mut self, arg: &Number) -> Evaluated {
        match arg {
            Number::IntegralNumber(x) => self.integral_number(&x.integral_number),
            Number::RealNumber(x) => self.real_number(&x.real_number),
        }
    }

    fn integral_number(&mut self, arg: &IntegralNumber) -> Evaluated {
        match arg {
            IntegralNumber::Based(x) => self.based(&x.based),
            IntegralNumber::BaseLess(x) => self.base_less(&x.base_less),
            IntegralNumber::AllBit(x) => self.all_bit(&x.all_bit),
        }
    }

    fn real_number(&mut self, arg: &RealNumber) -> Evaluated {
        match arg {
            RealNumber::FixedPoint(x) => self.fixed_point(&x.fixed_point),
            RealNumber::Exponent(x) => self.exponent(&x.exponent),
        }
    }

    pub fn expression(&mut self, arg: &Expression) -> Evaluated {
        let mut ret = self.expression01(&arg.expression01);
        for x in &arg.expression_list {
            let operator = x.operator01.operator01_token.text();
            let operand = self.expression01(&x.expression01);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression01(&mut self, arg: &Expression01) -> Evaluated {
        let mut ret = self.expression02(&arg.expression02);
        for x in &arg.expression01_list {
            let operator = x.operator02.operator02_token.text();
            let operand = self.expression02(&x.expression02);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression02(&mut self, arg: &Expression02) -> Evaluated {
        let mut ret = self.expression03(&arg.expression03);
        for x in &arg.expression02_list {
            let operator = x.operator03.operator03_token.text();
            let operand = self.expression03(&x.expression03);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression03(&mut self, arg: &Expression03) -> Evaluated {
        let mut ret = self.expression04(&arg.expression04);
        for x in &arg.expression03_list {
            let operator = x.operator04.operator04_token.text();
            let operand = self.expression04(&x.expression04);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression04(&mut self, arg: &Expression04) -> Evaluated {
        let mut ret = self.expression05(&arg.expression05);
        for x in &arg.expression04_list {
            let operator = x.operator05.operator05_token.text();
            let operand = self.expression05(&x.expression05);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression05(&mut self, arg: &Expression05) -> Evaluated {
        let mut ret = self.expression06(&arg.expression06);
        for x in &arg.expression05_list {
            let operator = x.operator06.operator06_token.text();
            let operand = self.expression06(&x.expression06);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression06(&mut self, arg: &Expression06) -> Evaluated {
        let mut ret = self.expression07(&arg.expression07);
        for x in &arg.expression06_list {
            let operator = x.operator07.operator07_token.text();
            let operand = self.expression07(&x.expression07);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression07(&mut self, arg: &Expression07) -> Evaluated {
        let mut ret = self.expression08(&arg.expression08);
        for x in &arg.expression07_list {
            let operator = x.operator08.operator08_token.text();
            let operand = self.expression08(&x.expression08);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression08(&mut self, arg: &Expression08) -> Evaluated {
        let mut ret = self.expression09(&arg.expression09);
        for x in &arg.expression08_list {
            let operator = x.operator09.operator09_token.text();
            let operand = self.expression09(&x.expression09);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression09(&mut self, arg: &Expression09) -> Evaluated {
        let mut ret = self.expression10(&arg.expression10);
        for x in &arg.expression09_list {
            let operator = match &*x.expression09_list_group {
                Expression09ListGroup::Operator10(x) => x.operator10.operator10_token.text(),
                Expression09ListGroup::Star(x) => x.star.star_token.text(),
            };
            let operand = self.expression10(&x.expression10);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression10(&mut self, arg: &Expression10) -> Evaluated {
        let mut ret = self.expression11(&arg.expression11);
        for x in &arg.expression10_list {
            let operator = x.operator11.operator11_token.text();
            let operand = self.expression11(&x.expression11);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression11(&mut self, arg: &Expression11) -> Evaluated {
        self.expression12(&arg.expression12)
    }

    fn expression12(&mut self, arg: &Expression12) -> Evaluated {
        let mut ret = self.factor(&arg.factor);
        for x in arg.expression12_list.iter().rev() {
            let operator = match &*x.expression12_list_group {
                Expression12ListGroup::UnaryOperator(x) => {
                    x.unary_operator.unary_operator_token.text()
                }
                Expression12ListGroup::Operator03(x) => x.operator03.operator03_token.text(),
                Expression12ListGroup::Operator04(x) => x.operator04.operator04_token.text(),
                Expression12ListGroup::Operator05(x) => x.operator05.operator05_token.text(),
                Expression12ListGroup::Operator09(x) => x.operator09.operator09_token.text(),
            };
            ret = self.unary_operator(&operator, ret);
        }
        ret
    }

    fn factor(&mut self, arg: &Factor) -> Evaluated {
        match arg {
            Factor::Number(x) => self.number(&x.number),
            Factor::ExpressionIdentifierFactorOpt(x) => {
                if x.factor_opt.is_some() {
                    // Function call
                    Evaluated::Unknown
                } else {
                    // Identifier
                    let symbol = symbol_table::resolve(x.expression_identifier.as_ref());
                    if let Ok(symbol) = symbol {
                        if let ResolveSymbol::Symbol(symbol) = symbol.found {
                            if let Some(evaluated) = symbol.evaluated.get() {
                                evaluated
                            } else {
                                Evaluated::Unknown
                            }
                        } else {
                            Evaluated::Unknown
                        }
                    } else {
                        Evaluated::Unknown
                    }
                }
            }
            Factor::LParenExpressionRParen(x) => self.expression(&x.expression),
            Factor::LBraceConcatenationListRBrace(x) => {
                self.concatenation_list(&x.concatenation_list)
            }
            Factor::IfExpression(x) => self.if_expression(&x.if_expression),
            Factor::CaseExpression(x) => self.case_expression(&x.case_expression),
            Factor::StringLiteral(_) => Evaluated::Unknown,
            Factor::FactorGroup(_) => Evaluated::Unknown,
            Factor::InsideExpression(_) => Evaluated::Unknown,
            Factor::OutsideExpression(_) => Evaluated::Unknown,
        }
    }

    fn concatenation_list(&mut self, _arg: &ConcatenationList) -> Evaluated {
        Evaluated::Unknown
    }

    fn if_expression(&mut self, _arg: &IfExpression) -> Evaluated {
        Evaluated::Unknown
    }

    fn case_expression(&mut self, _arg: &CaseExpression) -> Evaluated {
        Evaluated::Unknown
    }
}
