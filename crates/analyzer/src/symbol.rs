use crate::evaluator::{Evaluated, Evaluator};
use crate::namespace::Namespace;
use crate::symbol_path::{GenericSymbolPath, SymbolPath};
use crate::symbol_table;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenRange};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::Stringifier;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub usize);

thread_local!(static SYMBOL_ID: RefCell<usize> = const { RefCell::new(0) });

pub fn new_symbol_id() -> SymbolId {
    SYMBOL_ID.with(|f| {
        let mut ret = f.borrow_mut();
        *ret += 1;
        SymbolId(*ret)
    })
}

#[derive(Debug, Default, Clone)]
pub struct DocComment(pub Vec<StrId>);

impl DocComment {
    pub fn format(&self, single_line: bool) -> String {
        let mut ret = String::new();
        for t in &self.0 {
            let t = format!("{}", t);
            let t = t.trim_start_matches("///");
            ret.push_str(t);
            if single_line {
                break;
            }
        }
        ret
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct GenericMap {
    pub name: String,
    pub map: HashMap<StrId, GenericSymbolPath>,
}

impl GenericMap {
    pub fn generic(&self) -> bool {
        !self.map.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub token: Token,
    pub id: SymbolId,
    pub kind: SymbolKind,
    pub namespace: Namespace,
    pub references: Vec<Token>,
    pub generic_instances: Vec<SymbolId>,
    pub imported: Vec<Namespace>,
    pub evaluated: Cell<Option<Evaluated>>,
    pub allow_unused: bool,
    pub public: bool,
    pub doc_comment: DocComment,
    pub r#type: Option<Type>,
}

impl Symbol {
    pub fn new(
        token: &Token,
        kind: SymbolKind,
        namespace: &Namespace,
        public: bool,
        doc_comment: DocComment,
    ) -> Self {
        Self {
            token: *token,
            id: new_symbol_id(),
            kind,
            namespace: namespace.to_owned(),
            references: Vec::new(),
            generic_instances: Vec::new(),
            imported: Vec::new(),
            evaluated: Cell::new(None),
            allow_unused: false,
            public,
            doc_comment,
            r#type: None,
        }
    }

    pub fn evaluate(&self) -> Evaluated {
        if let Some(evaluated) = self.evaluated.get() {
            evaluated
        } else {
            let evaluated = match &self.kind {
                SymbolKind::Variable(x) => {
                    let mut evaluator = Evaluator::new();
                    if let Some(width) = evaluator.type_width(x.r#type.clone()) {
                        Evaluated::Variable { width }
                    } else {
                        Evaluated::Unknown
                    }
                }
                SymbolKind::Parameter(x) => {
                    let mut evaluator = Evaluator::new();
                    if let Some(width) = evaluator.type_width(x.r#type.clone()) {
                        evaluator.context_width.push(width);
                    }
                    match &x.value {
                        ParameterValue::Expression(x) => evaluator.expression(x),
                        ParameterValue::TypeExpression(_) => Evaluated::Unknown,
                    }
                }
                _ => Evaluated::Unknown,
            };
            self.evaluated.replace(Some(evaluated));
            evaluated
        }
    }

    pub fn inner_namespace(&self) -> Namespace {
        let mut ret = self.namespace.clone();
        ret.push(self.token.text);
        ret
    }

    pub fn generic_maps(&self) -> Vec<GenericMap> {
        let mut ret = Vec::new();

        let prefix = if matches!(
            self.kind,
            SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_)
        ) {
            format!("{}_", self.namespace)
        } else {
            "".to_string()
        };

        for i in &self.generic_instances {
            let symbol = symbol_table::get(*i).unwrap();
            let map = if let SymbolKind::GenericInstance(ref x) = symbol.kind {
                self.generic_table(&x.arguments)
            } else {
                HashMap::new()
            };
            let name = format!("{}{}", prefix, symbol.token.text);
            ret.push(GenericMap { name, map });
        }

        // empty map for non-generic
        if ret.is_empty() {
            ret.push(GenericMap::default());
        }
        ret
    }

    pub fn generic_table(
        &self,
        arguments: &[GenericSymbolPath],
    ) -> HashMap<StrId, GenericSymbolPath> {
        let generic_parameters = self.generic_parameters();
        let mut ret = HashMap::new();

        for (i, arg) in arguments.iter().enumerate() {
            if let Some((p, _)) = generic_parameters.get(i) {
                ret.insert(*p, arg.clone());
            }
        }

        for param in generic_parameters.iter().skip(arguments.len()) {
            ret.insert(param.0, param.1.as_ref().unwrap().clone());
        }

        ret
    }

    pub fn generic_parameters(&self) -> Vec<(StrId, Option<GenericSymbolPath>)> {
        fn get_generic_parameter(id: SymbolId) -> (StrId, Option<GenericSymbolPath>) {
            let symbol = symbol_table::get(id).unwrap();
            if let SymbolKind::GenericParameter(x) = symbol.kind {
                (symbol.token.text, x.default_value)
            } else {
                unreachable!()
            }
        }

        match &self.kind {
            SymbolKind::Function(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_parameter(*x))
                .collect(),
            SymbolKind::Module(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_parameter(*x))
                .collect(),
            SymbolKind::Interface(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_parameter(*x))
                .collect(),
            SymbolKind::Package(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_parameter(*x))
                .collect(),
            SymbolKind::Struct(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_parameter(*x))
                .collect(),
            SymbolKind::Union(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_parameter(*x))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn generic_references(&self) -> Vec<GenericSymbolPath> {
        match &self.kind {
            SymbolKind::Function(x) => x.generic_references.clone(),
            SymbolKind::Module(x) => x.generic_references.clone(),
            SymbolKind::Interface(x) => x.generic_references.clone(),
            SymbolKind::Package(x) => x.generic_references.clone(),
            SymbolKind::Struct(x) => x.generic_references.clone(),
            SymbolKind::Union(x) => x.generic_references.clone(),
            _ => Vec::new(),
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
    Package(PackageProperty),
    Struct(StructProperty),
    StructMember(StructMemberProperty),
    Union(UnionProperty),
    UnionMember(UnionMemberProperty),
    TypeDef(TypeDefProperty),
    Enum(EnumProperty),
    EnumMember(EnumMemberProperty),
    Modport(ModportProperty),
    Genvar,
    ModportVariableMember(ModportVariableMemberProperty),
    ModportFunctionMember(ModportFunctionMemberProperty),
    SystemVerilog,
    Namespace,
    SystemFunction,
    GenericParameter(GenericParameterProperty),
    GenericInstance(GenericInstanceProperty),
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
            SymbolKind::Package(_) => "package".to_string(),
            SymbolKind::Struct(_) => "struct".to_string(),
            SymbolKind::StructMember(_) => "struct member".to_string(),
            SymbolKind::Union(_) => "union".to_string(),
            SymbolKind::UnionMember(_) => "union member".to_string(),
            SymbolKind::TypeDef(_) => "typedef".to_string(),
            SymbolKind::Enum(_) => "enum".to_string(),
            SymbolKind::EnumMember(_) => "enum member".to_string(),
            SymbolKind::Modport(_) => "modport".to_string(),
            SymbolKind::Genvar => "genvar".to_string(),
            SymbolKind::ModportVariableMember(_) => "modport variable member".to_string(),
            SymbolKind::ModportFunctionMember(_) => "modport function member".to_string(),
            SymbolKind::SystemVerilog => "systemverilog item".to_string(),
            SymbolKind::Namespace => "namespace".to_string(),
            SymbolKind::SystemFunction => "system function".to_string(),
            SymbolKind::GenericParameter(_) => "generic parameter".to_string(),
            SymbolKind::GenericInstance(_) => "generic instance".to_string(),
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
                    "module ({} generic, {} params, {} ports)",
                    x.generic_parameters.len(),
                    x.parameters.len(),
                    x.ports.len()
                )
            }
            SymbolKind::Interface(x) => {
                format!(
                    "interface ({} generic, {} params)",
                    x.generic_parameters.len(),
                    x.parameters.len()
                )
            }
            SymbolKind::Function(x) => {
                format!(
                    "function ({} generic, {} args)",
                    x.generic_parameters.len(),
                    x.ports.len()
                )
            }
            SymbolKind::Parameter(x) => {
                let mut stringifier = Stringifier::new();
                match &x.value {
                    ParameterValue::Expression(x) => stringifier.expression(x),
                    ParameterValue::TypeExpression(x) => stringifier.type_expression(x),
                }
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
                let mut type_name = String::new();
                for (i, x) in x.type_name.iter().enumerate() {
                    if i != 0 {
                        type_name.push_str("::");
                    }
                    type_name.push_str(&format!("{x}"));
                }
                format!("instance ({type_name})")
            }
            SymbolKind::Block => "block".to_string(),
            SymbolKind::Package(x) => {
                format!("package ({} generic)", x.generic_parameters.len())
            }
            SymbolKind::Struct(_) => "struct".to_string(),
            SymbolKind::StructMember(x) => {
                format!("struct member ({})", x.r#type)
            }
            SymbolKind::Union(_) => "union".to_string(),
            SymbolKind::UnionMember(x) => {
                format!("union member ({})", x.r#type)
            }
            SymbolKind::TypeDef(x) => {
                format!("typedef alias ({})", x.r#type)
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
            SymbolKind::Genvar => "genvar".to_string(),
            SymbolKind::ModportVariableMember(x) => {
                format!("modport variable member ({})", x.direction)
            }
            SymbolKind::ModportFunctionMember(_) => "modport function member".to_string(),
            SymbolKind::SystemVerilog => "systemverilog item".to_string(),
            SymbolKind::Namespace => "namespace".to_string(),
            SymbolKind::SystemFunction => "system function".to_string(),
            SymbolKind::GenericParameter(_) => "generic parameter".to_string(),
            SymbolKind::GenericInstance(_) => "generic instance".to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
    Inout,
    Ref,
    Interface,
    Modport,
    Import,
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
            Direction::Import => "import".to_string(),
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
            syntax_tree::Direction::Import(_) => Direction::Import,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Type {
    pub modifier: Vec<TypeModifier>,
    pub kind: TypeKind,
    pub width: Vec<syntax_tree::Expression>,
    pub array: Vec<syntax_tree::Expression>,
    pub is_const: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Clock,
    ClockPosedge,
    ClockNegedge,
    Reset,
    ResetAsyncHigh,
    ResetAsyncLow,
    ResetSyncHigh,
    ResetSyncLow,
    Bit,
    Logic,
    U32,
    U64,
    I32,
    I64,
    F32,
    F64,
    Type,
    String,
    UserDefined(Vec<StrId>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeModifier {
    Tri,
    Signed,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        for x in &self.modifier {
            match x {
                TypeModifier::Tri => text.push_str("tri "),
                TypeModifier::Signed => text.push_str("signed "),
            }
        }
        match &self.kind {
            TypeKind::Clock => text.push_str("clock"),
            TypeKind::ClockPosedge => text.push_str("clock posedge"),
            TypeKind::ClockNegedge => text.push_str("clock negedge"),
            TypeKind::Reset => text.push_str("reset"),
            TypeKind::ResetAsyncHigh => text.push_str("reset async high"),
            TypeKind::ResetAsyncLow => text.push_str("reset async low"),
            TypeKind::ResetSyncHigh => text.push_str("reset sync high"),
            TypeKind::ResetSyncLow => text.push_str("reset sync low"),
            TypeKind::Bit => text.push_str("bit"),
            TypeKind::Logic => text.push_str("logic"),
            TypeKind::U32 => text.push_str("u32"),
            TypeKind::U64 => text.push_str("u64"),
            TypeKind::I32 => text.push_str("i32"),
            TypeKind::I64 => text.push_str("i64"),
            TypeKind::F32 => text.push_str("f32"),
            TypeKind::F64 => text.push_str("f64"),
            TypeKind::Type => text.push_str("type"),
            TypeKind::String => text.push_str("string"),
            TypeKind::UserDefined(paths) => {
                text.push_str(&format!("{}", paths.first().unwrap()));
                for path in &paths[1..] {
                    text.push_str(&format!("::{path}"));
                }
            }
        }
        if !self.width.is_empty() {
            text.push('<');
            for (i, x) in self.width.iter().enumerate() {
                if i != 0 {
                    text.push_str(", ");
                }
                let mut stringifier = Stringifier::new();
                stringifier.expression(x);
                text.push_str(stringifier.as_str());
            }
            text.push('>');
        }
        if !self.array.is_empty() {
            text.push_str(" [");
            for (i, x) in self.array.iter().enumerate() {
                if i != 0 {
                    text.push_str(", ");
                }
                let mut stringifier = Stringifier::new();
                stringifier.expression(x);
                text.push_str(stringifier.as_str());
            }
            text.push(']');
        }
        text.fmt(f)
    }
}

impl From<&syntax_tree::ScalarType> for Type {
    fn from(value: &syntax_tree::ScalarType) -> Self {
        let mut modifier = Vec::new();
        for x in &value.scalar_type_list {
            match &*x.type_modifier {
                syntax_tree::TypeModifier::Tri(_) => modifier.push(TypeModifier::Tri),
                syntax_tree::TypeModifier::Signed(_) => modifier.push(TypeModifier::Signed),
            }
        }
        match &*value.scalar_type_group {
            syntax_tree::ScalarTypeGroup::VariableType(x) => {
                let x = &x.variable_type;
                let kind = match &*x.variable_type_group {
                    syntax_tree::VariableTypeGroup::Clock(_) => TypeKind::Clock,
                    syntax_tree::VariableTypeGroup::ClockPosedge(_) => TypeKind::ClockPosedge,
                    syntax_tree::VariableTypeGroup::ClockNegedge(_) => TypeKind::ClockNegedge,
                    syntax_tree::VariableTypeGroup::Reset(_) => TypeKind::Reset,
                    syntax_tree::VariableTypeGroup::ResetAsyncHigh(_) => TypeKind::ResetAsyncHigh,
                    syntax_tree::VariableTypeGroup::ResetAsyncLow(_) => TypeKind::ResetAsyncLow,
                    syntax_tree::VariableTypeGroup::ResetSyncHigh(_) => TypeKind::ResetSyncHigh,
                    syntax_tree::VariableTypeGroup::ResetSyncLow(_) => TypeKind::ResetSyncLow,
                    syntax_tree::VariableTypeGroup::Logic(_) => TypeKind::Logic,
                    syntax_tree::VariableTypeGroup::Bit(_) => TypeKind::Bit,
                    syntax_tree::VariableTypeGroup::ScopedIdentifier(x) => {
                        let x = &x.scoped_identifier;
                        let mut name = Vec::new();
                        match &*x.scoped_identifier_group {
                            syntax_tree::ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(
                                x,
                            ) => {
                                name.push(x.identifier.identifier_token.token.text);
                            }
                            syntax_tree::ScopedIdentifierGroup::DollarIdentifier(x) => {
                                name.push(x.dollar_identifier.dollar_identifier_token.token.text);
                            }
                        }
                        for x in &x.scoped_identifier_list {
                            name.push(x.identifier.identifier_token.token.text);
                        }
                        TypeKind::UserDefined(name)
                    }
                };
                let mut width = Vec::new();
                if let Some(ref x) = x.variable_type_opt {
                    let x = &x.width;
                    width.push(*x.expression.clone());
                    for x in &x.width_list {
                        width.push(*x.expression.clone());
                    }
                }
                Type {
                    kind,
                    modifier,
                    width,
                    array: vec![],
                    is_const: false,
                }
            }
            syntax_tree::ScalarTypeGroup::FixedType(x) => {
                let x = &x.fixed_type;
                let kind = match **x {
                    syntax_tree::FixedType::U32(_) => TypeKind::U32,
                    syntax_tree::FixedType::U64(_) => TypeKind::U64,
                    syntax_tree::FixedType::I32(_) => TypeKind::I32,
                    syntax_tree::FixedType::I64(_) => TypeKind::I64,
                    syntax_tree::FixedType::F32(_) => TypeKind::F32,
                    syntax_tree::FixedType::F64(_) => TypeKind::F64,
                    syntax_tree::FixedType::Strin(_) => TypeKind::String,
                };
                Type {
                    kind,
                    modifier,
                    width: vec![],
                    array: vec![],
                    is_const: false,
                }
            }
        }
    }
}

impl From<&syntax_tree::ArrayType> for Type {
    fn from(value: &syntax_tree::ArrayType) -> Self {
        let scalar_type: Type = value.scalar_type.as_ref().into();
        let mut array = Vec::new();
        if let Some(ref x) = value.array_type_opt {
            let x = &x.array;
            array.push(*x.expression.clone());
            for x in &x.array_list {
                array.push(*x.expression.clone());
            }
        }
        Type {
            kind: scalar_type.kind,
            modifier: scalar_type.modifier,
            width: scalar_type.width,
            array,
            is_const: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariableProperty {
    pub r#type: Type,
    pub affiniation: VariableAffiniation,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableAffiniation {
    Module,
    Intarface,
    Package,
    Function,
}

#[derive(Debug, Clone)]
pub struct PortProperty {
    pub token: Token,
    pub r#type: Option<Type>,
    pub direction: Direction,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
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
        let token = value.identifier.identifier_token.token;
        let property = match &*value.port_declaration_item_group {
            syntax_tree::PortDeclarationItemGroup::DirectionArrayType(x) => {
                let r#type: Type = x.array_type.as_ref().into();
                let direction: Direction = x.direction.as_ref().into();
                PortProperty {
                    token,
                    r#type: Some(r#type),
                    direction,
                    prefix: None,
                    suffix: None,
                }
            }
            syntax_tree::PortDeclarationItemGroup::InterfacePortDeclarationItemOpt(_) => {
                PortProperty {
                    token,
                    r#type: None,
                    direction: Direction::Interface,
                    prefix: None,
                    suffix: None,
                }
            }
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
    pub token: Token,
    pub r#type: Type,
    pub scope: ParameterScope,
    pub value: ParameterValue,
}

#[derive(Debug, Clone)]
pub enum ParameterValue {
    Expression(syntax_tree::Expression),
    TypeExpression(syntax_tree::TypeExpression),
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
        let token = value.identifier.identifier_token.token;
        let scope = match &*value.with_parameter_item_group {
            syntax_tree::WithParameterItemGroup::Param(_) => ParameterScope::Global,
            syntax_tree::WithParameterItemGroup::Local(_) => ParameterScope::Local,
        };
        match &*value.with_parameter_item_group0 {
            syntax_tree::WithParameterItemGroup0::ArrayTypeEquExpression(x) => {
                let r#type: Type = x.array_type.as_ref().into();
                let property = ParameterProperty {
                    token,
                    r#type,
                    scope,
                    value: ParameterValue::Expression(*x.expression.clone()),
                };
                Parameter {
                    name: value.identifier.identifier_token.token.text,
                    property,
                }
            }
            syntax_tree::WithParameterItemGroup0::TypeEquTypeExpression(x) => {
                let r#type: Type = Type {
                    modifier: vec![],
                    kind: TypeKind::Type,
                    width: vec![],
                    array: vec![],
                    is_const: false,
                };
                let property = ParameterProperty {
                    token,
                    r#type,
                    scope,
                    value: ParameterValue::TypeExpression(*x.type_expression.clone()),
                };
                Parameter {
                    name: value.identifier.identifier_token.token.text,
                    property,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleProperty {
    pub range: TokenRange,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
    pub parameters: Vec<Parameter>,
    pub ports: Vec<Port>,
    pub default_clock: Option<SymbolId>,
    pub default_reset: Option<SymbolId>,
}

#[derive(Debug, Clone)]
pub struct InterfaceProperty {
    pub range: TokenRange,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Clone)]
pub struct FunctionProperty {
    pub range: TokenRange,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
    pub ports: Vec<Port>,
    pub ret: Option<Type>,
}

#[derive(Debug, Clone)]
pub struct ConnectTarget {
    pub path: Vec<(StrId, Vec<syntax_tree::Expression>)>,
}

impl ConnectTarget {
    pub fn path(&self) -> Vec<StrId> {
        self.path.iter().map(|x| x.0).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    pub fn is_partial(&self) -> bool {
        self.path.iter().any(|x| !x.1.is_empty())
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for ConnectTarget {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let path: SymbolPath = value.scoped_identifier.as_ref().into();

        let mut ret = vec![];
        for (i, x) in path.as_slice().iter().enumerate() {
            if i == path.as_slice().len() - 1 {
                let select: Vec<_> = value
                    .expression_identifier_list
                    .iter()
                    .map(|x| x.select.expression.as_ref().clone())
                    .collect();
                ret.push((*x, select));
            } else {
                ret.push((*x, vec![]));
            }
        }

        for x in &value.expression_identifier_list0 {
            let text = x.identifier.identifier_token.token.text;
            let select: Vec<_> = x
                .expression_identifier_list0_list
                .iter()
                .map(|x| x.select.expression.as_ref().clone())
                .collect();
            ret.push((text, select));
        }
        Self { path: ret }
    }
}

#[derive(Debug, Clone)]
pub struct InstanceProperty {
    pub type_name: Vec<StrId>,
    pub connects: HashMap<Token, Vec<ConnectTarget>>,
}

#[derive(Debug, Clone)]
pub struct PackageProperty {
    pub range: TokenRange,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub struct StructProperty {
    pub members: Vec<SymbolId>,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub struct StructMemberProperty {
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct UnionProperty {
    pub members: Vec<SymbolId>,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub struct UnionMemberProperty {
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct TypeDefProperty {
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct EnumProperty {
    pub r#type: Type,
    pub members: Vec<SymbolId>,
}

#[derive(Debug, Clone)]
pub struct EnumMemberProperty {
    pub value: Option<syntax_tree::Expression>,
    pub prefix: String,
}

#[derive(Debug, Clone)]
pub struct ModportProperty {
    pub members: Vec<SymbolId>,
}

#[derive(Debug, Clone)]
pub struct ModportVariableMemberProperty {
    pub direction: Direction,
}

#[derive(Debug, Clone)]
pub struct ModportFunctionMemberProperty {
    pub function: SymbolId,
}

#[derive(Debug, Clone)]
pub struct GenericParameterProperty {
    pub default_value: Option<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub struct GenericInstanceProperty {
    pub base: SymbolId,
    pub arguments: Vec<GenericSymbolPath>,
}
