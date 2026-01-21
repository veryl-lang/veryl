use crate::AnalyzerError;
use crate::conv::Context;
use crate::ir::{ArrayLiteralItem, Expression, Factor};
use veryl_parser::token_range::TokenRange;

pub fn check_anonymous(
    context: &mut Context,
    expr: &Expression,
    allow_anonymous: bool,
    token: TokenRange,
) {
    let is_anonymous = is_anonymous(expr);
    let has_anonymous = has_anonymous(expr);

    if has_anonymous && !is_anonymous {
        context.insert_error(AnalyzerError::anonymous_identifier_usage(&token));
    }

    if is_anonymous && !allow_anonymous {
        context.insert_error(AnalyzerError::anonymous_identifier_usage(&token));
    }
}

fn is_anonymous(expr: &Expression) -> bool {
    if let Expression::Term(x) = expr {
        matches!(x.as_ref(), Factor::Anonymous(_))
    } else {
        false
    }
}

fn has_anonymous(expr: &Expression) -> bool {
    match expr {
        Expression::Term(x) => matches!(x.as_ref(), Factor::Anonymous(_)),
        Expression::Unary(_, x) => has_anonymous(x),
        Expression::Binary(x, _, y) => has_anonymous(x) | has_anonymous(y),
        Expression::Ternary(x, y, z) => has_anonymous(x) | has_anonymous(y) | has_anonymous(z),
        Expression::Concatenation(x) => {
            let mut ret = false;
            for x in x {
                ret |= has_anonymous(&x.0);

                if let Some(x) = &x.1 {
                    ret |= has_anonymous(x);
                }
            }
            ret
        }
        Expression::ArrayLiteral(x) => {
            let mut ret = false;
            for x in x {
                match x {
                    ArrayLiteralItem::Value(x, y) => {
                        ret |= has_anonymous(x);

                        if let Some(y) = &y {
                            ret |= has_anonymous(y);
                        }
                    }
                    ArrayLiteralItem::Defaul(x) => {
                        ret |= has_anonymous(x);
                    }
                }
            }
            ret
        }
        Expression::StructConstructor(_, x) => {
            let mut ret = false;
            for (_, x) in x {
                ret |= has_anonymous(x);
            }
            ret
        }
    }
}
