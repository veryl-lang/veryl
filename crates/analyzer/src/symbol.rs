use crate::namespace::Namespace;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::Stringifier;

#[derive(Debug, Clone)]
pub struct Symbol {
    pub token: Token,
    pub kind: SymbolKind,
    pub namespace: Namespace,
    pub references: Vec<Token>,
}

impl Symbol {
    pub fn new(token: &Token, kind: SymbolKind, namespace: &Namespace) -> Self {
        Self {
            token: *token,
            kind,
            namespace: namespace.to_owned(),
            references: Vec::new(),
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
    Struct,
    StructMember(StructMemberProperty),
    Enum(EnumProperty),
    EnumMember(EnumMemberProperty),
    Modport(ModportProperty),
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
            SymbolKind::Struct => "struct".to_string(),
            SymbolKind::StructMember(_) => "struct member".to_string(),
            SymbolKind::Enum(_) => "enum".to_string(),
            SymbolKind::EnumMember(_) => "enum member".to_string(),
            SymbolKind::Modport(_) => "modport".to_string(),
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            SymbolKind::Port(x) => {
                if let Some(ref r#type) = x.r#type {
                    format!("port ({} {})", x.direction, r#type)
                } else {
                    format!("port ({})", x.direction)
                }
            }
            SymbolKind::Variable(x) => {
                format!("variable ({})", x.r#type)
            }
            SymbolKind::Module(x) => {
                format!(
                    "module ({} params, {} ports)",
                    x.parameters.len(),
                    x.ports.len()
                )
            }
            SymbolKind::Interface(x) => {
                format!("interface ({} params)", x.parameters.len())
            }
            SymbolKind::Function(x) => {
                format!(
                    "function ({} params, {} args)",
                    x.parameters.len(),
                    x.ports.len()
                )
            }
            SymbolKind::Parameter(x) => {
                let mut stringifier = Stringifier::new();
                stringifier.expression(&x.value);
                match x.scope {
                    ParameterScope::Global => {
                        format!("parameter ({}) = {}", x.r#type, stringifier.as_str())
                    }
                    ParameterScope::Local => {
                        format!("localparam ({}) = {}", x.r#type, stringifier.as_str())
                    }
                }
            }
            SymbolKind::Instance(x) => {
                format!("instance ({})", x.type_name)
            }
            SymbolKind::Block => "block".to_string(),
            SymbolKind::Package => "package".to_string(),
            SymbolKind::Struct => "struct".to_string(),
            SymbolKind::StructMember(x) => {
                format!("struct member ({})", x.r#type)
            }
            SymbolKind::Enum(x) => {
                format!("enum ({})", x.r#type)
            }
            SymbolKind::EnumMember(x) => {
                if let Some(ref x) = x.value {
                    let mut stringifier = Stringifier::new();
                    stringifier.expression(x);
                    format!("enum member = {}", stringifier.as_str())
                } else {
                    "enum member".to_string()
                }
            }
            SymbolKind::Modport(x) => {
                format!("modport ({} ports)", x.members.len())
            }
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
    Interface,
    Modport,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Direction::Input => "input".to_string(),
            Direction::Output => "output".to_string(),
            Direction::Inout => "inout".to_string(),
            Direction::Ref => "ref".to_string(),
            Direction::Interface => "interface".to_string(),
            Direction::Modport => "modport".to_string(),
        };
        text.fmt(f)
    }
}

impl From<&syntax_tree::Direction> for Direction {
    fn from(value: &syntax_tree::Direction) -> Self {
        match value {
            syntax_tree::Direction::Input(_) => Direction::Input,
            syntax_tree::Direction::Output(_) => Direction::Output,
            syntax_tree::Direction::Inout(_) => Direction::Inout,
            syntax_tree::Direction::Ref(_) => Direction::Ref,
            syntax_tree::Direction::Modport(_) => Direction::Modport,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Type {
    modifier: Option<TypeModifier>,
    kind: TypeKind,
    width: Vec<syntax_tree::Expression>,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
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

#[derive(Debug, Clone)]
pub enum TypeModifier {
    Tri,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        if let Some(x) = &self.modifier {
            match x {
                TypeModifier::Tri => text.push_str("tri "),
            }
        }
        match &self.kind {
            TypeKind::Bit => text.push_str("bit"),
            TypeKind::Logic => text.push_str("logic"),
            TypeKind::U32 => text.push_str("u32"),
            TypeKind::U64 => text.push_str("u64"),
            TypeKind::I32 => text.push_str("i32"),
            TypeKind::I64 => text.push_str("i64"),
            TypeKind::F32 => text.push_str("f32"),
            TypeKind::F64 => text.push_str("f64"),
            TypeKind::UserDefined(paths) => {
                text.push_str(&format!("{}", paths.first().unwrap()));
                for path in &paths[1..] {
                    text.push_str(&format!("::{}", path));
                }
            }
        }
        if !self.width.is_empty() {
            text.push(' ');
        }
        for x in &self.width {
            let mut stringifier = Stringifier::new();
            stringifier.expression(x);
            text.push_str(&format!("[{}]", stringifier.as_str()));
        }
        text.fmt(f)
    }
}

impl From<&syntax_tree::Type> for Type {
    fn from(value: &syntax_tree::Type) -> Self {
        let modifier = if value.type_opt.is_some() {
            Some(TypeModifier::Tri)
        } else {
            None
        };
        let kind = match &*value.type_group {
            syntax_tree::TypeGroup::BuiltinType(x) => match &*x.builtin_type {
                syntax_tree::BuiltinType::Logic(_) => TypeKind::Logic,
                syntax_tree::BuiltinType::Bit(_) => TypeKind::Bit,
                syntax_tree::BuiltinType::U32(_) => TypeKind::U32,
                syntax_tree::BuiltinType::U64(_) => TypeKind::U64,
                syntax_tree::BuiltinType::I32(_) => TypeKind::I32,
                syntax_tree::BuiltinType::I64(_) => TypeKind::I64,
                syntax_tree::BuiltinType::F32(_) => TypeKind::F32,
                syntax_tree::BuiltinType::F64(_) => TypeKind::F64,
            },
            syntax_tree::TypeGroup::ScopedIdentifier(x) => {
                let x = &x.scoped_identifier;
                let mut name = Vec::new();
                name.push(x.identifier.identifier_token.token.text);
                for x in &x.scoped_identifier_list {
                    name.push(x.identifier.identifier_token.token.text);
                }
                TypeKind::UserDefined(name)
            }
        };
        let mut width = Vec::new();
        for x in &value.type_list {
            width.push(*x.width.expression.clone());
        }
        Type {
            kind,
            modifier,
            width,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariableProperty {
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct PortProperty {
    pub r#type: Option<Type>,
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

impl From<&syntax_tree::PortDeclarationItem> for Port {
    fn from(value: &syntax_tree::PortDeclarationItem) -> Self {
        let property = match &*value.port_declaration_item_group {
            syntax_tree::PortDeclarationItemGroup::DirectionType(x) => {
                let r#type: Type = x.r#type.as_ref().into();
                let direction: Direction = x.direction.as_ref().into();
                PortProperty {
                    r#type: Some(r#type),
                    direction,
                }
            }
            syntax_tree::PortDeclarationItemGroup::Interface(_) => PortProperty {
                r#type: None,
                direction: Direction::Interface,
            },
        };
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
    pub value: syntax_tree::Expression,
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

impl From<&syntax_tree::WithParameterItem> for Parameter {
    fn from(value: &syntax_tree::WithParameterItem) -> Self {
        let scope = match &*value.with_parameter_item_group {
            syntax_tree::WithParameterItemGroup::Parameter(_) => ParameterScope::Global,
            syntax_tree::WithParameterItemGroup::Localparam(_) => ParameterScope::Local,
        };
        let r#type: Type = value.r#type.as_ref().into();
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

#[derive(Debug, Clone)]
pub struct StructMemberProperty {
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct EnumProperty {
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct EnumMemberProperty {
    pub value: Option<syntax_tree::Expression>,
}

#[derive(Debug, Clone)]
pub struct ModportProperty {
    pub members: Vec<ModportMember>,
}

#[derive(Debug, Clone)]
pub struct ModportMember {
    pub name: StrId,
    pub direction: Direction,
}
