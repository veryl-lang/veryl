use crate::ir::Shape;
use std::fmt;
use veryl_parser::resource_table::{self, StrId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WidthOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
}

impl fmt::Display for WidthOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            WidthOp::Add => "+",
            WidthOp::Sub => "-",
            WidthOp::Mul => "*",
            WidthOp::Div => "/",
            WidthOp::Rem => "%",
            WidthOp::Pow => "**",
        };
        f.write_str(s)
    }
}

/// Width expression that preserves parameter references (e.g. `W`,
/// `W + 1`) instead of collapsing to a numeric value as `Shape` does.
/// Lets the emitter render `logic [W-1:0]` directly.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WidthExpr {
    Concrete(usize),
    Param(StrId),
    BinOp(Box<WidthExpr>, WidthOp, Box<WidthExpr>),
}

impl WidthExpr {
    pub fn numeric(&self) -> Option<usize> {
        match self {
            WidthExpr::Concrete(n) => Some(*n),
            _ => None,
        }
    }

    pub fn is_parametric(&self) -> bool {
        match self {
            WidthExpr::Concrete(_) => false,
            WidthExpr::Param(_) => true,
            WidthExpr::BinOp(lhs, _, rhs) => lhs.is_parametric() || rhs.is_parametric(),
        }
    }

    pub fn to_sv_expr(&self) -> String {
        match self {
            WidthExpr::Concrete(n) => n.to_string(),
            WidthExpr::Param(id) => resource_table::get_str_value(*id).unwrap_or_default(),
            WidthExpr::BinOp(lhs, op, rhs) => {
                let higher_prec = matches!(op, WidthOp::Mul | WidthOp::Div | WidthOp::Rem);
                let needs_parens = |child: &WidthExpr| {
                    matches!(child, WidthExpr::BinOp(_, WidthOp::Add | WidthOp::Sub, _))
                        && higher_prec
                };
                let lhs_str = if needs_parens(lhs) {
                    format!("({})", lhs.to_sv_expr())
                } else {
                    lhs.to_sv_expr()
                };
                let rhs_str = if needs_parens(rhs) {
                    format!("({})", rhs.to_sv_expr())
                } else {
                    rhs.to_sv_expr()
                };
                format!("{lhs_str} {op} {rhs_str}")
            }
        }
    }

    pub fn to_sv_width_string(&self) -> String {
        match self {
            WidthExpr::Concrete(1) => String::new(),
            WidthExpr::Concrete(n) => format!("[{n}-1:0]"),
            _ => format!("[{}-1:0]", self.to_sv_expr()),
        }
    }

    pub fn from_shape(shape: &Shape) -> Vec<WidthExpr> {
        shape
            .as_slice()
            .iter()
            .filter_map(|x| x.map(WidthExpr::Concrete))
            .collect()
    }

    /// Solve `self == value` for the single `Param` in `self`.
    /// Supports `Param(N)`, `Param(N) ± c`, `c + Param(N)`,
    /// `Param(N) * c`, `c * Param(N)`.
    pub fn solve_for_param(&self, value: usize) -> Option<(StrId, usize)> {
        match self {
            WidthExpr::Param(p) => Some((*p, value)),
            WidthExpr::Concrete(_) => None,
            WidthExpr::BinOp(lhs, op, rhs) => match (lhs.as_ref(), op, rhs.as_ref()) {
                (WidthExpr::Param(p), WidthOp::Add, WidthExpr::Concrete(c)) => {
                    value.checked_sub(*c).map(|v| (*p, v))
                }
                (WidthExpr::Concrete(c), WidthOp::Add, WidthExpr::Param(p)) => {
                    value.checked_sub(*c).map(|v| (*p, v))
                }
                (WidthExpr::Param(p), WidthOp::Sub, WidthExpr::Concrete(c)) => {
                    value.checked_add(*c).map(|v| (*p, v))
                }
                (WidthExpr::Param(p), WidthOp::Mul, WidthExpr::Concrete(c)) if *c != 0 => {
                    if value.is_multiple_of(*c) {
                        Some((*p, value / c))
                    } else {
                        None
                    }
                }
                (WidthExpr::Concrete(c), WidthOp::Mul, WidthExpr::Param(p)) if *c != 0 => {
                    if value.is_multiple_of(*c) {
                        Some((*p, value / c))
                    } else {
                        None
                    }
                }
                _ => None,
            },
        }
    }
}

impl fmt::Display for WidthExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_sv_expr())
    }
}
