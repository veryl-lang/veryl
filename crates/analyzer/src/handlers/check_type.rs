use crate::analyzer_error::AnalyzerError;
use crate::attribute::AllowItem;
use crate::attribute::Attribute as Attr;
use crate::attribute_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{GenericBoundKind, Symbol, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};
use veryl_parser::{ParolError, Stringifier};

#[derive(Default)]
pub struct CheckType {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    in_module: bool,
    in_interface: bool,
    in_user_defined_type: Vec<()>,
    in_casting_type: Vec<()>,
    in_generic_argument: Vec<()>,
    in_generic_parameter: bool,
    in_generic_inst_parameter: bool,
    in_modport: bool,
    in_modport_default_member: bool,
    in_alias_module: bool,
    in_alias_interface: bool,
    in_alias_package: bool,
    in_proto_alias_module: bool,
    in_proto_alias_interface: bool,
    in_proto_alias_package: bool,
}

impl CheckType {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckType {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn is_variable_type(symbol: &Symbol) -> bool {
    match &symbol.kind {
        SymbolKind::Enum(_)
        | SymbolKind::Union(_)
        | SymbolKind::Struct(_)
        | SymbolKind::TypeDef(_)
        | SymbolKind::ProtoTypeDef
        | SymbolKind::SystemVerilog => true,
        SymbolKind::Parameter(x) => x.r#type.kind == TypeKind::Type,
        SymbolKind::GenericParameter(x) => x.bound == GenericBoundKind::Type,
        SymbolKind::GenericInstance(x) => {
            let base = symbol_table::get(x.base).unwrap();
            is_variable_type(&base)
        }
        _ => false,
    }
}

fn is_casting_type(symbol: &Symbol) -> bool {
    match &symbol.kind {
        // U8/U16/U32/U64 can be used as casting type
        SymbolKind::Parameter(x) => matches!(
            x.r#type.kind,
            TypeKind::Type | TypeKind::U8 | TypeKind::U16 | TypeKind::U32 | TypeKind::U64
        ),
        _ => is_variable_type(symbol),
    }
}

fn resolve_inst_generic_arg_type(arg: &GenericSymbolPath, namespace: &Namespace) -> Option<Symbol> {
    if !arg.is_resolvable() {
        return None;
    }

    let arg_symbol = symbol_table::resolve((&arg.generic_path(), namespace)).ok()?;
    let inst_symbol = if let SymbolKind::Instance(inst) = arg_symbol.found.kind {
        symbol_table::resolve((&inst.type_name.mangled_path(), namespace)).ok()?
    } else {
        return None;
    };

    inst_symbol
        .found
        .proto()
        .map(|x| symbol_table::get(x).unwrap())
}

fn resolve_proto_generic_arg_type(
    arg: &GenericSymbolPath,
    namespace: &Namespace,
) -> Option<SymbolId> {
    if !arg.is_resolvable() {
        return None;
    }

    let arg_symbol = symbol_table::resolve((&arg.generic_path(), namespace)).ok()?;
    arg_symbol.found.proto()
}

enum InstTypeSource {
    Id(SymbolId),
    Path(SymbolPathNamespace),
}

fn resolve_inst_type(arg: &InstTypeSource) -> Option<Symbol> {
    let symbol = match arg {
        InstTypeSource::Id(x) => symbol_table::get(*x)?,
        InstTypeSource::Path(x) => symbol_table::resolve(x).ok()?.found,
    };

    match &symbol.kind {
        SymbolKind::AliasModule(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type(&InstTypeSource::Path(path));
        }
        SymbolKind::AliasInterface(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type(&InstTypeSource::Path(path));
        }
        SymbolKind::GenericInstance(x) => {
            return resolve_inst_type(&InstTypeSource::Id(x.base));
        }
        SymbolKind::GenericParameter(x) => {
            if let GenericBoundKind::Proto(proto) = &x.bound {
                let path: SymbolPathNamespace = (proto, &symbol.namespace).into();
                return resolve_inst_type(&InstTypeSource::Path(path));
            }
        }
        _ => {}
    }

    Some(symbol)
}

impl VerylGrammarTrait for CheckType {
    fn user_defined_type(&mut self, _arg: &UserDefinedType) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_user_defined_type.push(());
            }
            HandlerPoint::After => {
                self.in_user_defined_type.pop();
            }
        }
        Ok(())
    }

    fn casting_type(&mut self, _arg: &CastingType) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_casting_type.push(());
            }
            HandlerPoint::After => {
                self.in_casting_type.pop();
            }
        }
        Ok(())
    }

    fn generic_bound(&mut self, arg: &GenericBound) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => match arg {
                GenericBound::InstScopedIdentifier(_) => self.in_generic_inst_parameter = true,
                GenericBound::ScopedIdentifier(_) => self.in_generic_parameter = true,
                _ => {}
            },
            HandlerPoint::After => {
                self.in_generic_parameter = false;
                self.in_generic_inst_parameter = false;
            }
        }
        Ok(())
    }

    fn with_generic_argument(&mut self, _arg: &WithGenericArgument) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_generic_argument.push(());
            }
            HandlerPoint::After => {
                self.in_generic_argument.pop();
            }
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        let is_modport = if let PortDeclarationItemGroup::PortTypeConcrete(x) =
            &*arg.port_declaration_item_group
        {
            let x = x.port_type_concrete.as_ref();
            matches!(&*x.direction, Direction::Modport(_))
        } else {
            false
        };

        match self.point {
            HandlerPoint::Before => self.in_modport = is_modport,
            HandlerPoint::After => self.in_modport = false,
        }
        Ok(())
    }

    fn identifier(&mut self, arg: &Identifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(symbol) = symbol_table::resolve(arg) {
                // Check modport default member type
                if self.in_modport_default_member
                    && !matches!(symbol.found.kind, SymbolKind::Modport(_))
                {
                    self.errors.push(AnalyzerError::mismatch_type(
                        &symbol.found.token.to_string(),
                        "modport",
                        &symbol.found.kind.to_kind_name(),
                        &arg.identifier_token.token.into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(symbol) = symbol_table::resolve(arg) {
                let symbol = symbol.found;

                // Mangled enum member can't be used directly
                if matches!(symbol.kind, SymbolKind::EnumMemberMangled) {
                    self.errors.push(AnalyzerError::undefined_identifier(
                        &symbol.token.to_string(),
                        &symbol.token.into(),
                    ));
                }

                // Check generic parameter
                if self.in_generic_parameter
                    && !(symbol.is_proto_module(false)
                        || symbol.is_proto_interface(false, false)
                        || symbol.is_proto_package(false))
                {
                    self.errors.push(AnalyzerError::mismatch_type(
                        &symbol.token.to_string(),
                        "proto module, proto interface or proto package",
                        &symbol.kind.to_kind_name(),
                        &arg.identifier().token.into(),
                    ));
                }
                if self.in_generic_inst_parameter && !symbol.is_proto_interface(false, true) {
                    self.errors.push(AnalyzerError::mismatch_type(
                        &symbol.token.to_string(),
                        "proto module, proto interface or non generic interface",
                        &symbol.kind.to_kind_name(),
                        &arg.identifier().token.into(),
                    ));
                }

                // Check variable type
                if !self.in_user_defined_type.is_empty() && self.in_generic_argument.is_empty() {
                    if self.in_modport {
                        if !matches!(
                            symbol.kind,
                            SymbolKind::Modport(_) | SymbolKind::SystemVerilog
                        ) {
                            self.errors.push(AnalyzerError::mismatch_type(
                                &symbol.token.to_string(),
                                "modport",
                                &symbol.kind.to_kind_name(),
                                &arg.identifier().token.into(),
                            ));
                        }
                    } else {
                        let type_error = if !self.in_casting_type.is_empty() {
                            !is_casting_type(&symbol)
                        } else {
                            !is_variable_type(&symbol)
                        };
                        if type_error {
                            self.errors.push(AnalyzerError::mismatch_type(
                                &symbol.token.to_string(),
                                "enum or union or struct",
                                &symbol.kind.to_kind_name(),
                                &arg.identifier().token.into(),
                            ));
                        }
                    }
                }

                // Check targe of alias
                if self.in_generic_argument.is_empty() {
                    let expected = if self.in_alias_module && !symbol.is_module(false) {
                        Some("module")
                    } else if self.in_proto_alias_module && !symbol.is_proto_module(true) {
                        Some("proto module")
                    } else if self.in_alias_interface && !symbol.is_interface(false) {
                        Some("interface")
                    } else if self.in_proto_alias_interface
                        && !symbol.is_proto_interface(true, false)
                    {
                        Some("proto interface")
                    } else if self.in_alias_package && !symbol.is_package(false) {
                        Some("package")
                    } else if self.in_proto_alias_package && !symbol.is_proto_package(true) {
                        Some("proto package")
                    } else {
                        None
                    };
                    if let Some(expected) = expected {
                        self.errors.push(AnalyzerError::mismatch_type(
                            &symbol.token.to_string(),
                            expected,
                            &symbol.kind.to_kind_name(),
                            &arg.identifier().token.into(),
                        ));
                    }
                }
            }

            // Check generic argument type
            let namespace = namespace_table::get(arg.identifier().token.id).unwrap();
            let path: GenericSymbolPath = arg.into();
            for i in 0..path.len() {
                let base_path = path.base_path(i);
                if let Ok(symbol) = symbol_table::resolve((&base_path, &namespace)) {
                    let params = symbol.found.generic_parameters();
                    let args = &path.paths[i].arguments;
                    let defined_namespace = symbol.found.namespace;

                    for (i, arg) in args.iter().enumerate() {
                        if let Some(param) = params.get(i) {
                            match &param.1.bound {
                                GenericBoundKind::Const => (),
                                GenericBoundKind::Type => {
                                    let is_type = if arg.is_resolvable() {
                                        if let Ok(symbol) =
                                            symbol_table::resolve((&arg.generic_path(), &namespace))
                                        {
                                            is_variable_type(&symbol.found)
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    };

                                    if !is_type {
                                        self.errors.push(AnalyzerError::mismatch_type(
                                            &symbol.found.token.to_string(),
                                            "enum or union or struct",
                                            &symbol.found.kind.to_kind_name(),
                                            &arg.range,
                                        ));
                                    }
                                }
                                GenericBoundKind::Inst(proto) => {
                                    let actual = resolve_inst_generic_arg_type(arg, &namespace);
                                    let required =
                                        symbol_table::resolve((proto, &defined_namespace));
                                    let proto_match = if let (Some(actual), Ok(required)) =
                                        (&actual, &required)
                                    {
                                        actual.id == required.found.id
                                    } else {
                                        false
                                    };

                                    if !proto_match {
                                        let (name, kind) = if let Some(x) = actual {
                                            (x.token.to_string(), x.kind.to_kind_name())
                                        } else {
                                            (
                                                symbol.found.token.to_string(),
                                                symbol.found.kind.to_kind_name(),
                                            )
                                        };
                                        self.errors.push(AnalyzerError::mismatch_type(
                                            &name,
                                            &format!("inst {proto}"),
                                            &kind,
                                            &arg.range,
                                        ));
                                    }
                                }
                                GenericBoundKind::Proto(proto) => {
                                    let actual = resolve_proto_generic_arg_type(arg, &namespace);
                                    let required =
                                        symbol_table::resolve((proto, &defined_namespace));
                                    let proto_match =
                                        if let (Some(actual), Ok(required)) = (actual, required) {
                                            actual == required.found.id
                                        } else {
                                            false
                                        };

                                    if !proto_match {
                                        self.errors.push(AnalyzerError::mismatch_type(
                                            &symbol.found.token.to_string(),
                                            &format!("proto {proto}"),
                                            &symbol.found.kind.to_kind_name(),
                                            &arg.range,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_interface {
                self.errors
                    .push(AnalyzerError::invalid_type_declaration("enum", &arg.into()));
            }
        }
        Ok(())
    }

    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_interface {
                let kind = match *arg.struct_union {
                    StructUnion::Struct(_) => "struct",
                    StructUnion::Union(_) => "union",
                };
                self.errors
                    .push(AnalyzerError::invalid_type_declaration(kind, &arg.into()));
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, _arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_module = true,
            HandlerPoint::After => self.in_module = false,
        };
        Ok(())
    }

    fn interface_declaration(&mut self, _arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_interface = true,
            HandlerPoint::After => self.in_interface = false,
        }
        Ok(())
    }

    fn modport_default(&mut self, _arg: &ModportDefault) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_modport_default_member = true,
            HandlerPoint::After => self.in_modport_default_member = false,
        }
        Ok(())
    }

    fn alias_declaration(&mut self, arg: &AliasDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => match &*arg.alias_declaration_group {
                AliasDeclarationGroup::Module(_) => self.in_alias_module = true,
                AliasDeclarationGroup::Interface(_) => self.in_alias_interface = true,
                AliasDeclarationGroup::Package(_) => self.in_alias_package = true,
            },
            HandlerPoint::After => {
                self.in_alias_module = false;
                self.in_alias_interface = false;
                self.in_alias_package = false;
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut connected_params = Vec::new();
            if let Some(ref x) = arg.inst_declaration_opt1 {
                if let Some(ref x) = x.inst_parameter.inst_parameter_opt {
                    let items: Vec<InstParameterItem> = x.inst_parameter_list.as_ref().into();
                    for item in items {
                        connected_params.push(item.identifier.identifier_token.token.text);
                    }
                }
            }

            let mut connected_ports = Vec::new();
            if let Some(ref x) = arg.inst_declaration_opt2 {
                if let Some(ref x) = x.inst_declaration_opt3 {
                    let items: Vec<InstPortItem> = x.inst_port_list.as_ref().into();
                    for item in items {
                        connected_ports.push(item.identifier.identifier_token.token.text);
                    }
                }
            }

            let path: SymbolPathNamespace = arg.scoped_identifier.as_ref().into();
            if let Some(symbol) = resolve_inst_type(&InstTypeSource::Path(path)) {
                let mut stringifier = Stringifier::new();
                stringifier.scoped_identifier(&arg.scoped_identifier);
                let name = stringifier.as_str();

                let mut params = vec![];
                let mut ports = vec![];
                let mut check_port_connection = false;

                let type_expected = match symbol.kind {
                    SymbolKind::Module(ref x) if self.in_module => {
                        params.append(&mut x.parameters.clone());
                        ports.append(&mut x.ports.clone());
                        check_port_connection = true;
                        None
                    }
                    SymbolKind::ProtoModule(ref x) if self.in_module => {
                        params.append(&mut x.parameters.clone());
                        ports.append(&mut x.ports.clone());
                        check_port_connection = true;
                        None
                    }
                    SymbolKind::Interface(ref x) => {
                        params.append(&mut x.parameters.clone());
                        check_port_connection = true;
                        None
                    }
                    SymbolKind::ProtoInterface(ref x) => {
                        params.append(&mut x.parameters.clone());
                        check_port_connection = true;
                        None
                    }
                    SymbolKind::SystemVerilog => None,
                    _ => {
                        if self.in_module {
                            Some("module or interface")
                        } else {
                            Some("interface")
                        }
                    }
                };

                if let Some(expected) = type_expected {
                    self.errors.push(AnalyzerError::mismatch_type(
                        name,
                        expected,
                        &symbol.kind.to_kind_name(),
                        &arg.identifier.as_ref().into(),
                    ));
                }

                if check_port_connection {
                    for port in &ports {
                        if !connected_ports.contains(&port.name())
                            && port.property().default_value.is_none()
                            && !attribute_table::contains(
                                &arg.inst.inst_token.token,
                                Attr::Allow(AllowItem::MissingPort),
                            )
                        {
                            let port = resource_table::get_str_value(port.name()).unwrap();
                            self.errors.push(AnalyzerError::missing_port(
                                name,
                                &port,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                    }
                    for param in &connected_params {
                        if !params.iter().any(|x| &x.name == param) {
                            let param = resource_table::get_str_value(*param).unwrap();
                            self.errors.push(AnalyzerError::unknown_param(
                                name,
                                &param,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                    }
                    for port in &connected_ports {
                        if !ports.iter().any(|x| &x.name() == port) {
                            let port = resource_table::get_str_value(*port).unwrap();
                            self.errors.push(AnalyzerError::unknown_port(
                                name,
                                &port,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn proto_alias_declaration(&mut self, arg: &ProtoAliasDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => match &*arg.proto_alias_declaration_group {
                ProtoAliasDeclarationGroup::Module(_) => self.in_proto_alias_module = true,
                ProtoAliasDeclarationGroup::Interface(_) => self.in_proto_alias_interface = true,
                ProtoAliasDeclarationGroup::Package(_) => self.in_proto_alias_package = true,
            },
            HandlerPoint::After => {
                self.in_proto_alias_module = false;
                self.in_proto_alias_interface = false;
                self.in_proto_alias_package = false;
            }
        }
        Ok(())
    }
}
