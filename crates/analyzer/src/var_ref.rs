use crate::evaluator::{Evaluated, Evaluator};
use crate::namespace::Namespace;
use crate::symbol::{ConnectTarget, SymbolId};
use crate::symbol_table;
use miette::Result;
use std::convert::{From, TryFrom};
use std::fmt;
use std::ops::RangeInclusive;
use veryl_parser::veryl_grammar_trait::{
    Expression, ExpressionIdentifier, HierarchicalIdentifier, Identifier, Select, SelectOperator,
};
use veryl_parser::veryl_token::Token;

#[derive(Clone, Debug)]
pub struct VarRef {
    pub r#type: VarRefType,
    pub affiliation: VarRefAffiliation,
    pub path: VarRefPath,
}

impl VarRef {
    pub fn is_assign(&self) -> bool {
        matches!(self.r#type, VarRefType::AssignTarget { .. })
    }

    pub fn is_expression(&self) -> bool {
        matches!(self.r#type, VarRefType::ExpressionTarget { .. })
    }
}

#[derive(Clone, Debug)]
pub struct Assign {
    pub path: VarRefPath,
    pub position: AssignPosition,
    pub partial: bool,
}

impl Assign {
    pub fn new(var_ref: &VarRef) -> Self {
        match &var_ref.r#type {
            VarRefType::AssignTarget { position } => Self {
                path: var_ref.path.clone(),
                position: position.clone(),
                partial: var_ref.path.is_partial(),
            },
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum VarRefType {
    AssignTarget { position: AssignPosition },
    ExpressionTarget { r#type: ExpressionTargetType },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarRefAffiliation {
    Module { token: Token },
    Interface { token: Token },
    AlwaysComb { token: Token },
    AlwaysFF { token: Token },
    Function { token: Token },
}

impl VarRefAffiliation {
    pub fn token(&self) -> &Token {
        match self {
            VarRefAffiliation::Module { token } => token,
            VarRefAffiliation::Interface { token } => token,
            VarRefAffiliation::AlwaysComb { token } => token,
            VarRefAffiliation::AlwaysFF { token } => token,
            VarRefAffiliation::Function { token } => token,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum VarRefPathItem {
    Identifier {
        symbol_id: SymbolId,
    },
    SelectSingle {
        index: Evaluated,
    },
    SelectColon {
        msb: Evaluated,
        lsb: Evaluated,
    },
    SelectPlusClon {
        position: Evaluated,
        width: Evaluated,
    },
    SelectMinusColon {
        position: Evaluated,
        width: Evaluated,
    },
    SelectStep {
        index: Evaluated,
        step: Evaluated,
    },
}

impl VarRefPathItem {
    pub fn may_included(&self, x: &VarRefPathItem) -> bool {
        match self {
            VarRefPathItem::Identifier { symbol_id: id_self } => match x {
                VarRefPathItem::Identifier { symbol_id: id_x } => id_self == id_x,
                _ => false,
            },
            _ => match (self.select_range(self), self.select_range(x)) {
                (Some(range_self), Some(range_x)) => {
                    range_self.contains(range_x.start()) || range_x.contains(range_self.start())
                }
                _ => true,
            },
        }
    }

    fn select_range(&self, x: &VarRefPathItem) -> Option<RangeInclusive<isize>> {
        match x {
            VarRefPathItem::SelectSingle {
                index: Evaluated::Fixed { value: index, .. },
            } => Some(*index..=*index),
            VarRefPathItem::SelectColon { msb, lsb } => match (msb, lsb) {
                (Evaluated::Fixed { value: msb, .. }, Evaluated::Fixed { value: lsb, .. }) => {
                    Some(*lsb..=*msb)
                }
                _ => None,
            },
            VarRefPathItem::SelectPlusClon { position, width } => match (position, width) {
                (
                    Evaluated::Fixed {
                        value: position, ..
                    },
                    Evaluated::Fixed { value: width, .. },
                ) => Some(*position..=position + width - 1),
                _ => None,
            },
            VarRefPathItem::SelectMinusColon { position, width } => match (position, width) {
                (
                    Evaluated::Fixed {
                        value: position, ..
                    },
                    Evaluated::Fixed { value: width, .. },
                ) => Some(position - width + 1..=*position),
                _ => None,
            },
            VarRefPathItem::SelectStep { index, step } => match (index, step) {
                (Evaluated::Fixed { value: index, .. }, Evaluated::Fixed { value: step, .. }) => {
                    Some(step * index..=step * (index + 1) - 1)
                }
                _ => None,
            },
            _ => None,
        }
    }
}

impl From<&SymbolId> for VarRefPathItem {
    fn from(arg: &SymbolId) -> Self {
        VarRefPathItem::Identifier { symbol_id: *arg }
    }
}

impl From<&Select> for VarRefPathItem {
    fn from(arg: &Select) -> Self {
        if let Some(ref x) = arg.select_opt {
            let mut evaluator = Evaluator::new();
            let exp0 = evaluator.expression(&arg.expression);
            let exp1 = evaluator.expression(&x.expression);
            match &*x.select_operator {
                SelectOperator::Colon(_) => VarRefPathItem::SelectColon {
                    msb: exp0,
                    lsb: exp1,
                },
                SelectOperator::PlusColon(_) => VarRefPathItem::SelectPlusClon {
                    position: exp0,
                    width: exp1,
                },
                SelectOperator::MinusColon(_) => VarRefPathItem::SelectMinusColon {
                    position: exp0,
                    width: exp1,
                },
                SelectOperator::Step(_) => VarRefPathItem::SelectStep {
                    index: exp0,
                    step: exp1,
                },
            }
        } else {
            let exp = Evaluator::new().expression(&arg.expression);
            VarRefPathItem::SelectSingle { index: exp }
        }
    }
}

impl From<&Expression> for VarRefPathItem {
    fn from(arg: &Expression) -> Self {
        let exp = Evaluator::new().expression(arg);
        VarRefPathItem::SelectSingle { index: exp }
    }
}

impl fmt::Display for VarRefPathItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            VarRefPathItem::Identifier { symbol_id } => {
                if let Some(symbol) = symbol_table::get(*symbol_id) {
                    symbol.token.to_string()
                } else {
                    "".to_string()
                }
            }
            VarRefPathItem::SelectSingle { index } => {
                if let Evaluated::Fixed { value: index, .. } = index {
                    format!("[{}]", index)
                } else {
                    "[]".to_string()
                }
            }
            VarRefPathItem::SelectColon { msb, lsb } => match (msb, lsb) {
                (Evaluated::Fixed { value: msb, .. }, Evaluated::Fixed { value: lsb, .. }) => {
                    format!("[{}:{}]", msb, lsb)
                }
                _ => "[]".to_string(),
            },
            VarRefPathItem::SelectPlusClon { position, width } => match (position, width) {
                (
                    Evaluated::Fixed {
                        value: position, ..
                    },
                    Evaluated::Fixed { value: width, .. },
                ) => {
                    format!("[{}+:{}]", position, width)
                }
                _ => "[]".to_string(),
            },
            VarRefPathItem::SelectMinusColon { position, width } => match (position, width) {
                (
                    Evaluated::Fixed {
                        value: position, ..
                    },
                    Evaluated::Fixed { value: width, .. },
                ) => {
                    format!("[{}-:{}]", position, width)
                }
                _ => "[]".to_string(),
            },
            VarRefPathItem::SelectStep { index, step } => match (index, step) {
                (Evaluated::Fixed { value: index, .. }, Evaluated::Fixed { value: step, .. }) => {
                    format!("[{} step {}]", index, step)
                }
                _ => "[]".to_string(),
            },
        };
        s.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct VarRefPath(Vec<VarRefPathItem>, Vec<SymbolId>);

impl VarRefPath {
    pub fn new(x: Vec<VarRefPathItem>) -> Self {
        let mut full_path = Vec::new();

        for path in &x {
            if let VarRefPathItem::Identifier { symbol_id } = path {
                full_path.push(*symbol_id);
            }
        }

        Self(x, full_path)
    }

    pub fn push(&mut self, x: VarRefPathItem) {
        if let VarRefPathItem::Identifier { symbol_id } = x {
            self.1.push(symbol_id);
        }
        self.0.push(x)
    }

    pub fn pop(&mut self) -> Option<VarRefPathItem> {
        let poped = self.0.pop();
        if let Some(VarRefPathItem::Identifier { .. }) = poped {
            self.1.pop();
        }
        poped
    }

    pub fn included(&self, x: &VarRefPath) -> bool {
        let full_path_self = self.full_path();
        let full_path_x = x.full_path();

        for (i, x) in full_path_x.iter().enumerate() {
            if let Some(path) = full_path_self.get(i) {
                if path != x {
                    return false;
                }
            } else {
                return false;
            }
        }

        true
    }

    pub fn may_fully_included(&self, x: &VarRefPath) -> bool {
        for (i, x) in x.0.iter().enumerate() {
            if let Some(path) = self.0.get(i) {
                if !path.may_included(x) {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }

    pub fn full_path(&self) -> &[SymbolId] {
        &self.1
    }

    pub fn is_partial(&self) -> bool {
        self.0
            .iter()
            .any(|x| !matches!(x, VarRefPathItem::Identifier { .. }))
    }
}

impl fmt::Display for VarRefPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut ret = "".to_string();
        for (i, item) in self.0.iter().enumerate() {
            let s = item.to_string();
            if i != 0 && !s.is_empty() && matches!(item, VarRefPathItem::Identifier { .. }) {
                ret.push('.');
            }
            ret.push_str(&s);
        }
        ret.fmt(f)
    }
}

impl TryFrom<&Identifier> for VarRefPath {
    type Error = ();

    fn try_from(arg: &Identifier) -> Result<Self, Self::Error> {
        if let Ok(symbol) = symbol_table::resolve(arg) {
            let path_items = symbol.full_path.iter().map(VarRefPathItem::from).collect();
            Ok(VarRefPath::new(path_items))
        } else {
            Err(())
        }
    }
}

impl TryFrom<&HierarchicalIdentifier> for VarRefPath {
    type Error = ();

    fn try_from(arg: &HierarchicalIdentifier) -> Result<Self, Self::Error> {
        let mut path_items = Vec::new();

        let mut full_path: Vec<_> = if let Ok(symbol) = symbol_table::resolve(arg) {
            symbol
                .full_path
                .iter()
                .rev()
                .map(VarRefPathItem::from)
                .collect()
        } else {
            return Err(());
        };

        path_items.push(full_path.pop().unwrap());
        for x in &arg.hierarchical_identifier_list {
            path_items.push(VarRefPathItem::from(&*x.select));
        }

        for x in &arg.hierarchical_identifier_list0 {
            path_items.push(full_path.pop().unwrap());
            for x in &x.hierarchical_identifier_list0_list {
                path_items.push(VarRefPathItem::from(&*x.select));
            }
        }

        Ok(VarRefPath::new(path_items))
    }
}

impl TryFrom<&ExpressionIdentifier> for VarRefPath {
    type Error = ();

    fn try_from(arg: &ExpressionIdentifier) -> Result<Self, Self::Error> {
        let mut path_items = Vec::new();

        let mut full_path: Vec<_> = if let Ok(symbol) = symbol_table::resolve(arg) {
            symbol
                .full_path
                .iter()
                .rev()
                .map(VarRefPathItem::from)
                .collect()
        } else {
            return Err(());
        };

        path_items.push(full_path.pop().unwrap());
        for _x in &arg.scoped_identifier.scoped_identifier_list {
            path_items.push(full_path.pop().unwrap());
        }

        for x in &arg.expression_identifier_list {
            path_items.push(VarRefPathItem::from(&*x.select));
        }

        for x in &arg.expression_identifier_list0 {
            path_items.push(full_path.pop().unwrap());
            for x in &x.expression_identifier_list0_list {
                path_items.push(VarRefPathItem::from(&*x.select));
            }
        }

        Ok(VarRefPath::new(path_items))
    }
}

impl TryFrom<(&ConnectTarget, &Namespace)> for VarRefPath {
    type Error = ();

    fn try_from(arg: (&ConnectTarget, &Namespace)) -> Result<Self, Self::Error> {
        if arg.0.is_empty() {
            return Err(());
        }

        if let Ok(symbol) = symbol_table::resolve((&arg.0.path(), arg.1)) {
            let mut path_items = Vec::new();
            let mut full_path: Vec<_> = symbol
                .full_path
                .iter()
                .rev()
                .map(VarRefPathItem::from)
                .collect();

            for (_, selects) in &arg.0.path {
                path_items.push(full_path.pop().unwrap());
                for select in selects {
                    path_items.push(VarRefPathItem::from(select));
                }
            }
            Ok(VarRefPath::new(path_items))
        } else {
            Err(())
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct AssignPosition(pub Vec<AssignPositionType>);

impl AssignPosition {
    pub fn new(x: AssignPositionType) -> Self {
        Self(vec![x])
    }

    pub fn push(&mut self, x: AssignPositionType) {
        self.0.push(x)
    }

    pub fn pop(&mut self) -> Option<AssignPositionType> {
        self.0.pop()
    }
}

impl fmt::Display for AssignPosition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut ret = "".to_string();
        for (i, x) in self.0.iter().enumerate() {
            if i != 0 {
                ret.push('.');
            }
            ret.push_str(&x.token().to_string());
        }
        ret.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignPositionType {
    DeclarationBranch {
        token: Token,
        branches: usize,
    },
    DeclarationBranchItem {
        token: Token,
        index: usize,
    },
    Declaration {
        token: Token,
        r#type: AssignDeclarationType,
    },
    StatementBranch {
        token: Token,
        branches: usize,
        has_default: bool,
        allow_missing_reset_statement: bool,
        r#type: AssignStatementBranchType,
    },
    StatementBranchItem {
        token: Token,
        index: usize,
        r#type: AssignStatementBranchItemType,
    },
    Statement {
        token: Token,
        resettable: bool,
    },
    Connect {
        token: Token,
        maybe: bool,
    },
}

impl AssignPositionType {
    pub fn token(&self) -> &Token {
        match self {
            AssignPositionType::DeclarationBranch { token, .. } => token,
            AssignPositionType::DeclarationBranchItem { token, .. } => token,
            AssignPositionType::Declaration { token, .. } => token,
            AssignPositionType::StatementBranch { token, .. } => token,
            AssignPositionType::StatementBranchItem { token, .. } => token,
            AssignPositionType::Statement { token, .. } => token,
            AssignPositionType::Connect { token, .. } => token,
        }
    }

    pub fn is_maybe(&self) -> bool {
        match self {
            AssignPositionType::Connect { maybe, .. } => *maybe,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignDeclarationType {
    Let,
    AlwaysFF,
    AlwaysComb,
    Assign,
    Inst,
    Function,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignStatementBranchType {
    If,
    IfReset,
    Case,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignStatementBranchItemType {
    If,
    IfReset,
    Else,
    Case,
}

#[derive(Clone, Default, Debug)]
pub struct AssignPositionTree {
    r#type: Option<AssignPositionType>,
    children: Vec<AssignPositionTree>,
}

impl AssignPositionTree {
    pub fn add(&mut self, mut pos: AssignPosition) {
        if pos.0.is_empty() {
            return;
        }

        let mut head: Vec<_> = pos.0.drain(0..1).collect();

        for child in &mut self.children {
            if child.r#type.as_ref() == head.first() {
                child.add(pos);
                return;
            }
        }

        let mut node = AssignPositionTree {
            r#type: Some(head.remove(0)),
            children: vec![],
        };
        node.add(pos);
        self.children.push(node);
    }

    pub fn check_always_comb_uncovered(&self) -> Option<Token> {
        if let Some(AssignPositionType::Declaration { ref r#type, .. }) = self.r#type {
            if *r#type == AssignDeclarationType::AlwaysComb {
                let children: Vec<_> = self
                    .children
                    .iter()
                    .map(|x| x.impl_always_comb_uncovered())
                    .collect();
                if children.iter().any(|x| x.is_none()) {
                    return None;
                } else {
                    return children.into_iter().find(|x| x.is_some()).flatten();
                }
            }
        }

        for child in &self.children {
            let ret = child.check_always_comb_uncovered();
            if ret.is_some() {
                return ret;
            }
        }

        None
    }

    fn impl_always_comb_uncovered(&self) -> Option<Token> {
        match self.r#type {
            Some(AssignPositionType::StatementBranch {
                token,
                branches,
                has_default,
                ..
            }) => {
                if !has_default || self.children.len() != branches {
                    Some(token)
                } else {
                    self.children
                        .iter()
                        .map(|x| x.impl_always_comb_uncovered())
                        .find(|x| x.is_some())
                        .flatten()
                }
            }
            Some(AssignPositionType::StatementBranchItem { .. }) => {
                let children: Vec<_> = self
                    .children
                    .iter()
                    .map(|x| x.impl_always_comb_uncovered())
                    .collect();
                if children.iter().any(|x| x.is_none()) {
                    None
                } else {
                    children.into_iter().find(|x| x.is_some()).flatten()
                }
            }
            Some(AssignPositionType::Statement { .. }) => None,
            _ => unreachable!(),
        }
    }

    pub fn check_always_ff_missing_reset(&self) -> Option<Token> {
        if let Some(AssignPositionType::StatementBranch {
            ref r#type,
            ref token,
            ref allow_missing_reset_statement,
            ..
        }) = self.r#type
        {
            if *r#type == AssignStatementBranchType::IfReset
                && !allow_missing_reset_statement
                && self.is_resettable()
            {
                if let Some(AssignPositionType::StatementBranchItem { ref r#type, .. }) =
                    self.children[0].r#type
                {
                    if *r#type != AssignStatementBranchItemType::IfReset {
                        return Some(*token);
                    }
                }
            }
        }

        for child in &self.children {
            let ret = child.check_always_ff_missing_reset();
            if ret.is_some() {
                return ret;
            }
        }

        None
    }

    fn is_resettable(&self) -> bool {
        if let Some(AssignPositionType::Statement { resettable, .. }) = self.r#type {
            resettable
        } else {
            self.children.iter().any(|x| x.is_resettable())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpressionTargetType {
    Variable,
    Parameter,
    InputPort,
    OutputPort,
    InoutPort,
    RefPort,
}
