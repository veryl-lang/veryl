use crate::namespace::Namespace;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait::{BuiltinType, Expression, TypeGroup};
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::Stringifier;

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
    Port(PortProperty),
    Variable(VariableProperty),
    Module(ModuleProperty),
    Interface(InterfaceProperty),
    Function(FunctionProperty),
    Parameter(ParameterProperty),
    Instance(InstanceProperty),
    Block,
    Package,
}

impl SymbolKind {
    pub fn to_kind_name(&self) -> String {
        match self {
            SymbolKind::Port(_) => "port".to_string(),
            SymbolKind::Variable(_) => "variable".to_string(),
            SymbolKind::Module(_) => "module".to_string(),
            SymbolKind::Interface(_) => "interface".to_string(),
            SymbolKind::Function(_) => "function".to_string(),
            SymbolKind::Parameter(_) => "parameter".to_string(),
            SymbolKind::Instance(_) => "instance".to_string(),
            SymbolKind::Block => "block".to_string(),
            SymbolKind::Package => "package".to_string(),
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            SymbolKind::Port(x) => {
                format!("port [{}]", x.direction)
            }
            SymbolKind::Variable(x) => {
                format!("variable [{}]", x.r#type)
            }
            SymbolKind::Module(x) => {
                let mut text = "module [".to_string();
                for parameter in &x.parameters {
                    text.push_str(&format!("{}, ", parameter));
                }
                text.push_str("] [");
                for port in &x.ports {
                    text.push_str(&format!("{}, ", port));
                }
                text.push(']');
                text
            }
            SymbolKind::Interface(x) => {
                let mut text = "interface [".to_string();
                for parameter in &x.parameters {
                    text.push_str(&format!("{}, ", parameter));
                }
                text.push(']');
                text
            }
            SymbolKind::Function(x) => {
                let mut text = "function [".to_string();
                for parameter in &x.parameters {
                    text.push_str(&format!("{}, ", parameter));
                }
                text.push_str("] [");
                for port in &x.ports {
                    text.push_str(&format!("{}, ", port));
                }
                text.push(']');
                text
            }
            SymbolKind::Parameter(x) => {
                let mut stringifier = Stringifier::new();
                stringifier.expression(&x.value);
                match x.scope {
                    ParameterScope::Global => {
                        format!("parameter [{}] ({})", x.r#type, stringifier.as_str())
                    }
                    ParameterScope::Local => {
                        format!("localparam [{}] ({})", x.r#type, stringifier.as_str())
                    }
                }
            }
            SymbolKind::Instance(x) => {
                format!("instance [{}]", x.type_name)
            }
            SymbolKind::Block => "block".to_string(),
            SymbolKind::Package => "package".to_string(),
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
pub struct VariableProperty {
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct PortProperty {
    pub direction: Direction,
}

#[derive(Debug, Clone)]
pub struct Port {
    pub name: StrId,
    pub property: PortProperty,
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{} [{}]", self.name, self.property.direction);
        text.fmt(f)
    }
}

impl From<&veryl_parser::veryl_grammar_trait::PortDeclarationItem> for Port {
    fn from(value: &veryl_parser::veryl_grammar_trait::PortDeclarationItem) -> Self {
        let direction: Direction = (&*value.direction).into();
        let property = PortProperty { direction };
        Port {
            name: value.identifier.identifier_token.token.text,
            property,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ParameterScope {
    Global,
    Local,
}

#[derive(Debug, Clone)]
pub struct ParameterProperty {
    pub r#type: Type,
    pub scope: ParameterScope,
    pub value: Expression,
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: StrId,
    pub property: ParameterProperty,
}

impl fmt::Display for Parameter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{} [{}]", self.name, self.property.r#type);
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
        let property = ParameterProperty {
            r#type,
            scope,
            value: *value.expression.clone(),
        };
        Parameter {
            name: value.identifier.identifier_token.token.text,
            property,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleProperty {
    pub parameters: Vec<Parameter>,
    pub ports: Vec<Port>,
}

#[derive(Debug, Clone)]
pub struct InterfaceProperty {
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Clone)]
pub struct FunctionProperty {
    pub parameters: Vec<Parameter>,
    pub ports: Vec<Port>,
}

#[derive(Debug, Clone)]
pub struct InstanceProperty {
    pub type_name: StrId,
}
