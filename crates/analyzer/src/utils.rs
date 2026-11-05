use crate::ir::Op;
use crate::symbol::{Symbol, SymbolId, SymbolKind};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::Expression01Op;

pub(crate) fn resolve_op(op: &Expression01Op) -> (Op, u32) {
    match op {
        Expression01Op::Operator01(x) => {
            let tok = x.operator01.operator01_token.to_string();
            match tok.as_str() {
                "||" => (Op::LogicOr, 1),
                "&&" => (Op::LogicAnd, 2),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator03(_) => (Op::BitOr, 3),
        Expression01Op::Operator04(x) => {
            let tok = x.operator04.operator04_token.to_string();
            match tok.as_str() {
                "^" => (Op::BitXor, 4),
                "~^" => (Op::BitXnor, 4),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator05(_) => (Op::BitAnd, 5),
        Expression01Op::Operator02(x) => {
            let tok = x.operator02.operator02_token.to_string();
            match tok.as_str() {
                "==" => (Op::Eq, 6),
                "!=" => (Op::Ne, 6),
                "==?" => (Op::EqWildcard, 6),
                "!=?" => (Op::NeWildcard, 6),
                "<=" => (Op::LessEq, 7),
                ">=" => (Op::GreaterEq, 7),
                "<:" => (Op::Less, 7),
                ">:" => (Op::Greater, 7),
                "<<<" => (Op::ArithShiftL, 8),
                ">>>" => (Op::ArithShiftR, 8),
                "<<" => (Op::LogicShiftL, 8),
                ">>" => (Op::LogicShiftR, 8),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator06(x) => {
            let tok = x.operator06.operator06_token.to_string();
            match tok.as_str() {
                "+" => (Op::Add, 9),
                "-" => (Op::Sub, 9),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator07(x) => {
            let tok = x.operator07.operator07_token.to_string();
            match tok.as_str() {
                "/" => (Op::Div, 10),
                "%" => (Op::Rem, 10),
                _ => unreachable!(),
            }
        }
        Expression01Op::Star(_) => (Op::Mul, 10),
        Expression01Op::Operator08(_) => (Op::Pow, 11),
    }
}

/// Index of the operator in `ops[lo..hi)` to be used as the subtree root.
pub(crate) fn split_index(ops: &[(Op, u32)], lo: usize, hi: usize) -> usize {
    // Find the lowest-precedence operator in [lo..hi) to use as the subtree
    // root. For left-associative operators pick the rightmost one at the
    // minimum precedence (so `a - b - c` groups as `(a - b) - c`); for the
    // right-associative power operator (precedence 11) pick the leftmost
    // (so `a ** b ** c` groups as `a ** (b ** c)`, matching SystemVerilog).
    let mut min_idx = lo;
    let mut min_prec = ops[lo].1;
    for (i, op) in ops.iter().enumerate().take(hi).skip(lo + 1) {
        // `**` is the only right-associative operator; identify it by Op, not
        // by its precedence number, so the rule survives precedence changes.
        let right_assoc = matches!(op.0, Op::Pow);
        let better = if right_assoc {
            op.1 < min_prec
        } else {
            op.1 <= min_prec
        };
        if better {
            min_idx = i;
            min_prec = op.1;
        }
    }
    min_idx
}

pub(crate) enum InstTypeSource {
    Id(SymbolId),
    Path(SymbolPathNamespace),
}

pub(crate) fn resolve_inst_type(arg: &InstTypeSource) -> Option<Symbol> {
    resolve_inst_type_inner(arg, &mut Vec::new())
}

fn resolve_inst_type_inner(arg: &InstTypeSource, visited: &mut Vec<SymbolId>) -> Option<Symbol> {
    let symbol = match arg {
        InstTypeSource::Id(x) => symbol_table::get(*x)?,
        InstTypeSource::Path(x) => (*symbol_table::resolve(x).ok()?.found).clone(),
    };

    // cyclic-alias guard (alias instantiated): stop if a symbol was revisited.
    if visited.contains(&symbol.id) {
        return None;
    }
    visited.push(symbol.id);

    match &symbol.kind {
        SymbolKind::AliasModule(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type_inner(&InstTypeSource::Path(path), visited);
        }
        SymbolKind::AliasInterface(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type_inner(&InstTypeSource::Path(path), visited);
        }
        SymbolKind::GenericInstance(x) => {
            return resolve_inst_type_inner(&InstTypeSource::Id(x.base), visited);
        }
        SymbolKind::GenericParameter(x) => {
            let proto = x.bound.resolve_proto_bound(&symbol.namespace).ok()?;
            if let Some(symbol) = proto.get_symbol() {
                return resolve_inst_type_inner(&InstTypeSource::Id(symbol.id), visited);
            }
        }
        _ => {}
    }

    Some(symbol)
}
