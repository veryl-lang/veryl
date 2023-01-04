use crate::namespace::Namespace;
use veryl_parser::global_table::StrId;
use veryl_parser::veryl_grammar_trait::{BuiltinType, TypeGroup};
use veryl_parser::veryl_token::Token;

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
