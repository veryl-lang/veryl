use std::collections::HashMap;
use std::path::PathBuf;
use veryl_parser::veryl_grammar_trait::{BuiltinType, TypeGroup};
use veryl_parser::ParolLocation;

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
        name: String,
    },
}

#[derive(Debug, Clone)]
pub enum Direction {
    Input,
    Output,
    Inout,
    Ref,
    ModPort { interface: String, modport: String },
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
    UserDefined(String),
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
            TypeGroup::Identifier(x) => {
                Type::UserDefined(x.identifier.identifier_token.text().into())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Port {
    pub name: String,
    pub direction: Direction,
}

impl From<&veryl_parser::veryl_grammar_trait::PortDeclarationItem> for Port {
    fn from(value: &veryl_parser::veryl_grammar_trait::PortDeclarationItem) -> Self {
        let direction: Direction = (&*value.direction).into();
        Port {
            name: value.identifier.identifier_token.text().into(),
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
    pub name: String,
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
            name: value.identifier.identifier_token.text().into(),
            r#type,
            scope,
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct HierarchicalName {
    pub paths: Vec<String>,
}

impl From<&veryl_parser::veryl_grammar_trait::HierarchicalIdentifier> for HierarchicalName {
    fn from(value: &veryl_parser::veryl_grammar_trait::HierarchicalIdentifier) -> Self {
        let mut paths = Vec::new();
        paths.push(value.identifier.identifier_token.text().into());
        for x in &value.hierarchical_identifier_list0 {
            paths.push(x.identifier.identifier_token.text().into());
        }
        Self { paths }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct NameSpace {
    pub paths: Vec<String>,
}

impl NameSpace {
    pub fn push(&mut self, path: &str) {
        self.paths.push(path.to_owned());
    }

    pub fn pop(&mut self) {
        self.paths.pop();
    }

    pub fn depth(&self) -> usize {
        self.paths.len()
    }

    pub fn included(&self, x: &NameSpace) -> bool {
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
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub file_name: PathBuf,
}

impl From<&ParolLocation> for Location {
    fn from(x: &ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            file_name: PathBuf::from(&*x.file_name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub name_space: NameSpace,
    pub location: Location,
}

impl Symbol {
    pub fn new(name: &str, kind: SymbolKind, name_space: &NameSpace, location: &Location) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            name_space: name_space.to_owned(),
            location: location.to_owned(),
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    table: HashMap<String, Vec<Symbol>>,
}

impl SymbolTable {
    pub fn insert(&mut self, name: &str, symbol: Symbol) -> bool {
        let entry = self.table.entry(name.to_owned()).or_default();
        for item in entry.iter() {
            if symbol.name_space == item.name_space {
                return false;
            }
        }
        entry.push(symbol);
        true
    }

    pub fn get(
        &self,
        hierarchical_name: &HierarchicalName,
        name_space: &NameSpace,
    ) -> Option<&Symbol> {
        let mut ret = None;
        let mut name_space = name_space.clone();
        for name in &hierarchical_name.paths {
            let mut max_depth = 0;
            ret = None;
            if let Some(symbols) = self.table.get(name) {
                for symbol in symbols {
                    if name_space.included(&symbol.name_space)
                        && symbol.name_space.depth() > max_depth
                    {
                        ret = Some(symbol);
                        max_depth = symbol.name_space.depth();
                    }
                }

                if let Some(ret) = ret {
                    if let SymbolKind::Instance { ref name } = ret.kind {
                        name_space = NameSpace::default();
                        name_space.push(name);
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
