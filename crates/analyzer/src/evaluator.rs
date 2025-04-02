use crate::symbol::{SymbolId, SymbolKind, Type, TypeKind};
use crate::symbol_table::{self, ResolveError, ResolveResult};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;

#[derive(Clone, Debug)]
pub struct Evaluated {
    pub value: EvaluatedValue,
    pub r#type: EvaluatedType,
    pub errors: Vec<EvaluatedError>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvaluatedValue {
    Fixed(isize),
    FixedArray(Vec<isize>),
    Unknown,
    UnknownStatic,
}

impl EvaluatedValue {
    pub fn get_value(&self) -> Option<isize> {
        if let EvaluatedValue::Fixed(x) = self {
            Some(*x)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EvaluatedType {
    Clock(EvaluatedTypeClock),
    Reset(EvaluatedTypeReset),
    Bit(EvaluatedTypeBit),
    Logic(EvaluatedTypeLogic),
    UserDefined(EvaluatedTypeUserDefined),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EvaluatedTypeClock {
    pub kind: EvaluatedTypeClockKind,
    pub width: Vec<usize>,
    pub array: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EvaluatedTypeClockKind {
    Implicit,
    Posedge,
    Negedge,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EvaluatedTypeReset {
    pub kind: EvaluatedTypeResetKind,
    pub width: Vec<usize>,
    pub array: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EvaluatedTypeResetKind {
    Implicit,
    AsyncHigh,
    AsyncLow,
    SyncHigh,
    SyncLow,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EvaluatedTypeBit {
    pub signed: bool,
    pub width: Vec<usize>,
    pub array: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EvaluatedTypeLogic {
    pub signed: bool,
    pub width: Vec<usize>,
    pub array: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EvaluatedTypeUserDefined {
    pub symbol: SymbolId,
    pub width: Vec<usize>,
    pub array: Vec<usize>,
}

#[derive(Clone, Debug)]
pub enum EvaluatedError {
    InvalidFactor { kind: String, token: Token },
    CallNonFunction { kind: String, token: Token },
    InvalidSelect { kind: String, range: TokenRange },
}

fn reduction<T: Fn(isize, isize) -> isize>(
    value: isize,
    width: Option<usize>,
    func: T,
) -> Option<isize> {
    if let Some(width) = width {
        let mut tmp = value;
        let mut ret = tmp & 1;
        for _ in 1..width {
            tmp >>= 1;
            ret = func(ret, tmp & 1);
        }
        Some(ret)
    } else {
        None
    }
}

impl Evaluated {
    pub fn is_known_static(&self) -> bool {
        matches!(
            self.value,
            EvaluatedValue::Fixed(_)
                | EvaluatedValue::FixedArray(_)
                | EvaluatedValue::UnknownStatic
        )
    }

    pub fn is_clock(&self) -> bool {
        matches!(self.r#type, EvaluatedType::Clock(_))
    }

    pub fn is_reset(&self) -> bool {
        matches!(self.r#type, EvaluatedType::Reset(_))
    }

    pub fn is_explicit_reset(&self) -> bool {
        if let EvaluatedType::Reset(x) = &self.r#type {
            !matches!(x.kind, EvaluatedTypeResetKind::Implicit)
        } else {
            false
        }
    }

    pub fn is_4state(&self) -> bool {
        matches!(
            self.r#type,
            EvaluatedType::Clock(_) | EvaluatedType::Reset(_) | EvaluatedType::Logic(_)
        )
    }

    pub fn is_2state(&self) -> bool {
        matches!(self.r#type, EvaluatedType::Bit(_))
    }

    pub fn get_value(&self) -> Option<isize> {
        self.value.get_value()
    }

    pub fn get_width(&self) -> Option<Vec<usize>> {
        match &self.r#type {
            EvaluatedType::Clock(x) => Some(x.width.clone()),
            EvaluatedType::Reset(x) => Some(x.width.clone()),
            EvaluatedType::Bit(x) => Some(x.width.clone()),
            EvaluatedType::Logic(x) => Some(x.width.clone()),
            EvaluatedType::UserDefined(x) => Some(x.width.clone()),
            EvaluatedType::Unknown => None,
        }
    }

    pub fn get_array(&self) -> Option<Vec<usize>> {
        match &self.r#type {
            EvaluatedType::Clock(x) => Some(x.array.clone()),
            EvaluatedType::Reset(x) => Some(x.array.clone()),
            EvaluatedType::Bit(x) => Some(x.array.clone()),
            EvaluatedType::Logic(x) => Some(x.array.clone()),
            EvaluatedType::UserDefined(x) => Some(x.array.clone()),
            EvaluatedType::Unknown => None,
        }
    }

    pub fn get_total_width(&self) -> Option<usize> {
        let width = match &self.r#type {
            EvaluatedType::Clock(x) => Some(&x.width),
            EvaluatedType::Reset(x) => Some(&x.width),
            EvaluatedType::Bit(x) => Some(&x.width),
            EvaluatedType::Logic(x) => Some(&x.width),
            // TODO calc width of user defined type
            EvaluatedType::UserDefined(_) => None,
            EvaluatedType::Unknown => None,
        };
        if let Some(width) = width {
            if width.is_empty() {
                None
            } else {
                Some(width.iter().product())
            }
        } else {
            None
        }
    }

    pub fn set_value(&mut self, value: isize) {
        if let EvaluatedValue::Fixed(x) = &mut self.value {
            *x = value;
        }
    }

    pub fn set_width(&mut self, width: Vec<usize>) {
        match &mut self.r#type {
            EvaluatedType::Clock(x) => x.width = width,
            EvaluatedType::Reset(x) => x.width = width,
            EvaluatedType::Bit(x) => x.width = width,
            EvaluatedType::Logic(x) => x.width = width,
            EvaluatedType::UserDefined(x) => x.width = width,
            EvaluatedType::Unknown => (),
        }
    }

    pub fn set_array(&mut self, array: Vec<usize>) {
        match &mut self.r#type {
            EvaluatedType::Clock(x) => x.array = array,
            EvaluatedType::Reset(x) => x.array = array,
            EvaluatedType::Bit(x) => x.array = array,
            EvaluatedType::Logic(x) => x.array = array,
            EvaluatedType::UserDefined(x) => x.array = array,
            EvaluatedType::Unknown => (),
        }
    }

    pub fn get_clock_kind(&self) -> Option<EvaluatedTypeClockKind> {
        match &self.r#type {
            EvaluatedType::Clock(x) => Some(x.kind),
            _ => None,
        }
    }

    pub fn get_reset_kind(&self) -> Option<EvaluatedTypeResetKind> {
        match &self.r#type {
            EvaluatedType::Reset(x) => Some(x.kind),
            _ => None,
        }
    }

    pub fn create_unknown() -> Evaluated {
        Evaluated {
            value: EvaluatedValue::Unknown,
            r#type: EvaluatedType::Unknown,
            errors: vec![],
        }
    }

    pub fn set_unknown(&mut self) {
        self.value = EvaluatedValue::Unknown;
        self.r#type = EvaluatedType::Unknown;
    }

    pub fn create_unknown_static() -> Evaluated {
        let mut ret = Self::create_unknown();
        ret.set_unknown_static();
        ret
    }

    pub fn set_unknown_static(&mut self) {
        self.value = EvaluatedValue::UnknownStatic;
        self.r#type = EvaluatedType::Unknown;
    }

    pub fn create_fixed(
        value: isize,
        signed: bool,
        width: Vec<usize>,
        array: Vec<usize>,
    ) -> Evaluated {
        let mut ret = Self::create_unknown();
        ret.set_fixed(value, signed, width, array);
        ret
    }

    pub fn set_fixed(&mut self, value: isize, signed: bool, width: Vec<usize>, array: Vec<usize>) {
        self.value = EvaluatedValue::Fixed(value);
        self.r#type = EvaluatedType::Bit(EvaluatedTypeBit {
            signed,
            width,
            array,
        });
    }

    pub fn create_undefine_fixed(signed: bool, width: Vec<usize>, array: Vec<usize>) -> Evaluated {
        let mut ret = Self::create_unknown();
        ret.set_undefine_fixed(signed, width, array);
        ret
    }

    pub fn set_undefine_fixed(&mut self, signed: bool, width: Vec<usize>, array: Vec<usize>) {
        self.value = EvaluatedValue::UnknownStatic;
        self.r#type = EvaluatedType::Logic(EvaluatedTypeLogic {
            signed,
            width,
            array,
        });
    }

    pub fn create_variable(
        signed: bool,
        is_4state: bool,
        width: Vec<usize>,
        array: Vec<usize>,
    ) -> Evaluated {
        let mut ret = Self::create_unknown();
        ret.set_variable(signed, is_4state, width, array);
        ret
    }

    pub fn set_variable(
        &mut self,
        signed: bool,
        is_4state: bool,
        width: Vec<usize>,
        array: Vec<usize>,
    ) {
        self.value = EvaluatedValue::Unknown;
        if is_4state {
            let new_type = match &self.r#type {
                EvaluatedType::Clock(x) => EvaluatedType::Clock(EvaluatedTypeClock {
                    kind: x.kind,
                    width,
                    array,
                }),
                EvaluatedType::Reset(x) => EvaluatedType::Reset(EvaluatedTypeReset {
                    kind: x.kind,
                    width,
                    array,
                }),
                _ => EvaluatedType::Logic(EvaluatedTypeLogic {
                    signed,
                    width,
                    array,
                }),
            };
            self.r#type = new_type;
        } else {
            self.r#type = EvaluatedType::Bit(EvaluatedTypeBit {
                signed,
                width,
                array,
            });
        }
    }

    pub fn create_clock(
        kind: EvaluatedTypeClockKind,
        width: Vec<usize>,
        array: Vec<usize>,
    ) -> Evaluated {
        let mut ret = Self::create_unknown();
        ret.set_clock(kind, width, array);
        ret
    }

    pub fn set_clock(
        &mut self,
        kind: EvaluatedTypeClockKind,
        width: Vec<usize>,
        array: Vec<usize>,
    ) {
        self.value = EvaluatedValue::Unknown;
        self.r#type = EvaluatedType::Clock(EvaluatedTypeClock { kind, width, array });
    }

    pub fn create_reset(
        kind: EvaluatedTypeResetKind,
        width: Vec<usize>,
        array: Vec<usize>,
    ) -> Evaluated {
        let mut ret = Self::create_unknown();
        ret.set_reset(kind, width, array);
        ret
    }

    pub fn set_reset(
        &mut self,
        kind: EvaluatedTypeResetKind,
        width: Vec<usize>,
        array: Vec<usize>,
    ) {
        self.value = EvaluatedValue::Unknown;
        self.r#type = EvaluatedType::Reset(EvaluatedTypeReset { kind, width, array });
    }

    pub fn create_user_defined(
        symbol: SymbolId,
        width: Vec<usize>,
        array: Vec<usize>,
    ) -> Evaluated {
        let mut ret = Self::create_unknown();
        ret.set_user_defined(symbol, width, array);
        ret
    }

    pub fn set_user_defined(&mut self, symbol: SymbolId, width: Vec<usize>, array: Vec<usize>) {
        self.value = EvaluatedValue::Unknown;
        self.r#type = EvaluatedType::UserDefined(EvaluatedTypeUserDefined {
            symbol,
            width,
            array,
        });
    }

    pub fn select(
        mut self,
        mut beg: Evaluated,
        mut end: Evaluated,
        single: bool,
        range: TokenRange,
    ) -> Evaluated {
        let value = self.get_value();
        let width = self.get_width();
        let array = self.get_array();
        if let (Some(width), Some(array)) = (width, array) {
            if let Some(select_array) = array.first() {
                // select array

                let mut rest: Vec<_> = array[1..].to_vec();
                if let (Some(beg), Some(end)) = (beg.get_value(), end.get_value()) {
                    if beg > end {
                        self.errors.push(EvaluatedError::InvalidSelect {
                            kind: format!("wrong index order [{beg}:{end}]"),
                            range,
                        });
                        self.set_unknown();
                    } else if end >= *select_array as isize {
                        self.errors.push(EvaluatedError::InvalidSelect {
                            kind: format!("out of range [{beg}:{end}] > {select_array}"),
                            range,
                        });
                        self.set_unknown();
                    } else {
                        if single {
                            self.set_array(rest);
                        } else {
                            let mut new_array = vec![(end - beg + 1) as usize];
                            new_array.append(&mut rest);
                            self.set_array(new_array);
                        }
                        self.set_unknown();
                    }
                } else if single {
                    self.set_array(rest);
                    self.set_unknown();
                }
            } else {
                // select width

                let select_width = width.first().unwrap_or(&0);
                let mut rest = if width.is_empty() {
                    vec![]
                } else {
                    width[1..].to_vec()
                };

                if let (Some(beg), Some(end)) = (beg.get_value(), end.get_value()) {
                    if end > beg {
                        self.errors.push(EvaluatedError::InvalidSelect {
                            kind: format!("wrong index order [{beg}:{end}]"),
                            range,
                        });
                        self.set_unknown();
                    } else if beg >= *select_width as isize {
                        self.errors.push(EvaluatedError::InvalidSelect {
                            kind: format!("out of range [{beg}:{end}] > {select_width}"),
                            range,
                        });
                        self.set_unknown();
                    } else {
                        let part_size: usize = if rest.is_empty() {
                            1
                        } else {
                            rest.iter().product()
                        };

                        let end_bit = end * part_size as isize;
                        let beg_bit = beg * part_size as isize;

                        if let Some(value) = value {
                            let mask = !(1 << (beg_bit - end_bit + 1));
                            let new_value = (value >> end_bit) & mask;
                            self.set_value(new_value);
                        }

                        let new_width = if beg == end {
                            if rest.is_empty() { vec![1] } else { rest }
                        } else {
                            let mut new_width = vec![(beg - end + 1) as usize];
                            new_width.append(&mut rest);
                            new_width
                        };

                        self.set_width(new_width);
                    }
                } else if single {
                    let new_width = if rest.is_empty() { vec![1] } else { rest };
                    self.set_width(new_width);
                }
            }
        } else {
            self.set_unknown();
        }

        self.errors.append(&mut beg.errors);
        self.errors.append(&mut end.errors);
        self
    }

    fn binary_op<
        T: Fn(usize, usize, Option<&usize>) -> usize,
        U: Fn(isize, isize) -> Option<isize>,
    >(
        mut left: Evaluated,
        mut right: Evaluated,
        context_width: Option<&usize>,
        calc_width: T,
        calc_value: U,
    ) -> Evaluated {
        // TODO array error

        let is_4state = left.is_4state() | right.is_4state();

        let mut ret = match (
            left.get_value(),
            right.get_value(),
            left.get_total_width(),
            right.get_total_width(),
        ) {
            (Some(value0), Some(value1), Some(width0), Some(width1)) => {
                let value = calc_value(value0, value1);
                let width = calc_width(width0, width1, context_width);
                if let Some(value) = value {
                    Evaluated::create_fixed(value, false, vec![width], vec![])
                } else {
                    Evaluated::create_variable(false, is_4state, vec![width], vec![])
                }
            }
            (_, _, Some(width0), Some(width1)) => {
                let width = calc_width(width0, width1, context_width);
                Evaluated::create_variable(false, is_4state, vec![width], vec![])
            }
            _ => Evaluated::create_unknown(),
        };

        ret.errors.append(&mut left.errors);
        ret.errors.append(&mut right.errors);
        ret
    }

    fn unary_op<T: Fn(usize) -> usize, U: Fn(isize) -> Option<isize>>(
        mut left: Evaluated,
        calc_width: T,
        calc_value: U,
    ) -> Evaluated {
        // TODO array error

        let is_4state = left.is_4state();

        match (left.get_value(), left.get_total_width()) {
            (Some(value0), Some(width0)) => {
                let value = calc_value(value0);
                let width = calc_width(width0);
                if let Some(value) = value {
                    left.set_fixed(value, false, vec![width], vec![]);
                } else {
                    left.set_variable(false, is_4state, vec![width], vec![]);
                }
            }
            (_, Some(width0)) => {
                let width = calc_width(width0);
                left.set_variable(false, is_4state, vec![width], vec![]);
            }
            _ => left.set_unknown(),
        }

        left
    }

    fn pow(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, y, z| x.max(y).max(*z.unwrap_or(&0)),
            |x, y| y.try_into().map(|y| x.checked_pow(y)).ok().flatten(),
        )
    }

    fn div(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, y, z| x.max(y).max(*z.unwrap_or(&0)),
            |x, y| x.checked_div(y),
        )
    }

    fn rem(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, y, z| x.max(y).max(*z.unwrap_or(&0)),
            |x, y| x.checked_rem(y),
        )
    }

    fn mul(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, y, z| x.max(y).max(*z.unwrap_or(&0)),
            |x, y| x.checked_mul(y),
        )
    }

    fn add(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, y, z| x.max(y).max(*z.unwrap_or(&0)),
            |x, y| x.checked_add(y),
        )
    }

    fn sub(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, y, z| x.max(y).max(*z.unwrap_or(&0)),
            |x, y| x.checked_sub(y),
        )
    }

    fn unsigned_shl(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, _, z| x.max(*z.unwrap_or(&0)),
            |x, y| {
                y.try_into()
                    .map(|y| (x as usize).checked_shl(y).map(|x| x as isize))
                    .ok()
                    .flatten()
            },
        )
    }

    fn unsigned_shr(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, _, z| x.max(*z.unwrap_or(&0)),
            |x, y| {
                y.try_into()
                    .map(|y| (x as usize).checked_shr(y).map(|x| x as isize))
                    .ok()
                    .flatten()
            },
        )
    }

    fn signed_shl(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, _, z| x.max(*z.unwrap_or(&0)),
            |x, y| y.try_into().map(|y| x.checked_shl(y)).ok().flatten(),
        )
    }

    fn signed_shr(self, exp: Evaluated, context_width: Option<&usize>) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            context_width,
            |x, _, z| x.max(*z.unwrap_or(&0)),
            |x, y| y.try_into().map(|y| x.checked_shr(y)).ok().flatten(),
        )
    }

    fn le(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some(x.cmp(&y).is_le() as isize),
        )
    }

    fn ge(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some(x.cmp(&y).is_ge() as isize),
        )
    }

    fn lt(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some(x.cmp(&y).is_lt() as isize),
        )
    }

    fn gt(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some(x.cmp(&y).is_gt() as isize),
        )
    }

    fn eq(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some(x.cmp(&y).is_eq() as isize),
        )
    }

    fn ne(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some(x.cmp(&y).is_ne() as isize),
        )
    }

    fn andand(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some((x != 0 && y != 0) as isize),
        )
    }

    fn oror(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(
            self,
            exp,
            None,
            |_, _, _| 1,
            |x, y| Some((x != 0 || y != 0) as isize),
        )
    }

    fn and(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, None, |x, y, _| x.max(y), |x, y| Some(x & y))
    }

    fn or(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, None, |x, y, _| x.max(y), |x, y| Some(x | y))
    }

    fn xor(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, None, |x, y, _| x.max(y), |x, y| Some(x ^ y))
    }

    fn xnor(self, exp: Evaluated) -> Evaluated {
        Self::binary_op(self, exp, None, |x, y, _| x.max(y), |x, y| Some(!(x ^ y)))
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
        let width = self.get_total_width();
        Self::unary_op(self, |_| 1, |x| reduction(x, width, |x, y| x & y))
    }

    fn reduction_or(self) -> Evaluated {
        let width = self.get_total_width();
        Self::unary_op(self, |_| 1, |x| reduction(x, width, |x, y| x | y))
    }

    fn reduction_nand(self) -> Evaluated {
        let width = self.get_total_width();
        Self::unary_op(
            self,
            |_| 1,
            |x| {
                let ret = reduction(x, width, |x, y| x & y);
                ret.map(|x| if x == 0 { 1 } else { 0 })
            },
        )
    }

    fn reduction_nor(self) -> Evaluated {
        let width = self.get_total_width();
        Self::unary_op(
            self,
            |_| 1,
            |x| {
                let ret = reduction(x, width, |x, y| x | y);
                ret.map(|x| if x == 0 { 1 } else { 0 })
            },
        )
    }

    fn reduction_xor(self) -> Evaluated {
        let width = self.get_total_width();
        Self::unary_op(self, |_| 1, |x| reduction(x, width, |x, y| x ^ y))
    }

    fn reduction_xnor(self) -> Evaluated {
        let width = self.get_total_width();
        Self::unary_op(
            self,
            |_| 1,
            |x| {
                let ret = reduction(x, width, |x, y| x ^ y);
                ret.map(|x| if x == 0 { 1 } else { 0 })
            },
        )
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

    pub fn evaluate_select(&mut self, value: &Select) -> (Evaluated, Evaluated, bool) {
        let beg = self.expression(value.expression.as_ref());
        if let Some(x) = &value.select_opt {
            let end = self.expression(x.expression.as_ref());
            match x.select_operator.as_ref() {
                SelectOperator::Colon(_) => (beg, end, false),
                SelectOperator::PlusColon(_) => {
                    let one = Evaluated::create_fixed(1, false, vec![32], vec![]);
                    let calc = beg.clone().add(end, None).sub(one, None);
                    (calc, beg, false)
                }
                SelectOperator::MinusColon(_) => {
                    let one = Evaluated::create_fixed(1, false, vec![32], vec![]);
                    let calc = beg.clone().sub(end, None).add(one, None);
                    (beg, calc, false)
                }
                SelectOperator::Step(_) => (
                    beg.clone().mul(end.clone(), None).add(end.clone(), None),
                    beg.mul(end, None),
                    false,
                ),
            }
        } else {
            (beg.clone(), beg, true)
        }
    }

    fn binary_operator(&mut self, operator: &str, left: Evaluated, right: Evaluated) -> Evaluated {
        match operator {
            "**" => left.pow(right, self.context_width.first()),
            "/" => left.div(right, self.context_width.first()),
            "*" => left.mul(right, self.context_width.first()),
            "%" => left.rem(right, self.context_width.first()),
            "+" => left.add(right, self.context_width.first()),
            "-" => left.sub(right, self.context_width.first()),
            "<<<" => left.signed_shl(right, self.context_width.first()),
            ">>>" => left.signed_shr(right, self.context_width.first()),
            "<<" => left.unsigned_shl(right, self.context_width.first()),
            ">>" => left.unsigned_shr(right, self.context_width.first()),
            "<=" => left.le(right),
            ">=" => left.ge(right),
            "<:" => left.lt(right),
            ">:" => left.gt(right),
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
            _ => Evaluated::create_unknown(),
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
            _ => Evaluated::create_unknown(),
        }
    }

    pub fn type_width(&mut self, x: Type) -> Option<Vec<usize>> {
        match x.kind {
            TypeKind::U32 | TypeKind::I32 | TypeKind::F32 => {
                if x.width.is_empty() {
                    Some(vec![32])
                } else {
                    // TODO error
                    None
                }
            }
            TypeKind::U64 | TypeKind::I64 | TypeKind::F64 => {
                if x.width.is_empty() {
                    Some(vec![64])
                } else {
                    // TODO error
                    None
                }
            }
            TypeKind::Bit
            | TypeKind::Logic
            | TypeKind::Clock
            | TypeKind::ClockPosedge
            | TypeKind::ClockNegedge
            | TypeKind::Reset
            | TypeKind::ResetAsyncHigh
            | TypeKind::ResetAsyncLow
            | TypeKind::ResetSyncHigh
            | TypeKind::ResetSyncLow => {
                if x.width.is_empty() {
                    Some(vec![1])
                } else {
                    let mut ret = Vec::new();
                    for x in &x.width {
                        let width = self.expression(x);
                        if let EvaluatedValue::Fixed(value) = width.value {
                            if let Ok(width) = value.try_into() {
                                ret.push(width);
                            } else {
                                return None;
                            }
                        } else {
                            return None;
                        }
                    }
                    Some(ret)
                }
            }
            _ => None,
        }
    }

    pub fn type_array(&mut self, x: Type) -> Option<Vec<usize>> {
        self.expression_list(&x.array)
    }

    pub fn expression_list(&mut self, x: &[Expression]) -> Option<Vec<usize>> {
        if x.is_empty() {
            Some(vec![])
        } else {
            let mut ret = Vec::new();
            for x in x {
                let width = self.expression(x);
                if let EvaluatedValue::Fixed(value) = width.value {
                    if let Ok(width) = value.try_into() {
                        ret.push(width);
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Some(ret)
        }
    }

    fn exponent(&mut self, _arg: &Exponent) -> Evaluated {
        Evaluated::create_unknown()
    }

    fn fixed_point(&mut self, _arg: &FixedPoint) -> Evaluated {
        Evaluated::create_unknown()
    }

    fn based(&mut self, arg: &Based) -> Evaluated {
        let text = arg.based_token.to_string().replace('_', "");
        if let Some((width, rest)) = text.split_once('\'') {
            let signed = &rest[0..1] == "s";
            let rest = if signed { &rest[1..] } else { rest };
            let (base, value) = rest.split_at(1);
            let radix = match base {
                "b" => 2,
                "o" => 8,
                "d" => 10,
                "h" => 16,
                _ => unreachable!(),
            };
            let width = str::parse::<usize>(width);
            let value = isize::from_str_radix(value, radix);
            match (width, value) {
                (Ok(width), Ok(value)) => {
                    Evaluated::create_fixed(value, signed, vec![width], vec![])
                }
                (Ok(width), _) => Evaluated::create_undefine_fixed(signed, vec![width], vec![]),
                _ => Evaluated::create_unknown_static(),
            }
        } else {
            unreachable!()
        }
    }

    fn base_less(&mut self, arg: &BaseLess) -> Evaluated {
        let text = arg.base_less_token.to_string().replace('_', "");
        if let Ok(value) = str::parse::<isize>(&text) {
            Evaluated::create_fixed(value, false, vec![32], vec![])
        } else {
            Evaluated::create_unknown_static()
        }
    }

    fn all_bit(&mut self, arg: &AllBit) -> Evaluated {
        let text = arg.all_bit_token.to_string();
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

        let width = *self.context_width.last().unwrap_or(&0);
        if unknown {
            Evaluated::create_undefine_fixed(false, vec![width], vec![])
        } else {
            Evaluated::create_fixed(value, false, vec![width], vec![])
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

    fn boolean_literal(&mut self, arg: &BooleanLiteral) -> Evaluated {
        let value = match arg {
            BooleanLiteral::True(_) => 1,
            BooleanLiteral::False(_) => 0,
        };
        Evaluated::create_fixed(value, false, vec![1], vec![])
    }

    pub fn expression(&mut self, arg: &Expression) -> Evaluated {
        self.if_expression(&arg.if_expression)
    }

    fn if_expression(&mut self, arg: &IfExpression) -> Evaluated {
        for x in &arg.if_expression_list {
            let cond = self.expression(&x.expression);

            if let EvaluatedValue::Fixed(1) = cond.value {
                return self.expression(&x.expression0);
            }
        }

        self.expression01(&arg.expression01)
    }

    fn expression01(&mut self, arg: &Expression01) -> Evaluated {
        let mut ret = self.expression02(&arg.expression02);
        for x in &arg.expression01_list {
            let operator = x.operator02.operator02_token.to_string();
            let operand = self.expression02(&x.expression02);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression02(&mut self, arg: &Expression02) -> Evaluated {
        let mut ret = self.expression03(&arg.expression03);
        for x in &arg.expression02_list {
            let operator = x.operator03.operator03_token.to_string();
            let operand = self.expression03(&x.expression03);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression03(&mut self, arg: &Expression03) -> Evaluated {
        let mut ret = self.expression04(&arg.expression04);
        for x in &arg.expression03_list {
            let operator = x.operator04.operator04_token.to_string();
            let operand = self.expression04(&x.expression04);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression04(&mut self, arg: &Expression04) -> Evaluated {
        let mut ret = self.expression05(&arg.expression05);
        for x in &arg.expression04_list {
            let operator = x.operator05.operator05_token.to_string();
            let operand = self.expression05(&x.expression05);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression05(&mut self, arg: &Expression05) -> Evaluated {
        let mut ret = self.expression06(&arg.expression06);
        for x in &arg.expression05_list {
            let operator = x.operator06.operator06_token.to_string();
            let operand = self.expression06(&x.expression06);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression06(&mut self, arg: &Expression06) -> Evaluated {
        let mut ret = self.expression07(&arg.expression07);
        for x in &arg.expression06_list {
            let operator = x.operator07.operator07_token.to_string();
            let operand = self.expression07(&x.expression07);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression07(&mut self, arg: &Expression07) -> Evaluated {
        let mut ret = self.expression08(&arg.expression08);
        for x in &arg.expression07_list {
            let operator = x.operator08.operator08_token.to_string();
            let operand = self.expression08(&x.expression08);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression08(&mut self, arg: &Expression08) -> Evaluated {
        let mut ret = self.expression09(&arg.expression09);
        for x in &arg.expression08_list {
            let operator = x.operator09.operator09_token.to_string();
            let operand = self.expression09(&x.expression09);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression09(&mut self, arg: &Expression09) -> Evaluated {
        let mut ret = self.expression10(&arg.expression10);
        for x in &arg.expression09_list {
            let operator = x.operator10.operator10_token.to_string();
            let operand = self.expression10(&x.expression10);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression10(&mut self, arg: &Expression10) -> Evaluated {
        let mut ret = self.expression11(&arg.expression11);
        for x in &arg.expression10_list {
            let operator = match &*x.expression10_list_group {
                Expression10ListGroup::Operator11(x) => x.operator11.operator11_token.to_string(),
                Expression10ListGroup::Star(x) => x.star.star_token.to_string(),
            };
            let operand = self.expression11(&x.expression11);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression11(&mut self, arg: &Expression11) -> Evaluated {
        let mut ret = self.expression12(&arg.expression12);
        for x in &arg.expression11_list {
            let operator = x.operator12.operator12_token.to_string();
            let operand = self.expression12(&x.expression12);
            ret = self.binary_operator(&operator, ret, operand);
        }
        ret
    }

    fn expression12(&mut self, arg: &Expression12) -> Evaluated {
        let mut ret = self.expression13(&arg.expression13);
        if let Some(x) = &arg.expression12_opt {
            let new_type = match x.casting_type.as_ref() {
                CastingType::Bool(_) => {
                    Some(Evaluated::create_variable(false, true, vec![1], vec![]))
                }
                CastingType::Clock(_) => Some(Evaluated::create_clock(
                    EvaluatedTypeClockKind::Implicit,
                    vec![1],
                    vec![],
                )),
                CastingType::ClockPosedge(_) => Some(Evaluated::create_clock(
                    EvaluatedTypeClockKind::Posedge,
                    vec![1],
                    vec![],
                )),
                CastingType::ClockNegedge(_) => Some(Evaluated::create_clock(
                    EvaluatedTypeClockKind::Negedge,
                    vec![1],
                    vec![],
                )),
                CastingType::Reset(_) => Some(Evaluated::create_reset(
                    EvaluatedTypeResetKind::Implicit,
                    vec![1],
                    vec![],
                )),
                CastingType::ResetAsyncHigh(_) => Some(Evaluated::create_reset(
                    EvaluatedTypeResetKind::AsyncHigh,
                    vec![1],
                    vec![],
                )),
                CastingType::ResetAsyncLow(_) => Some(Evaluated::create_reset(
                    EvaluatedTypeResetKind::AsyncLow,
                    vec![1],
                    vec![],
                )),
                CastingType::ResetSyncHigh(_) => Some(Evaluated::create_reset(
                    EvaluatedTypeResetKind::SyncHigh,
                    vec![1],
                    vec![],
                )),
                CastingType::ResetSyncLow(_) => Some(Evaluated::create_reset(
                    EvaluatedTypeResetKind::SyncLow,
                    vec![1],
                    vec![],
                )),
                _ => None,
            };
            if let Some(x) = new_type {
                // TODO check casting error
                ret.r#type = x.r#type;
            }
            ret
        } else {
            ret
        }
    }

    pub fn expression13(&mut self, arg: &Expression13) -> Evaluated {
        let mut ret = self.factor(&arg.factor);
        for x in arg.expression13_list.iter().rev() {
            let operator = match &*x.expression13_list_group {
                Expression13ListGroup::UnaryOperator(x) => {
                    x.unary_operator.unary_operator_token.to_string()
                }
                Expression13ListGroup::Operator04(x) => x.operator04.operator04_token.to_string(),
                Expression13ListGroup::Operator05(x) => x.operator05.operator05_token.to_string(),
                Expression13ListGroup::Operator06(x) => x.operator06.operator06_token.to_string(),
                Expression13ListGroup::Operator10(x) => x.operator10.operator10_token.to_string(),
            };
            ret = self.unary_operator(&operator, ret);
        }
        ret
    }

    pub fn inst_parameter_item(&mut self, arg: &InstParameterItem) -> Evaluated {
        if let Some(opt) = &arg.inst_parameter_item_opt {
            self.expression(opt.expression.as_ref())
        } else {
            self.identifier(arg.identifier.as_ref())
        }
    }

    fn identifier_helper(&mut self, symbol: Result<ResolveResult, ResolveError>) -> Evaluated {
        if let Ok(symbol) = symbol {
            let mut ret = symbol.found.evaluate();
            if let SymbolKind::Parameter(_) = symbol.found.kind {
                // Parameter is static
                if ret.value == EvaluatedValue::Unknown {
                    ret.value = EvaluatedValue::UnknownStatic;
                }
            }
            ret
        } else {
            Evaluated::create_unknown()
        }
    }

    fn identifier(&mut self, arg: &Identifier) -> Evaluated {
        let symbol = symbol_table::resolve(arg);
        self.identifier_helper(symbol)
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Evaluated {
        let range: TokenRange = arg.into();
        let symbol = symbol_table::resolve(arg);

        let last_select: Vec<_> = arg.last_select();

        let mut ret = self.identifier_helper(symbol);

        for s in &last_select {
            let (beg, end, single) = self.evaluate_select(s);
            ret = ret.select(beg, end, single, range);
        }

        ret
    }

    fn factor(&mut self, arg: &Factor) -> Evaluated {
        match arg {
            Factor::Number(x) => self.number(&x.number),
            Factor::BooleanLiteral(x) => self.boolean_literal(&x.boolean_literal),
            Factor::IdentifierFactor(x) => {
                if let Some(args) = &x.identifier_factor.identifier_factor_opt {
                    match args.identifier_factor_opt_group.as_ref() {
                        IdentifierFactorOptGroup::FunctionCall(args) => {
                            let args = &args.function_call.function_call_opt;
                            let func = x
                                .identifier_factor
                                .expression_identifier
                                .identifier()
                                .to_string();

                            if func.starts_with("$") {
                                self.system_function(&func, args)
                            } else {
                                let mut ret = Evaluated::create_unknown();

                                if let Ok(symbol) = symbol_table::resolve(
                                    x.identifier_factor.expression_identifier.as_ref(),
                                ) {
                                    if !symbol.found.kind.is_function() {
                                        ret.errors.push(EvaluatedError::CallNonFunction {
                                            kind: symbol.found.kind.to_kind_name(),
                                            token: symbol.found.token,
                                        });
                                    }
                                }

                                // TODO return type of function

                                ret
                            }
                        }
                        IdentifierFactorOptGroup::StructConstructor(_) => {
                            Evaluated::create_unknown()
                        }
                    }
                } else {
                    // Identifier
                    self.expression_identifier(x.identifier_factor.expression_identifier.as_ref())
                }
            }
            Factor::LParenExpressionRParen(x) => self.expression(&x.expression),
            Factor::LBraceConcatenationListRBrace(x) => {
                self.concatenation_list(&x.concatenation_list)
            }
            Factor::QuoteLBraceArrayLiteralListRBrace(x) => {
                self.array_literal_list(&x.array_literal_list)
            }
            Factor::CaseExpression(x) => self.case_expression(&x.case_expression),
            Factor::SwitchExpression(x) => self.switch_expression(&x.switch_expression),
            Factor::StringLiteral(_) => Evaluated::create_unknown(),
            Factor::FactorGroup(_) => Evaluated::create_unknown(),
            Factor::InsideExpression(_) => Evaluated::create_unknown(),
            Factor::OutsideExpression(_) => Evaluated::create_unknown(),
            Factor::TypeExpression(_) => Evaluated::create_unknown(),
            Factor::FactorTypeFactor(_) => Evaluated::create_unknown(),
        }
    }

    fn system_function(&mut self, name: &str, args: &Option<FunctionCallOpt>) -> Evaluated {
        let args: Vec<ArgumentItem> = if let Some(x) = args {
            x.argument_list.as_ref().into()
        } else {
            Vec::new()
        };

        match name {
            "$clog2" => {
                if let Some(arg) = args.first() {
                    let arg = self.expression(&arg.argument_expression.expression);
                    if let EvaluatedValue::Fixed(x) = arg.value {
                        let ret = isize::BITS - x.leading_zeros();
                        Evaluated::create_fixed(ret as isize, false, vec![32], vec![])
                    } else {
                        Evaluated::create_unknown()
                    }
                } else {
                    Evaluated::create_unknown()
                }
            }
            _ => Evaluated::create_unknown(),
        }
    }

    fn do_concatenation(&mut self, mut x: Evaluated, mut y: Evaluated) -> Evaluated {
        let mut ret = match (
            x.get_value(),
            y.get_value(),
            x.get_total_width(),
            y.get_total_width(),
        ) {
            (Some(value0), Some(value1), Some(width0), Some(width1)) => {
                let width = width0 + width1;
                if let Some(x) = value0.checked_shl(width1 as u32) {
                    let value = x | value1;
                    Evaluated::create_fixed(value, false, vec![width], vec![])
                } else {
                    Evaluated::create_undefine_fixed(false, vec![width], vec![])
                }
            }
            _ => {
                if x.is_known_static() && y.is_known_static() {
                    Evaluated::create_unknown_static()
                } else {
                    Evaluated::create_unknown()
                }
            }
        };

        ret.errors.append(&mut x.errors);
        ret.errors.append(&mut y.errors);
        ret
    }

    fn concatenation_item(&mut self, arg: &ConcatenationItem) -> Evaluated {
        let e = self.expression(arg.expression.as_ref());
        if let Some(cio) = &arg.concatenation_item_opt {
            let c = self.expression(cio.expression.as_ref());
            if let EvaluatedValue::Fixed(c) = c.value {
                let mut tmp = Evaluated::create_fixed(0, false, vec![0], vec![]);
                for _ in 0..c {
                    tmp = self.do_concatenation(tmp, e.clone());
                }
                tmp
            } else {
                Evaluated::create_unknown()
            }
        } else {
            e
        }
    }

    fn concatenation_list_list(&mut self, arg: &ConcatenationListList) -> Evaluated {
        self.concatenation_item(arg.concatenation_item.as_ref())
    }

    fn concatenation_list(&mut self, arg: &ConcatenationList) -> Evaluated {
        let mut eval_vec = vec![];
        eval_vec.push(self.concatenation_item(arg.concatenation_item.as_ref()));
        for cll in arg.concatenation_list_list.iter() {
            eval_vec.push(self.concatenation_list_list(cll));
        }
        let default_value = Evaluated::create_fixed(0, false, vec![0], vec![]);
        eval_vec.iter().fold(default_value, |acc, x| {
            self.do_concatenation(acc, x.clone())
        })
    }

    fn array_literal_item_group_default_colon_expression(
        &mut self,
        arg: &ArrayLiteralItemGroupDefaulColonExpression,
    ) -> Evaluated {
        match self.expression(arg.expression.as_ref()).value {
            EvaluatedValue::Fixed(_) => Evaluated::create_unknown_static(),
            EvaluatedValue::UnknownStatic => unreachable!(),
            _ => Evaluated::create_unknown(),
        }
    }

    fn array_literal_item_group(&mut self, arg: &ArrayLiteralItemGroup) -> Evaluated {
        match arg {
            ArrayLiteralItemGroup::ExpressionArrayLiteralItemOpt(x) => {
                let mut exp_eval = self.expression(x.expression.as_ref());
                if let Some(alio) = &x.array_literal_item_opt {
                    let repeat_exp = self.expression(alio.expression.as_ref());
                    if let Some(value) = repeat_exp.get_value() {
                        exp_eval.set_array(vec![value as usize]);
                        exp_eval
                    } else {
                        Evaluated::create_unknown()
                    }
                } else {
                    exp_eval
                }
            }
            ArrayLiteralItemGroup::DefaulColonExpression(x) => {
                self.array_literal_item_group_default_colon_expression(x)
            }
        }
    }

    fn array_literal_item(&mut self, arg: &ArrayLiteralItem) -> Evaluated {
        self.array_literal_item_group(arg.array_literal_item_group.as_ref())
    }

    fn array_literal_list_list(&mut self, arg: &ArrayLiteralListList) -> Evaluated {
        self.array_literal_item(arg.array_literal_item.as_ref())
    }

    fn array_literal_list(&mut self, arg: &ArrayLiteralList) -> Evaluated {
        // Currently only checking for `Defaul Colon Expression` syntax
        let mut e = self.array_literal_item(arg.array_literal_item.as_ref());
        if arg.array_literal_list_list.is_empty() {
            e.set_array(vec![1]);
            e
        } else if e.is_known_static() {
            let is_known_static: bool = arg
                .array_literal_list_list
                .iter()
                .map(|x| self.array_literal_list_list(x).is_known_static())
                .fold(true, |acc, b| acc & b);
            if is_known_static {
                Evaluated::create_unknown_static()
            } else {
                Evaluated::create_unknown()
            }
        } else {
            Evaluated::create_unknown()
        }
    }

    fn case_expression(&mut self, _arg: &CaseExpression) -> Evaluated {
        Evaluated::create_unknown()
    }

    fn switch_expression(&mut self, _arg: &SwitchExpression) -> Evaluated {
        Evaluated::create_unknown()
    }
}
