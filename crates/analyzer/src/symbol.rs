use crate::HashMap;
use crate::attribute::EnumEncodingItem;
use crate::definition_table::DefinitionId;
use crate::evaluator::{
    Evaluated, EvaluatedError, EvaluatedTypeClockKind, EvaluatedTypeResetKind, Evaluator,
};
use crate::namespace::Namespace;
use crate::symbol_path::{GenericSymbolPath, SymbolPath};
use crate::symbol_table;
use std::cell::RefCell;
use std::fmt;
use veryl_parser::Stringifier;
use veryl_parser::resource_table::{PathId, StrId};
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenRange, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;

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
    pub evaluated: RefCell<Option<Evaluated>>,
    pub overrides: Vec<Evaluated>,
    pub allow_unused: bool,
    pub public: bool,
    pub doc_comment: DocComment,
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
            evaluated: RefCell::new(None),
            overrides: Vec::new(),
            allow_unused: false,
            public,
            doc_comment,
        }
    }

    pub fn get_parent(&self) -> Option<Symbol> {
        let mut namespace = self.namespace.clone();
        if let Some(path) = namespace.pop() {
            if namespace.depth() >= 1 {
                let path = SymbolPath::new(&[path]);
                if let Ok(symbol) = symbol_table::resolve((&path, &namespace)) {
                    return Some(symbol.found);
                }
            }
        }

        None
    }

    pub fn evaluate(&self) -> Evaluated {
        if let Some(x) = self.overrides.last() {
            x.clone()
        } else if self.evaluated.borrow().is_some() {
            self.evaluated.borrow().clone().unwrap()
        } else {
            let evaluated = match &self.kind {
                SymbolKind::Variable(x) => {
                    let mut evaluator = Evaluator::new();
                    let width = evaluator.type_width(x.r#type.clone());
                    let array = evaluator.type_array(x.r#type.clone());

                    if let (Some(width), Some(array)) = (width, array) {
                        if x.r#type.kind.is_clock() {
                            let kind = match x.r#type.kind {
                                TypeKind::Clock => EvaluatedTypeClockKind::Implicit,
                                TypeKind::ClockPosedge => EvaluatedTypeClockKind::Posedge,
                                TypeKind::ClockNegedge => EvaluatedTypeClockKind::Negedge,
                                _ => unreachable!(),
                            };
                            Evaluated::create_clock(kind, width, array)
                        } else if x.r#type.kind.is_reset() {
                            let kind = match x.r#type.kind {
                                TypeKind::Reset => EvaluatedTypeResetKind::Implicit,
                                TypeKind::ResetAsyncHigh => EvaluatedTypeResetKind::AsyncHigh,
                                TypeKind::ResetAsyncLow => EvaluatedTypeResetKind::AsyncLow,
                                TypeKind::ResetSyncHigh => EvaluatedTypeResetKind::SyncHigh,
                                TypeKind::ResetSyncLow => EvaluatedTypeResetKind::SyncLow,
                                _ => unreachable!(),
                            };
                            Evaluated::create_reset(kind, width, array)
                        } else if x.loop_variable {
                            Evaluated::create_unknown_static()
                        } else {
                            let signed = x.r#type.modifier.contains(&TypeModifier::Signed);
                            let is_4state = x.r#type.kind.is_4state();
                            Evaluated::create_variable(signed, is_4state, width, array)
                        }
                    } else {
                        Evaluated::create_unknown()
                    }
                }
                SymbolKind::Port(x) => {
                    let mut evaluator = Evaluator::new();
                    let width = evaluator.type_width(x.r#type.clone());
                    let array = evaluator.type_array(x.r#type.clone());

                    if let (Some(width), Some(array)) = (width, array) {
                        if x.r#type.kind.is_clock() {
                            let kind = match x.r#type.kind {
                                TypeKind::Clock => EvaluatedTypeClockKind::Implicit,
                                TypeKind::ClockPosedge => EvaluatedTypeClockKind::Posedge,
                                TypeKind::ClockNegedge => EvaluatedTypeClockKind::Negedge,
                                _ => unreachable!(),
                            };
                            Evaluated::create_clock(kind, width, array)
                        } else if x.r#type.kind.is_reset() {
                            let kind = match x.r#type.kind {
                                TypeKind::Reset => EvaluatedTypeResetKind::Implicit,
                                TypeKind::ResetAsyncHigh => EvaluatedTypeResetKind::AsyncHigh,
                                TypeKind::ResetAsyncLow => EvaluatedTypeResetKind::AsyncLow,
                                TypeKind::ResetSyncHigh => EvaluatedTypeResetKind::SyncHigh,
                                TypeKind::ResetSyncLow => EvaluatedTypeResetKind::SyncLow,
                                _ => unreachable!(),
                            };
                            Evaluated::create_reset(kind, width, array)
                        } else {
                            let signed = x.r#type.modifier.contains(&TypeModifier::Signed);
                            let is_4state = x.r#type.kind.is_4state();
                            Evaluated::create_variable(signed, is_4state, width, array)
                        }
                    } else {
                        Evaluated::create_unknown()
                    }
                }
                SymbolKind::Parameter(x) => {
                    let mut evaluator = Evaluator::new();
                    if let Some(width) = evaluator.type_width(x.r#type.clone()) {
                        evaluator.context_width = width;
                    }
                    evaluator.expression(&x.value)
                }
                SymbolKind::EnumMember(_) => {
                    // TODO: Actually Evaluate its Width
                    Evaluated::create_unknown_static()
                }
                SymbolKind::Genvar => Evaluated::create_unknown_static(),
                SymbolKind::Module(_)
                | SymbolKind::ProtoModule(_)
                | SymbolKind::Interface(_)
                | SymbolKind::Function(_)
                | SymbolKind::Block
                | SymbolKind::Package(_)
                | SymbolKind::Modport(_)
                | SymbolKind::ModportFunctionMember(_)
                | SymbolKind::Namespace
                | SymbolKind::SystemFunction
                | SymbolKind::GenericInstance(_)
                | SymbolKind::ClockDomain
                | SymbolKind::Test(_) => {
                    let mut ret = Evaluated::create_unknown();
                    ret.errors.push(EvaluatedError::InvalidFactor {
                        kind: self.kind.to_kind_name(),
                        token: self.token,
                    });
                    ret
                }
                SymbolKind::Instance(x) => {
                    let mut evaluator = Evaluator::new();
                    if let Ok(symbol) =
                        symbol_table::resolve((&x.type_name.mangled_path(), &self.namespace))
                    {
                        if let SymbolKind::Interface(_) = symbol.found.kind {
                            if let Some(array) = evaluator.expression_list(&x.array) {
                                Evaluated::create_user_defined(symbol.found.id, vec![], array)
                            } else {
                                Evaluated::create_unknown()
                            }
                        } else {
                            let mut ret = Evaluated::create_unknown();
                            ret.errors.push(EvaluatedError::InvalidFactor {
                                kind: self.kind.to_kind_name(),
                                token: self.token,
                            });
                            ret
                        }
                    } else {
                        Evaluated::create_unknown()
                    }
                }
                SymbolKind::SystemVerilog => Evaluated::create_unknown_static(),
                _ => Evaluated::create_unknown(),
            };
            self.evaluated.replace(Some(evaluated.clone()));
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

        let generic_instances = if matches!(self.kind, SymbolKind::GenericInstance(_)) {
            &vec![self.id]
        } else {
            &self.generic_instances
        };
        for i in generic_instances {
            let symbol = symbol_table::get(*i).unwrap();
            let map = if let SymbolKind::GenericInstance(ref x) = symbol.kind {
                self.generic_table(&x.arguments)
            } else {
                HashMap::default()
            };
            let name = format!("{}{}", prefix, symbol.token.text);
            ret.push(GenericMap { name, map });
        }

        // empty map for non-generic
        if ret.is_empty() && !self.kind.is_generic() {
            ret.push(GenericMap::default());
        }
        ret
    }

    pub fn generic_table(
        &self,
        arguments: &[GenericSymbolPath],
    ) -> HashMap<StrId, GenericSymbolPath> {
        let generic_parameters = self.generic_parameters();
        let mut ret = HashMap::default();

        for (i, arg) in arguments.iter().enumerate() {
            if let Some((p, _)) = generic_parameters.get(i) {
                ret.insert(*p, arg.clone());
            }
        }

        for param in generic_parameters.iter().skip(arguments.len()) {
            ret.insert(param.0, param.1.default_value.as_ref().unwrap().clone());
        }

        ret
    }

    pub fn generic_parameters(&self) -> Vec<(StrId, GenericParameterProperty)> {
        fn get_generic_parameter(id: SymbolId) -> (StrId, GenericParameterProperty) {
            let symbol = symbol_table::get(id).unwrap();
            if let SymbolKind::GenericParameter(x) = symbol.kind {
                (symbol.token.text, x)
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
            SymbolKind::GenericInstance(x) => {
                let symbol = symbol_table::get(x.base).unwrap();
                symbol.generic_parameters()
            }
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

    pub fn proto(&self) -> Option<SymbolPath> {
        match &self.kind {
            SymbolKind::Module(x) => x.proto.clone(),
            SymbolKind::Package(x) => x.proto.clone(),
            SymbolKind::AliasPackage(x) => {
                let symbol =
                    symbol_table::resolve((&x.target.generic_path(), &self.namespace)).ok()?;
                symbol.found.proto()
            }
            SymbolKind::GenericParameter(x) => match x.bound {
                GenericBoundKind::Proto(ref x) => Some(x.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn alias_target(&self) -> Option<GenericSymbolPath> {
        match &self.kind {
            SymbolKind::AliasModule(x) => Some(x.target.clone()),
            SymbolKind::AliasInterface(x) => Some(x.target.clone()),
            SymbolKind::AliasPackage(x) => Some(x.target.clone()),
            _ => None,
        }
    }

    pub fn is_module(&self, include_proto: bool) -> bool {
        match &self.kind {
            SymbolKind::Module(_) | SymbolKind::AliasModule(_) => return true,
            SymbolKind::ProtoModule(_) => return include_proto,
            SymbolKind::GenericInstance(x) => {
                let symbol = symbol_table::get(x.base).unwrap();
                return symbol.is_module(false);
            }
            SymbolKind::GenericParameter(x) => {
                if let GenericBoundKind::Proto(proto) = &x.bound {
                    if let Ok(symbol) = symbol_table::resolve((proto, &self.namespace)) {
                        return symbol.found.is_module(true);
                    }
                }
            }
            _ => {}
        }
        false
    }

    pub fn is_interface(&self, _include_proto: bool) -> bool {
        match &self.kind {
            SymbolKind::Interface(_) | SymbolKind::AliasInterface(_) => return true,
            SymbolKind::GenericInstance(x) => {
                let symbol = symbol_table::get(x.base).unwrap();
                return symbol.is_interface(false);
            }
            SymbolKind::GenericParameter(x) => {
                if let GenericBoundKind::Proto(proto) = &x.bound {
                    if let Ok(symbol) = symbol_table::resolve((proto, &self.namespace)) {
                        return symbol.found.is_interface(true);
                    }
                }
            }
            _ => {}
        }
        false
    }

    pub fn is_package(&self, include_proto: bool) -> bool {
        match &self.kind {
            SymbolKind::Package(_) | SymbolKind::AliasPackage(_) => return true,
            SymbolKind::ProtoPackage(_) => return include_proto,
            SymbolKind::GenericInstance(x) => {
                let symbol = symbol_table::get(x.base).unwrap();
                return symbol.is_package(false);
            }
            SymbolKind::GenericParameter(x) => {
                if let GenericBoundKind::Proto(proto) = &x.bound {
                    if let Ok(symbol) = symbol_table::resolve((proto, &self.namespace)) {
                        return symbol.found.is_package(true);
                    }
                }
            }
            _ => {}
        }
        false
    }

    pub fn is_importable(&self, include_proto: bool) -> bool {
        match &self.kind {
            SymbolKind::ProtoConst(_) | SymbolKind::ProtoTypeDef | SymbolKind::ProtoFunction(_) => {
                return include_proto;
            }
            SymbolKind::Parameter(_)
            | SymbolKind::TypeDef(_)
            | SymbolKind::Enum(_)
            | SymbolKind::Struct(_)
            | SymbolKind::Union(_)
            | SymbolKind::Function(_) => {
                if let Some(parent) = self.get_parent() {
                    return parent.is_package(include_proto);
                }
            }
            SymbolKind::EnumMember(_) | SymbolKind::EnumMemberMangled => {
                if let Some(parent) = self.get_parent() {
                    return parent.is_importable(include_proto);
                }
            }
            _ => {}
        }
        false
    }
}

#[derive(Debug, Clone)]
pub enum SymbolKind {
    Port(PortProperty),
    Variable(VariableProperty),
    Module(ModuleProperty),
    ProtoModule(ProtoModuleProperty),
    AliasModule(AliasModuleProperty),
    Interface(InterfaceProperty),
    AliasInterface(AliasInterfaceProperty),
    Function(FunctionProperty),
    ProtoFunction(FunctionProperty),
    Parameter(ParameterProperty),
    ProtoConst(ProtoConstProperty),
    Instance(InstanceProperty),
    Block,
    Package(PackageProperty),
    ProtoPackage(ProtoPackageProperty),
    AliasPackage(AliasPackageProperty),
    Struct(StructProperty),
    StructMember(StructMemberProperty),
    Union(UnionProperty),
    UnionMember(UnionMemberProperty),
    TypeDef(TypeDefProperty),
    ProtoTypeDef,
    Enum(EnumProperty),
    EnumMember(EnumMemberProperty),
    EnumMemberMangled,
    Modport(ModportProperty),
    Genvar,
    ModportVariableMember(ModportVariableMemberProperty),
    ModportFunctionMember(ModportFunctionMemberProperty),
    SystemVerilog,
    Namespace,
    SystemFunction,
    GenericParameter(GenericParameterProperty),
    GenericInstance(GenericInstanceProperty),
    ClockDomain,
    Test(TestProperty),
}

impl SymbolKind {
    pub fn to_kind_name(&self) -> String {
        match self {
            SymbolKind::Port(x) => match x.direction {
                Direction::Modport => "modport".to_string(),
                Direction::Import => "function import".to_string(),
                _ => format!("{} port", x.direction),
            },
            SymbolKind::Variable(_) => "variable".to_string(),
            SymbolKind::Module(_) => "module".to_string(),
            SymbolKind::ProtoModule(_) => "proto module".to_string(),
            SymbolKind::AliasModule(_) => "alias module".to_string(),
            SymbolKind::Interface(_) => "interface".to_string(),
            SymbolKind::AliasInterface(_) => "alias interface".to_string(),
            SymbolKind::Function(_) => "function".to_string(),
            SymbolKind::ProtoFunction(_) => "proto function".to_string(),
            SymbolKind::Parameter(_) => "parameter".to_string(),
            SymbolKind::ProtoConst(_) => "proto const".to_string(),
            SymbolKind::Instance(_) => "instance".to_string(),
            SymbolKind::Block => "block".to_string(),
            SymbolKind::Package(_) => "package".to_string(),
            SymbolKind::ProtoPackage(_) => "proto package".to_string(),
            SymbolKind::AliasPackage(_) => "alias package".to_string(),
            SymbolKind::Struct(_) => "struct".to_string(),
            SymbolKind::StructMember(_) => "struct member".to_string(),
            SymbolKind::Union(_) => "union".to_string(),
            SymbolKind::UnionMember(_) => "union member".to_string(),
            SymbolKind::TypeDef(_) => "typedef".to_string(),
            SymbolKind::ProtoTypeDef => "proto typedef".to_string(),
            SymbolKind::Enum(_) => "enum".to_string(),
            SymbolKind::EnumMember(_) => "enum member".to_string(),
            SymbolKind::EnumMemberMangled => "enum member mangled".to_string(),
            SymbolKind::Modport(_) => "modport".to_string(),
            SymbolKind::Genvar => "genvar".to_string(),
            SymbolKind::ModportVariableMember(x) => match x.direction {
                Direction::Input | Direction::Output | Direction::Inout => {
                    format!("modport {} variable member", x.direction)
                }
                _ => unreachable!(),
            },
            SymbolKind::ModportFunctionMember(_) => "modport function member".to_string(),
            SymbolKind::SystemVerilog => "systemverilog item".to_string(),
            SymbolKind::Namespace => "namespace".to_string(),
            SymbolKind::SystemFunction => "system function".to_string(),
            SymbolKind::GenericParameter(_) => "generic parameter".to_string(),
            SymbolKind::GenericInstance(_) => "generic instance".to_string(),
            SymbolKind::ClockDomain => "clock domain".to_string(),
            SymbolKind::Test(_) => "test".to_string(),
        }
    }

    pub fn is_generic(&self) -> bool {
        match self {
            SymbolKind::Module(x) => !x.generic_parameters.is_empty(),
            SymbolKind::Interface(x) => !x.generic_parameters.is_empty(),
            SymbolKind::Function(x) => !x.generic_parameters.is_empty(),
            SymbolKind::Package(x) => !x.generic_parameters.is_empty(),
            SymbolKind::Struct(x) => !x.generic_parameters.is_empty(),
            SymbolKind::Union(x) => !x.generic_parameters.is_empty(),
            _ => false,
        }
    }

    pub fn is_clock(&self) -> bool {
        match self {
            SymbolKind::Port(x) => x.r#type.kind.is_clock(),
            SymbolKind::Variable(x) => x.r#type.kind.is_clock(),
            _ => false,
        }
    }

    pub fn is_reset(&self) -> bool {
        match self {
            SymbolKind::Port(x) => x.r#type.kind.is_reset(),
            SymbolKind::Variable(x) => x.r#type.kind.is_reset(),
            _ => false,
        }
    }

    pub fn is_function(&self) -> bool {
        match self {
            SymbolKind::Function(_)
            | SymbolKind::ProtoFunction(_)
            | SymbolKind::SystemVerilog
            | SymbolKind::ModportFunctionMember(..)
            | SymbolKind::SystemFunction => true,
            SymbolKind::GenericInstance(x) => {
                let base = symbol_table::get(x.base).unwrap();
                matches!(
                    base.kind,
                    SymbolKind::Function(_)
                        | SymbolKind::SystemVerilog
                        | SymbolKind::ModportFunctionMember(..)
                        | SymbolKind::SystemFunction
                )
            }
            _ => false,
        }
    }

    pub fn get_type(&self) -> Option<&Type> {
        match self {
            SymbolKind::Port(x) => Some(&x.r#type),
            SymbolKind::Variable(x) => Some(&x.r#type),
            SymbolKind::Function(x) => x.ret.as_ref(),
            SymbolKind::ProtoFunction(x) => x.ret.as_ref(),
            SymbolKind::Parameter(x) => Some(&x.r#type),
            SymbolKind::ProtoConst(x) => Some(&x.r#type),
            SymbolKind::StructMember(x) => Some(&x.r#type),
            SymbolKind::UnionMember(x) => Some(&x.r#type),
            SymbolKind::TypeDef(x) => Some(&x.r#type),
            _ => None,
        }
    }

    pub fn get_type_mut(&mut self) -> Option<&mut Type> {
        match self {
            SymbolKind::Port(x) => Some(&mut x.r#type),
            SymbolKind::Variable(x) => Some(&mut x.r#type),
            SymbolKind::Function(x) => x.ret.as_mut(),
            SymbolKind::ProtoFunction(x) => x.ret.as_mut(),
            SymbolKind::Parameter(x) => Some(&mut x.r#type),
            SymbolKind::ProtoConst(x) => Some(&mut x.r#type),
            SymbolKind::StructMember(x) => Some(&mut x.r#type),
            SymbolKind::UnionMember(x) => Some(&mut x.r#type),
            SymbolKind::TypeDef(x) => Some(&mut x.r#type),
            _ => None,
        }
    }

    pub fn get_parameters(&self) -> &[Parameter] {
        match self {
            SymbolKind::Module(x) => &x.parameters,
            SymbolKind::Interface(x) => &x.parameters,
            _ => &[],
        }
    }

    pub fn get_definition(&self) -> Option<DefinitionId> {
        match self {
            SymbolKind::Module(x) => Some(x.definition),
            SymbolKind::Interface(x) => Some(x.definition),
            _ => None,
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            SymbolKind::Port(x) => {
                format!("port ({} {})", x.direction, x.r#type)
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
            SymbolKind::ProtoModule(x) => {
                format!(
                    "proto module ({} params, {} ports)",
                    x.parameters.len(),
                    x.ports.len()
                )
            }
            SymbolKind::AliasModule(x) => {
                format!("alias module (target {})", x.target)
            }
            SymbolKind::Interface(x) => {
                format!(
                    "interface ({} generic, {} params)",
                    x.generic_parameters.len(),
                    x.parameters.len()
                )
            }
            SymbolKind::AliasInterface(x) => {
                format!("alias interface (target {})", x.target)
            }
            SymbolKind::Function(x) => {
                format!(
                    "function ({} generic, {} args)",
                    x.generic_parameters.len(),
                    x.ports.len()
                )
            }
            SymbolKind::ProtoFunction(x) => {
                format!(
                    "proto function ({} generic, {} args)",
                    x.generic_parameters.len(),
                    x.ports.len()
                )
            }
            SymbolKind::Parameter(x) => {
                let mut stringifier = Stringifier::new();
                stringifier.expression(&x.value);
                match x.kind {
                    ParameterKind::Param => {
                        format!("parameter ({}) = {}", x.r#type, stringifier.as_str())
                    }
                    ParameterKind::Const => {
                        format!("localparam ({}) = {}", x.r#type, stringifier.as_str())
                    }
                }
            }
            SymbolKind::ProtoConst(x) => {
                format!("proto localparam ({})", x.r#type)
            }
            SymbolKind::Instance(x) => {
                let type_name = x.type_name.to_string();
                format!("instance ({type_name})")
            }
            SymbolKind::Block => "block".to_string(),
            SymbolKind::Package(x) => {
                format!("package ({} generic)", x.generic_parameters.len())
            }
            SymbolKind::ProtoPackage(_) => "proto package".to_string(),
            SymbolKind::AliasPackage(x) => {
                format!("alias package (target {})", x.target)
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
            SymbolKind::ProtoTypeDef => "proto typedef".to_string(),
            SymbolKind::Enum(x) => {
                if let Some(ref r#type) = x.r#type {
                    format!("enum ({})", r#type)
                } else {
                    "enum ()".to_string()
                }
            }
            SymbolKind::EnumMember(x) => {
                if let EnumMemberValue::ExplicitValue(ref expression, ref _evaluated) = x.value {
                    let mut stringifier = Stringifier::new();
                    stringifier.expression(expression);
                    format!("enum member = {}", stringifier.as_str())
                } else {
                    "enum member".to_string()
                }
            }
            SymbolKind::EnumMemberMangled => "enum member mangled".to_string(),
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
            SymbolKind::ClockDomain => "clock domain".to_string(),
            SymbolKind::Test(_) => "test".to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
    Inout,
    Interface,
    Modport,
    Import,
}

impl Direction {
    pub fn converse(&self) -> Direction {
        match self {
            Direction::Input => Direction::Output,
            Direction::Output => Direction::Input,
            _ => *self,
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Direction::Input => "input".to_string(),
            Direction::Output => "output".to_string(),
            Direction::Inout => "inout".to_string(),
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

impl Type {
    pub fn is_compatible(&self, other: &Type) -> bool {
        self.to_string() == other.to_string()
    }
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
    UserDefined(UserDefinedType),
    AbstractInterface(Option<StrId>),
}

impl TypeKind {
    pub fn is_clock(&self) -> bool {
        matches!(
            self,
            TypeKind::Clock | TypeKind::ClockPosedge | TypeKind::ClockNegedge
        )
    }

    pub fn is_reset(&self) -> bool {
        matches!(
            self,
            TypeKind::Reset
                | TypeKind::ResetAsyncHigh
                | TypeKind::ResetAsyncLow
                | TypeKind::ResetSyncHigh
                | TypeKind::ResetSyncLow
        )
    }

    pub fn is_2state(&self) -> bool {
        matches!(
            self,
            TypeKind::Bit
                | TypeKind::U32
                | TypeKind::U64
                | TypeKind::I32
                | TypeKind::I64
                | TypeKind::F32
                | TypeKind::F64
        )
    }

    pub fn is_4state(&self) -> bool {
        self.is_clock() | self.is_reset() | (*self == TypeKind::Logic)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDefinedType {
    pub path: Vec<StrId>,
    pub symbol: Option<SymbolId>,
}

impl UserDefinedType {
    fn new(path: Vec<StrId>) -> Self {
        Self { path, symbol: None }
    }
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
            TypeKind::UserDefined(x) => {
                text.push_str(&format!("{}", x.path.first().unwrap()));
                for path in &x.path[1..] {
                    text.push_str(&format!("::{path}"));
                }
            }
            TypeKind::AbstractInterface(x) => {
                if let Some(x) = x {
                    text.push_str(&format!("interface::{x}"));
                } else {
                    text.push_str("interface");
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

impl TryFrom<&syntax_tree::Expression> for Type {
    type Error = ();

    fn try_from(value: &syntax_tree::Expression) -> Result<Self, Self::Error> {
        let value = if value.expression_list.is_empty() {
            value.expression01.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression01_list.is_empty() {
            value.expression02.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression02_list.is_empty() {
            value.expression03.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression03_list.is_empty() {
            value.expression04.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression04_list.is_empty() {
            value.expression05.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression05_list.is_empty() {
            value.expression06.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression06_list.is_empty() {
            value.expression07.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression07_list.is_empty() {
            value.expression08.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression08_list.is_empty() {
            value.expression09.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression09_list.is_empty() {
            value.expression10.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression10_list.is_empty() {
            value.expression11.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression11_opt.is_none() {
            value.expression12.as_ref()
        } else {
            return Err(());
        };
        let value = if value.expression12_list.is_empty() {
            value.factor.as_ref()
        } else {
            return Err(());
        };

        match value {
            syntax_tree::Factor::FactorTypeFactor(x) => {
                let factor = &x.factor_type_factor;

                let mut modifier = Vec::new();
                for x in &factor.factor_type_factor_list {
                    match &*x.type_modifier {
                        syntax_tree::TypeModifier::Tri(_) => modifier.push(TypeModifier::Tri),
                        syntax_tree::TypeModifier::Signed(_) => modifier.push(TypeModifier::Signed),
                    }
                }
                let mut factor_type: Type = factor.factor_type.as_ref().into();
                factor_type.modifier = modifier;
                Ok(factor_type)
            }
            syntax_tree::Factor::IdentifierFactor(x) => {
                let factor = &x.identifier_factor;

                if factor.identifier_factor_opt.is_some() {
                    Err(())
                } else {
                    let x = factor.expression_identifier.as_ref();
                    if !x.expression_identifier_list.is_empty() {
                        return Err(());
                    }
                    if !x.expression_identifier_list0.is_empty() {
                        return Err(());
                    }

                    let mut name = Vec::new();
                    match x.scoped_identifier.scoped_identifier_group.as_ref() {
                        syntax_tree::ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                            name.push(x.identifier.identifier_token.token.text);
                        }
                        syntax_tree::ScopedIdentifierGroup::DollarIdentifier(x) => {
                            name.push(x.dollar_identifier.dollar_identifier_token.token.text);
                        }
                    }
                    for x in &x.scoped_identifier.scoped_identifier_list {
                        name.push(x.identifier.identifier_token.token.text);
                    }
                    let kind = TypeKind::UserDefined(UserDefinedType::new(name));
                    let width: Vec<syntax_tree::Expression> =
                        if let Some(ref x) = x.expression_identifier_opt {
                            x.width.as_ref().into()
                        } else {
                            Vec::new()
                        };
                    Ok(Type {
                        kind,
                        modifier: vec![],
                        width,
                        array: vec![],
                        is_const: false,
                    })
                }
            }
            _ => Err(()),
        }
    }
}

impl From<&syntax_tree::FactorType> for Type {
    fn from(value: &syntax_tree::FactorType) -> Self {
        match value.factor_type_group.as_ref() {
            syntax_tree::FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                let kind = match x.variable_type.as_ref() {
                    syntax_tree::VariableType::Clock(_) => TypeKind::Clock,
                    syntax_tree::VariableType::ClockPosedge(_) => TypeKind::ClockPosedge,
                    syntax_tree::VariableType::ClockNegedge(_) => TypeKind::ClockNegedge,
                    syntax_tree::VariableType::Reset(_) => TypeKind::Reset,
                    syntax_tree::VariableType::ResetAsyncHigh(_) => TypeKind::ResetAsyncHigh,
                    syntax_tree::VariableType::ResetAsyncLow(_) => TypeKind::ResetAsyncLow,
                    syntax_tree::VariableType::ResetSyncHigh(_) => TypeKind::ResetSyncHigh,
                    syntax_tree::VariableType::ResetSyncLow(_) => TypeKind::ResetSyncLow,
                    syntax_tree::VariableType::Logic(_) => TypeKind::Logic,
                    syntax_tree::VariableType::Bit(_) => TypeKind::Bit,
                };
                let width: Vec<syntax_tree::Expression> = if let Some(ref x) = x.factor_type_opt {
                    x.width.as_ref().into()
                } else {
                    Vec::new()
                };
                Type {
                    kind,
                    modifier: vec![],
                    width,
                    array: vec![],
                    is_const: false,
                }
            }
            syntax_tree::FactorTypeGroup::FixedType(x) => {
                let kind = match x.fixed_type.as_ref() {
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
                    modifier: vec![],
                    width: vec![],
                    array: vec![],
                    is_const: false,
                }
            }
        }
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
            syntax_tree::ScalarTypeGroup::UserDefinedTypeScalarTypeOpt(x) => {
                let ident = x.user_defined_type.scoped_identifier.as_ref();
                let mut name = Vec::new();
                match ident.scoped_identifier_group.as_ref() {
                    syntax_tree::ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                        name.push(x.identifier.identifier_token.token.text);
                    }
                    syntax_tree::ScopedIdentifierGroup::DollarIdentifier(x) => {
                        name.push(x.dollar_identifier.dollar_identifier_token.token.text);
                    }
                }
                for x in &ident.scoped_identifier_list {
                    name.push(x.identifier.identifier_token.token.text);
                }
                let kind = TypeKind::UserDefined(UserDefinedType::new(name));
                let width: Vec<syntax_tree::Expression> = if let Some(ref x) = x.scalar_type_opt {
                    x.width.as_ref().into()
                } else {
                    Vec::new()
                };
                Type {
                    kind,
                    modifier,
                    width,
                    array: vec![],
                    is_const: false,
                }
            }
            syntax_tree::ScalarTypeGroup::FactorType(x) => {
                let factor_type: Type = x.factor_type.as_ref().into();
                Type {
                    kind: factor_type.kind,
                    modifier,
                    width: factor_type.width,
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
        let array: Vec<syntax_tree::Expression> = if let Some(ref x) = value.array_type_opt {
            x.array.as_ref().into()
        } else {
            Vec::new()
        };
        Type {
            kind: scalar_type.kind,
            modifier: scalar_type.modifier,
            width: scalar_type.width,
            array,
            is_const: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClockDomain {
    Explicit(SymbolId),
    Implicit,
    None,
}

impl ClockDomain {
    pub fn compatible(&self, x: &ClockDomain) -> bool {
        match (self, x) {
            (ClockDomain::None, _) => true,
            (_, ClockDomain::None) => true,
            (x, y) => x == y,
        }
    }
}

impl fmt::Display for ClockDomain {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            ClockDomain::Explicit(x) => format!("'{}", symbol_table::get(*x).unwrap().token),
            ClockDomain::Implicit => "'_".to_string(),
            ClockDomain::None => "".to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Debug, Clone)]
pub struct VariableProperty {
    pub r#type: Type,
    pub affiliation: VariableAffiliation,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub clock_domain: ClockDomain,
    pub loop_variable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableAffiliation {
    Module,
    Intarface,
    Package,
    StatementBlock,
    Function,
}

#[derive(Debug, Clone)]
pub struct PortProperty {
    pub token: Token,
    pub r#type: Type,
    pub direction: Direction,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub clock_domain: ClockDomain,
    pub default_value: Option<syntax_tree::Expression>,
    pub is_proto: bool,
}

#[derive(Debug, Clone)]
pub struct Port {
    pub token: VerylToken,
    pub symbol: SymbolId,
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{} [{}]", self.name(), self.property().direction);
        text.fmt(f)
    }
}

impl Port {
    pub fn property(&self) -> PortProperty {
        if let SymbolKind::Port(x) = self.symbol().kind {
            x.clone()
        } else {
            unreachable!()
        }
    }

    pub fn symbol(&self) -> Symbol {
        symbol_table::get(self.symbol).unwrap()
    }

    pub fn name(&self) -> StrId {
        self.token.token.text
    }
}

#[derive(Debug, Clone)]
pub enum ParameterKind {
    Param,
    Const,
}

#[derive(Debug, Clone)]
pub struct ParameterProperty {
    pub token: Token,
    pub r#type: Type,
    pub kind: ParameterKind,
    pub value: syntax_tree::Expression,
}

#[derive(Debug, Clone)]
pub struct ProtoConstProperty {
    pub token: Token,
    pub r#type: Type,
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: StrId,
    pub symbol: SymbolId,
}

impl fmt::Display for Parameter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{} [{}]", self.name, self.property().r#type);
        text.fmt(f)
    }
}

impl Parameter {
    pub fn property(&self) -> ParameterProperty {
        if let SymbolKind::Parameter(x) = symbol_table::get(self.symbol).unwrap().kind {
            x.clone()
        } else {
            unreachable!()
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleProperty {
    pub range: TokenRange,
    pub proto: Option<SymbolPath>,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
    pub parameters: Vec<Parameter>,
    pub ports: Vec<Port>,
    pub default_clock: Option<SymbolId>,
    pub default_reset: Option<SymbolId>,
    pub definition: DefinitionId,
}

#[derive(Debug, Clone)]
pub struct ProtoModuleProperty {
    pub range: TokenRange,
    pub parameters: Vec<Parameter>,
    pub ports: Vec<Port>,
}

#[derive(Debug, Clone)]
pub struct AliasModuleProperty {
    pub target: GenericSymbolPath,
}

#[derive(Debug, Clone)]
pub struct InterfaceProperty {
    pub range: TokenRange,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
    pub parameters: Vec<Parameter>,
    pub definition: DefinitionId,
}

#[derive(Debug, Clone)]
pub struct AliasInterfaceProperty {
    pub target: GenericSymbolPath,
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
    pub identifiers: Vec<ConnectTargetIdentifier>,
    pub expression: syntax_tree::Expression,
}

#[derive(Debug, Clone)]
pub struct ConnectTargetIdentifier {
    pub path: Vec<(StrId, Vec<syntax_tree::Expression>)>,
}

impl ConnectTargetIdentifier {
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

impl From<&syntax_tree::ExpressionIdentifier> for ConnectTargetIdentifier {
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
    pub array: Vec<syntax_tree::Expression>,
    pub type_name: GenericSymbolPath,
    pub connects: HashMap<Token, ConnectTarget>,
    pub clock_domain: ClockDomain,
}

#[derive(Debug, Clone)]
pub struct PackageProperty {
    pub range: TokenRange,
    pub proto: Option<SymbolPath>,
    pub generic_parameters: Vec<SymbolId>,
    pub generic_references: Vec<GenericSymbolPath>,
    pub members: Vec<SymbolId>,
}

#[derive(Debug, Clone)]
pub struct AliasPackageProperty {
    pub target: GenericSymbolPath,
}

#[derive(Debug, Clone)]
pub struct ProtoPackageProperty {
    pub range: TokenRange,
    pub members: Vec<SymbolId>,
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
    pub r#type: Option<Type>,
    pub width: usize,
    pub members: Vec<SymbolId>,
    pub encoding: EnumEncodingItem,
}

#[derive(Debug, Clone)]
pub enum EnumMemberValue {
    ImplicitValue(usize),
    ExplicitValue(syntax_tree::Expression, Option<usize>),
    UnevaluableValue,
}

impl EnumMemberValue {
    pub fn value(&self) -> Option<usize> {
        match self {
            EnumMemberValue::ImplicitValue(value) => Some(*value),
            EnumMemberValue::ExplicitValue(_expression, evaluated) => *evaluated,
            EnumMemberValue::UnevaluableValue => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnumMemberProperty {
    pub value: EnumMemberValue,
    pub prefix: String,
}

#[derive(Debug, Clone)]
pub struct ModportProperty {
    pub interface: SymbolId,
    pub members: Vec<SymbolId>,
    pub default: Option<ModportDefault>,
}

#[derive(Debug, Clone)]
pub enum ModportDefault {
    Input,
    Output,
    Same(Token),
    Converse(Token),
}

#[derive(Debug, Clone)]
pub struct ModportVariableMemberProperty {
    pub direction: Direction,
}

#[derive(Debug, Clone)]
pub struct ModportFunctionMemberProperty {
    pub function: SymbolId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericBoundKind {
    Const,
    Type,
    Inst(SymbolPath),
    Proto(SymbolPath),
}

impl GenericBoundKind {
    pub fn is_compatible(&self, other: &GenericBoundKind) -> bool {
        self.to_string() == other.to_string()
    }
}

impl fmt::Display for GenericBoundKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            GenericBoundKind::Const => "const".to_string(),
            GenericBoundKind::Type => "type".to_string(),
            GenericBoundKind::Inst(x) => x.to_string(),
            GenericBoundKind::Proto(x) => x.to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Debug, Clone)]
pub struct GenericParameterProperty {
    pub bound: GenericBoundKind,
    pub default_value: Option<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub struct GenericInstanceProperty {
    pub base: SymbolId,
    pub arguments: Vec<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub enum TestType {
    Inline,
    CocotbEmbed(StrId),
    CocotbInclude(StrId),
}

#[derive(Debug, Clone)]
pub struct TestProperty {
    pub r#type: TestType,
    pub path: PathId,
    pub top: Option<StrId>,
}
