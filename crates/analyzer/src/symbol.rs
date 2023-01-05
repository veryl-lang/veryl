use crate::namespace::Namespace;
use std::fmt;
use veryl_parser::resource_table::StrId;
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
        type_name: StrId,
    },
    Block,
}

impl SymbolKind {
    pub fn to_kind_name(&self) -> String {
        match self {
            SymbolKind::Port { .. } => "port".to_string(),
            SymbolKind::Variable { .. } => "variable".to_string(),
            SymbolKind::Module { .. } => "module".to_string(),
            SymbolKind::Interface { .. } => "interface".to_string(),
            SymbolKind::Function { .. } => "function".to_string(),
            SymbolKind::Parameter { .. } => "parameter".to_string(),
            SymbolKind::Instance { .. } => "instance".to_string(),
            SymbolKind::Block { .. } => "block".to_string(),
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            SymbolKind::Port { direction } => {
                format!("port [{}]", direction)
            }
            SymbolKind::Variable { r#type } => {
                format!("variable [{}]", r#type)
            }
            SymbolKind::Module { parameters, ports } => {
                let mut text = "module [".to_string();
                for parameter in parameters {
                    text.push_str(&format!("{}, ", parameter));
                }
                text.push_str("] [");
                for port in ports {
                    text.push_str(&format!("{}, ", port));
                }
                text.push_str("]");
                text
            }
            SymbolKind::Interface { parameters } => {
                let mut text = "interface [".to_string();
                for parameter in parameters {
                    text.push_str(&format!("{}, ", parameter));
                }
                text.push_str("]");
                text
            }
            SymbolKind::Function { parameters, ports } => {
                let mut text = "function [".to_string();
                for parameter in parameters {
                    text.push_str(&format!("{}, ", parameter));
                }
                text.push_str("] [");
                for port in ports {
                    text.push_str(&format!("{}, ", port));
                }
                text.push_str("]");
                text
            }
            SymbolKind::Parameter { r#type, scope } => match scope {
                ParameterScope::Global => format!("parameter [{}]", r#type),
                ParameterScope::Local => format!("localparam [{}]", r#type),
            },
            SymbolKind::Instance { type_name } => {
                format!("instance [{}]", type_name)
            }
            SymbolKind::Block => "block".to_string(),
        };
        text.fmt(f)
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

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Direction::Input => "input".to_string(),
            Direction::Output => "output".to_string(),
            Direction::Inout => "inout".to_string(),
            Direction::Ref => "ref".to_string(),
            Direction::ModPort { interface, modport } => {
                format!("{}.{}", interface, modport)
            }
        };
        text.fmt(f)
    }
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

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Type::Bit => "bit".to_string(),
            Type::Logic => "logic".to_string(),
            Type::U32 => "u32".to_string(),
            Type::U64 => "u64".to_string(),
            Type::I32 => "i32".to_string(),
            Type::I64 => "i64".to_string(),
            Type::F32 => "f32".to_string(),
            Type::F64 => "f64".to_string(),
            Type::UserDefined(paths) => {
                let mut text = format!("{}", paths.first().unwrap());
                for path in &paths[1..] {
                    text.push_str(&format!("::{}", path));
                }
                text
            }
        };
        text.fmt(f)
    }
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

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{} [{}]", self.name, self.direction);
        text.fmt(f)
    }
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

impl fmt::Display for Parameter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{} [{}]", self.name, self.r#type);
        text.fmt(f)
    }
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
