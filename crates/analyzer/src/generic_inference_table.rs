//! Generic argument inference. Pass1 records calls that omitted
//! `::<…>`; `analyze_post_pass1` matches each argument type against the
//! callee port's `WidthExpr` pattern and writes the inferred arguments
//! keyed by the callee identifier token.

use crate::HashMap;
use crate::ir::{WidthExpr, WidthOp};
use crate::literal::Literal;
use crate::literal_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{GenericParameterProperty, Symbol, SymbolKind};
use crate::symbol_path::{GenericSymbol, GenericSymbolPath, GenericSymbolPathKind, SymbolPath};
use crate::symbol_table;
use crate::value::Value;
use std::cell::RefCell;
use veryl_parser::Stringifier;
use veryl_parser::resource_table::{self, StrId, TokenId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::{self as syntax_tree, Expression};
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::VerylWalker;

#[derive(Clone, Debug)]
pub struct PendingEntry {
    pub call_token_id: TokenId,
    pub call_token: Token,
    pub path: GenericSymbolPath,
    pub arg_exprs: Vec<Expression>,
    pub namespace: Namespace,
}

thread_local! {
    static PENDING: RefCell<Vec<PendingEntry>> = const { RefCell::new(Vec::new()) };
    static INFERRED: RefCell<HashMap<TokenId, Vec<GenericSymbolPath>>>
        = RefCell::new(HashMap::default());
}

pub fn push_pending(entry: PendingEntry) {
    PENDING.with(|f| f.borrow_mut().push(entry));
}

pub fn drain_pending() -> Vec<PendingEntry> {
    PENDING.with(|f| f.borrow_mut().drain(..).collect())
}

pub fn insert_inferred(call_token_id: TokenId, args: Vec<GenericSymbolPath>) {
    INFERRED.with(|f| {
        f.borrow_mut().insert(call_token_id, args);
    });
}

pub fn get_inferred(call_token_id: TokenId) -> Option<Vec<GenericSymbolPath>> {
    INFERRED.with(|f| f.borrow().get(&call_token_id).cloned())
}

/// Apply inferred generic arguments to the last path segment when the call
/// site omitted `::<…>`.
pub fn apply_inferred_args(path: &mut GenericSymbolPath, symbol: &Symbol) -> InferredApply {
    let Some(last) = path.paths.last_mut() else {
        return InferredApply::NotApplicable;
    };
    if !last.arguments.is_empty() {
        return InferredApply::NotApplicable;
    }
    if !matches!(symbol.kind, SymbolKind::Function(_)) {
        return InferredApply::NotApplicable;
    }
    if symbol.generic_parameters().is_empty() {
        return InferredApply::NotApplicable;
    }
    if let Some(inferred) = get_inferred(last.base.id) {
        last.arguments = inferred;
        InferredApply::Applied
    } else {
        InferredApply::Missing
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferredApply {
    /// Inference does not apply (non-generic, non-function, or args present).
    NotApplicable,
    /// Call site expects inference, and args were applied.
    Applied,
    /// Call site expects inference, but no inferred args were recorded.
    Missing,
}

pub fn clear() {
    PENDING.with(|f| f.borrow_mut().clear());
    INFERRED.with(|f| f.borrow_mut().clear());
}

pub fn resolve_pending() {
    let saved = namespace_table::get_default();
    let entries = drain_pending();
    for entry in entries {
        if let Some(args) = try_infer_entry(&entry) {
            insert_inferred(entry.call_token_id, args);
        }
    }
    namespace_table::set_default(&saved.paths);
}

fn try_infer_entry(entry: &PendingEntry) -> Option<Vec<GenericSymbolPath>> {
    if entry.arg_exprs.is_empty() {
        return None;
    }

    namespace_table::set_default(&entry.namespace.paths);

    if entry.path.paths.len() != 1 {
        return None;
    }
    let symbol_path: SymbolPath = entry.path.generic_path();
    let symbol = symbol_table::resolve((&symbol_path, &entry.namespace)).ok()?;
    let generic_params = symbol.found.generic_parameters();
    if generic_params.is_empty() {
        return None;
    }
    let ports = match &symbol.found.kind {
        SymbolKind::Function(x) => x.ports.clone(),
        _ => return None,
    };

    let mut bindings: HashMap<StrId, usize> = HashMap::default();
    for (i, port) in ports.iter().enumerate() {
        let Some(arg_expr) = entry.arg_exprs.get(i) else {
            break;
        };
        let Some(port_symbol) = symbol_table::get(port.symbol) else {
            continue;
        };
        let port_type = match &port_symbol.kind {
            SymbolKind::Port(x) => x.r#type.clone(),
            SymbolKind::Variable(x) => x.r#type.clone(),
            _ => continue,
        };
        if port_type.width.len() != 1 {
            continue;
        }
        let Some(pattern) = port_expr_to_width_expr(&port_type.width[0], &generic_params) else {
            continue;
        };
        let Some(arg_width) = resolve_argument_width(arg_expr) else {
            continue;
        };

        if let Some((param, value)) = pattern.solve_for_param(arg_width) {
            if let Some(&existing) = bindings.get(&param) {
                if existing != value {
                    return None;
                }
            } else {
                bindings.insert(param, value);
            }
        } else if let WidthExpr::Concrete(c) = &pattern
            && *c != arg_width
        {
            return None;
        }
    }

    let mut result: Vec<GenericSymbolPath> = Vec::new();
    for (name, prop) in &generic_params {
        if let Some(&value) = bindings.get(name) {
            result.push(numeric_arg_path(&entry.call_token, value));
        } else if prop.default_value.is_some() {
            break;
        } else {
            return None;
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn port_expr_to_width_expr(
    expr: &Expression,
    generic_params: &[(StrId, GenericParameterProperty)],
) -> Option<WidthExpr> {
    let if_expr = &*expr.if_expression;
    if !if_expr.if_expression_list.is_empty() {
        return None;
    }
    let e01 = &*if_expr.expression01;
    if e01.expression01_list.is_empty() {
        expression02_to_width_expr(&e01.expression02, generic_params)
    } else {
        if e01.expression01_list.len() != 1 {
            return None;
        }
        let lhs = expression02_to_width_expr(&e01.expression02, generic_params)?;
        let op_item = &e01.expression01_list[0];
        let rhs = expression02_to_width_expr(&op_item.expression02, generic_params)?;
        let op = operator_text_to_width_op(&operator_text(&op_item.expression01_op))?;
        Some(WidthExpr::BinOp(Box::new(lhs), op, Box::new(rhs)))
    }
}

fn expression02_to_width_expr(
    e02: &syntax_tree::Expression02,
    generic_params: &[(StrId, GenericParameterProperty)],
) -> Option<WidthExpr> {
    if !e02.expression02_list.is_empty() || e02.expression02_opt.is_some() {
        return None;
    }
    factor_to_width_expr(&e02.factor, generic_params)
}

fn factor_to_width_expr(
    factor: &syntax_tree::Factor,
    generic_params: &[(StrId, GenericParameterProperty)],
) -> Option<WidthExpr> {
    match factor {
        syntax_tree::Factor::Number(_) => {
            let text = factor_text(factor);
            parse_plain_integer(&text).map(WidthExpr::Concrete)
        }
        syntax_tree::Factor::IdentifierFactor(id) => {
            let id = &id.identifier_factor;
            if id.identifier_factor_opt.is_some() {
                return None;
            }
            let scoped = &*id.expression_identifier;
            if scoped.expression_identifier_opt.is_some()
                || !scoped.expression_identifier_list.is_empty()
                || !scoped.expression_identifier_list0.is_empty()
            {
                return None;
            }
            let name = scoped.scoped_identifier.identifier().token.text;
            if generic_params.iter().any(|(p, _)| *p == name) {
                Some(WidthExpr::Param(name))
            } else {
                None
            }
        }
        syntax_tree::Factor::LParenExpressionRParen(x) => {
            port_expr_to_width_expr(&x.expression, generic_params)
        }
        _ => None,
    }
}

fn factor_text(factor: &syntax_tree::Factor) -> String {
    let mut s = Stringifier::new();
    s.factor(factor);
    s.as_str().to_string()
}

fn parse_plain_integer(text: &str) -> Option<usize> {
    let trimmed = text.trim();
    trimmed.parse::<usize>().ok()
}

fn operator_text(op: &syntax_tree::Expression01Op) -> String {
    let mut s = Stringifier::new();
    s.expression01_op(op);
    s.as_str().trim().to_string()
}

fn operator_text_to_width_op(text: &str) -> Option<WidthOp> {
    match text {
        "+" => Some(WidthOp::Add),
        "-" => Some(WidthOp::Sub),
        "*" => Some(WidthOp::Mul),
        "/" => Some(WidthOp::Div),
        "%" => Some(WidthOp::Rem),
        _ => None,
    }
}

fn resolve_argument_width(expr: &Expression) -> Option<usize> {
    let if_expr = &*expr.if_expression;
    if !if_expr.if_expression_list.is_empty() {
        return None;
    }
    let e01 = &*if_expr.expression01;
    if !e01.expression01_list.is_empty() {
        return None;
    }
    let e02 = &*e01.expression02;
    if !e02.expression02_list.is_empty() || e02.expression02_opt.is_some() {
        return None;
    }
    let factor = &*e02.factor;
    let id_factor = match factor {
        syntax_tree::Factor::IdentifierFactor(x) => &x.identifier_factor,
        _ => return None,
    };
    if id_factor.identifier_factor_opt.is_some() {
        return None;
    }
    let scoped = &*id_factor.expression_identifier;
    if scoped.expression_identifier_opt.is_some()
        || !scoped.expression_identifier_list.is_empty()
        || !scoped.expression_identifier_list0.is_empty()
    {
        return None;
    }
    let symbol = symbol_table::resolve(scoped.scoped_identifier.as_ref()).ok()?;
    let var_type = match &symbol.found.kind {
        SymbolKind::Variable(x) => &x.r#type,
        SymbolKind::Parameter(x) => &x.r#type,
        _ => return None,
    };
    if var_type.width.len() != 1 {
        return None;
    }
    let mut s = Stringifier::new();
    s.expression(&var_type.width[0]);
    parse_plain_integer(s.as_str().trim())
}

fn numeric_arg_path(base: &Token, value: usize) -> GenericSymbolPath {
    let text_id = resource_table::insert_str(&value.to_string());
    // A fresh token id keeps the synthetic literal separate from the call-site
    // token. Without the new id, `literal_table::get(token.id)` would miss,
    // and downstream `to_literal()` lookups during IR generation would fail.
    let token = if let Some(path) = base.source.get_path() {
        Token::generate(text_id, path)
    } else {
        let mut token = *base;
        token.text = text_id;
        token
    };
    literal_table::insert(token.id, Literal::Value(Value::new(value as u64, 32, true)));
    GenericSymbolPath {
        paths: vec![GenericSymbol {
            base: token,
            arguments: Vec::new(),
        }],
        kind: GenericSymbolPathKind::ValueLiteral,
        range: TokenRange::default(),
    }
}
