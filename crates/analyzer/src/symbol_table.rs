use std::collections::HashMap;
use veryl_parser::global_table::StrId;
use veryl_parser::veryl_grammar_trait::{BuiltinType, TypeGroup};
use veryl_parser::veryl_token::Token;

#[derive(Debug, Clone)]
pub enum SymbolKind {
    Port {
        direction: Direction,
    },
    Variable {
        r#type: Type,
    },
    Module {
        parameters: Vec<Parameter>,
        ports: Vec<Port>,
    },
    Interface {
        parameters: Vec<Parameter>,
    },
    Function {
        parameters: Vec<Parameter>,
        ports: Vec<Port>,
    },
    Parameter {
        r#type: Type,
        scope: ParameterScope,
    },
    Instance {
        name: Vec<StrId>,
    },
}

impl ToString for SymbolKind {
    fn to_string(&self) -> String {
        match self {
            SymbolKind::Port { .. } => "port".to_string(),
            SymbolKind::Variable { .. } => "variable".to_string(),
            SymbolKind::Module { .. } => "module".to_string(),
            SymbolKind::Interface { .. } => "interface".to_string(),
            SymbolKind::Function { .. } => "function".to_string(),
            SymbolKind::Parameter { .. } => "parameter".to_string(),
            SymbolKind::Instance { .. } => "instance".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Direction {
    Input,
    Output,
    Inout,
    Ref,
    ModPort { interface: StrId, modport: StrId },
}

impl From<&veryl_parser::veryl_grammar_trait::Direction> for Direction {
    fn from(value: &veryl_parser::veryl_grammar_trait::Direction) -> Self {
        match value {
            veryl_parser::veryl_grammar_trait::Direction::Input(_) => Direction::Input,
            veryl_parser::veryl_grammar_trait::Direction::Output(_) => Direction::Output,
            veryl_parser::veryl_grammar_trait::Direction::Inout(_) => Direction::Inout,
            veryl_parser::veryl_grammar_trait::Direction::Ref(_) => Direction::Ref,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Type {
    Bit,
    Logic,
    U32,
    U64,
    I32,
    I64,
    F32,
    F64,
    UserDefined(Vec<StrId>),
}

impl From<&veryl_parser::veryl_grammar_trait::Type> for Type {
    fn from(value: &veryl_parser::veryl_grammar_trait::Type) -> Self {
        match &*value.type_group {
            TypeGroup::BuiltinType(x) => match &*x.builtin_type {
                BuiltinType::Logic(_) => Type::Logic,
                BuiltinType::Bit(_) => Type::Bit,
                BuiltinType::U32(_) => Type::U32,
                BuiltinType::U64(_) => Type::U64,
                BuiltinType::I32(_) => Type::I32,
                BuiltinType::I64(_) => Type::I64,
                BuiltinType::F32(_) => Type::F32,
                BuiltinType::F64(_) => Type::F64,
            },
            TypeGroup::ScopedIdentifier(x) => {
                let x = &x.scoped_identifier;
                let mut name = Vec::new();
                name.push(x.identifier.identifier_token.token.text);
                for x in &x.scoped_identifier_list {
                    name.push(x.identifier.identifier_token.token.text);
                }
                Type::UserDefined(name)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Port {
    pub name: StrId,
    pub direction: Direction,
}

impl From<&veryl_parser::veryl_grammar_trait::PortDeclarationItem> for Port {
    fn from(value: &veryl_parser::veryl_grammar_trait::PortDeclarationItem) -> Self {
        let direction: Direction = (&*value.direction).into();
        Port {
            name: value.identifier.identifier_token.token.text,
            direction,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ParameterScope {
    Global,
    Local,
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: StrId,
    pub r#type: Type,
    pub scope: ParameterScope,
}

impl From<&veryl_parser::veryl_grammar_trait::WithParameterItem> for Parameter {
    fn from(value: &veryl_parser::veryl_grammar_trait::WithParameterItem) -> Self {
        let scope = match &*value.with_parameter_item_group {
            veryl_parser::veryl_grammar_trait::WithParameterItemGroup::Parameter(_) => {
                ParameterScope::Global
            }
            veryl_parser::veryl_grammar_trait::WithParameterItemGroup::Localparam(_) => {
                ParameterScope::Local
            }
        };
        let r#type: Type = (&*value.r#type).into();
        Parameter {
            name: value.identifier.identifier_token.token.text,
            r#type,
            scope,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Name {
    Hierarchical(Vec<StrId>),
    Scoped(Vec<StrId>),
}

impl Name {
    pub fn as_slice(&self) -> &[StrId] {
        match self {
            Name::Hierarchical(x) => x.as_slice(),
            Name::Scoped(x) => x.as_slice(),
        }
    }
}

impl Default for Name {
    fn default() -> Self {
        Name::Hierarchical(vec![])
    }
}

impl From<&veryl_parser::veryl_grammar_trait::HierarchicalIdentifier> for Name {
    fn from(value: &veryl_parser::veryl_grammar_trait::HierarchicalIdentifier) -> Self {
        let mut paths = Vec::new();
        paths.push(value.identifier.identifier_token.token.text);
        for x in &value.hierarchical_identifier_list0 {
            paths.push(x.identifier.identifier_token.token.text);
        }
        Name::Hierarchical(paths)
    }
}

impl From<&veryl_parser::veryl_grammar_trait::ScopedIdentifier> for Name {
    fn from(value: &veryl_parser::veryl_grammar_trait::ScopedIdentifier) -> Self {
        let mut paths = Vec::new();
        paths.push(value.identifier.identifier_token.token.text);
        for x in &value.scoped_identifier_list {
            paths.push(x.identifier.identifier_token.token.text);
        }
        Name::Scoped(paths)
    }
}

impl From<&veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifier> for Name {
    fn from(value: &veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifier) -> Self {
        let mut paths = Vec::new();
        paths.push(value.identifier.identifier_token.token.text);
        match &*value.scoped_or_hier_identifier_group {
            veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifierGroup::ColonColonIdentifierScopedOrHierIdentifierGroupList(x) => {
                paths.push(x.identifier.identifier_token.token.text);
                for x in &x.scoped_or_hier_identifier_group_list {
                    paths.push(x.identifier.identifier_token.token.text);
                }
                Name::Scoped(paths)
            },
            veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifierGroup::ScopedOrHierIdentifierGroupList0ScopedOrHierIdentifierGroupList1(x) => {
                for x in &x.scoped_or_hier_identifier_group_list1 {
                    paths.push(x.identifier.identifier_token.token.text);
                }
                Name::Hierarchical(paths)
            },
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct Namespace {
    pub paths: Vec<StrId>,
}

impl Namespace {
    pub fn push(&mut self, path: StrId) {
        self.paths.push(path);
    }

    pub fn pop(&mut self) {
        self.paths.pop();
    }

    pub fn depth(&self) -> usize {
        self.paths.len()
    }

    pub fn included(&self, x: &Namespace) -> bool {
        for (i, x) in x.paths.iter().enumerate() {
            if let Some(path) = self.paths.get(i) {
                if path != x {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub token: Token,
    pub kind: SymbolKind,
    pub namespace: Namespace,
}

impl Symbol {
    pub fn new(token: &Token, kind: SymbolKind, namespace: &Namespace) -> Self {
        Self {
            token: *token,
            kind,
            namespace: namespace.to_owned(),
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    table: HashMap<StrId, Vec<Symbol>>,
}

impl SymbolTable {
    pub fn insert(&mut self, token: &Token, symbol: Symbol) -> bool {
        let entry = self.table.entry(token.text).or_default();
        for item in entry.iter() {
            if symbol.namespace == item.namespace {
                return false;
            }
        }
        entry.push(symbol);
        true
    }

    pub fn get(&self, identifier: &Name, namespace: &Namespace) -> Option<&Symbol> {
        match identifier {
            Name::Hierarchical(x) => self.get_hierarchical(x, namespace),
            Name::Scoped(_) => todo!(),
        }
    }

    fn get_hierarchical(&self, paths: &[StrId], namespace: &Namespace) -> Option<&Symbol> {
        let mut ret = None;
        let mut namespace = namespace.clone();
        for name in paths {
            let mut max_depth = 0;
            ret = None;
            if let Some(symbols) = self.table.get(name) {
                for symbol in symbols {
                    if namespace.included(&symbol.namespace)
                        && symbol.namespace.depth() >= max_depth
                    {
                        ret = Some(symbol);
                        max_depth = symbol.namespace.depth();
                    }
                }

                if let Some(ret) = ret {
                    if let SymbolKind::Instance { ref name } = ret.kind {
                        namespace = Namespace::default();
                        for x in name {
                            namespace.push(*x);
                        }
                    }
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
        ret
    }
}
