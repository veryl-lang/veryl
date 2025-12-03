use crate::analyzer_error::AnalyzerError;
use crate::attribute::AllowItem;
use crate::attribute::Attribute as Attr;
use crate::attribute_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{
    GenericBoundKind, ProtoBound, Symbol, SymbolId, SymbolKind, Type as SymType, TypeKind,
    TypeModifierKind,
};
use crate::symbol_path::{GenericSymbolPath, GenericSymbolPathKind, SymbolPathNamespace};
use crate::symbol_table;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
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
    modport_name: Option<Token>,
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

fn is_referable_generic_arg_symbol(symbol: &Symbol, defined_namesapce: &Namespace) -> bool {
    match &symbol.kind {
        SymbolKind::Package(_)
        | SymbolKind::ProtoPackage(_)
        | SymbolKind::GenericParameter(_)
        | SymbolKind::SystemVerilog => {
            return true;
        }
        SymbolKind::GenericInstance(x) => {
            return symbol_table::get(x.base)
                .map(|x| is_referable_generic_arg_symbol(&x, defined_namesapce))
                .unwrap_or(false);
        }
        _ => {
            if symbol.is_variable_type()
                || matches!(
                    symbol.kind,
                    SymbolKind::Parameter(_) | SymbolKind::ProtoConst(_) | SymbolKind::Instance(_)
                )
            {
                if symbol.namespace.matched(defined_namesapce) {
                    return true;
                } else if let Some(parent) = symbol.get_parent() {
                    return is_referable_generic_arg_symbol(&parent, defined_namesapce);
                }
            }
        }
    }

    false
}

fn is_referable_generic_arg(full_path: &[SymbolId], defined_namesapce: &Namespace) -> bool {
    full_path
        .iter()
        .map(|x| symbol_table::get(*x).unwrap())
        .any(|x| is_referable_generic_arg_symbol(&x, defined_namesapce))
}

fn check_generic_type_arg(
    arg: &GenericSymbolPath,
    namespace: &Namespace,
    base: &Symbol,
) -> Option<AnalyzerError> {
    if matches!(arg.kind, GenericSymbolPathKind::FixedType(_)) {
        None
    } else if arg.is_resolvable() {
        let Ok(symbol) = symbol_table::resolve((&arg.generic_path(), namespace)) else {
            // Undefiend identifier has been checked at 'analyze_post_pass1' phase
            return None;
        };

        if symbol.found.is_variable_type() {
            if is_referable_generic_arg_symbol(&symbol.found, &base.namespace) {
                return None;
            }

            Some(AnalyzerError::unresolvable_generic_argument(
                &arg.to_string(),
                &arg.range,
                &base.token.into(),
            ))
        } else {
            Some(AnalyzerError::mismatch_type(
                &arg.to_string(),
                "variable type",
                &symbol.found.kind.to_kind_name(),
                &arg.range,
            ))
        }
    } else {
        Some(AnalyzerError::mismatch_type(
            &arg.to_string(),
            "variable type",
            &arg.kind.to_string(),
            &arg.range,
        ))
    }
}

fn check_generic_inst_arg(
    arg: &GenericSymbolPath,
    namespace: &Namespace,
    bound: &GenericBoundKind,
    base: &Symbol,
) -> Option<AnalyzerError> {
    let required = bound.resolve_inst_bound(&base.namespace)?;
    let actual = if arg.is_resolvable() {
        'inst: {
            let arg_symbol = symbol_table::resolve((&arg.generic_path(), namespace)).ok()?;
            let SymbolKind::Instance(inst) = &arg_symbol.found.kind else {
                break 'inst arg_symbol.found.kind.to_kind_name();
            };
            if !is_referable_generic_arg_symbol(&arg_symbol.found, &base.namespace) {
                return Some(AnalyzerError::unresolvable_generic_argument(
                    &arg.to_string(),
                    &arg.range,
                    &base.token.into(),
                ));
            }

            let Ok(type_symbol) =
                symbol_table::resolve((&inst.type_name.mangled_path(), namespace))
            else {
                return None;
            };

            let Some(proto_symbol) = type_symbol.found.proto() else {
                break 'inst type_symbol.found.kind.to_kind_name();
            };

            if proto_symbol.id == required.id {
                return None;
            }

            proto_symbol.kind.to_kind_name()
        }
    } else {
        arg.kind.to_string()
    };

    Some(AnalyzerError::mismatch_type(
        &arg.to_string(),
        &format!("inst {}", required.token),
        &actual,
        &arg.range,
    ))
}

fn check_generic_proto_arg(
    arg: &GenericSymbolPath,
    namespace: &Namespace,
    bound: &GenericBoundKind,
    base: &Symbol,
) -> Option<AnalyzerError> {
    let required = bound.resolve_proto_bound(&base.namespace)?;
    let arg_symbol = if arg.is_resolvable() {
        let symbol = symbol_table::resolve((&arg.generic_path(), namespace)).ok();
        if symbol.is_some() {
            symbol
        } else {
            return None;
        }
    } else {
        None
    };
    let (arg_type, type_symbol) = if let Some(x) = &arg_symbol {
        let proto_symbol = x.found.proto();
        if proto_symbol.is_some() {
            (None, proto_symbol)
        } else if let Some(r#type) = x.found.kind.get_type() {
            r#type
                .trace_user_defined(Some(&x.found.namespace))
                .map(|(x, y)| (Some(x), y))
                .unwrap_or((None, None))
        } else if let Some(alias_target) = resolve_alias(&x.found) {
            (None, Some(alias_target))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let expected = match &required {
        ProtoBound::ProtoModule(r)
        | ProtoBound::ProtoInterface(r)
        | ProtoBound::ProtoPackage(r) => {
            if type_symbol.as_ref().map(|x| x.id == r.id).unwrap_or(false) {
                None
            } else {
                Some(format!("proto {}", r.token))
            }
        }
        ProtoBound::Enum((r, _)) => {
            let is_matched = if let Some(arg_symbol) = arg_symbol.as_ref() {
                if let SymbolKind::EnumMember(_) = arg_symbol.found.kind {
                    arg_symbol
                        .found
                        .get_parent()
                        .map(|x| x.id == r.id)
                        .unwrap_or(false)
                } else {
                    type_symbol.as_ref().map(|x| x.id == r.id).unwrap_or(false)
                }
            } else {
                false
            };
            if is_matched {
                None
            } else {
                Some(format!("{}", r.token))
            }
        }
        ProtoBound::Struct((r, _)) | ProtoBound::Union((r, _)) => {
            if type_symbol.as_ref().map(|x| x.id == r.id).unwrap_or(false) {
                None
            } else {
                Some(format!("{}", r.token))
            }
        }
        ProtoBound::FactorType(param_type) => {
            let is_matched = if let Some(arg_type) = arg_type.as_ref() {
                match_fixed_type(arg_type, param_type)
            } else if let Some(arg_symbol) = arg_symbol.as_ref() {
                match &arg_symbol.found.kind {
                    SymbolKind::Parameter(_) | SymbolKind::ProtoConst(_) => true,
                    SymbolKind::GenericParameter(x) => x
                        .bound
                        .resolve_proto_bound(&arg_symbol.found.namespace)
                        .map(|x| x.is_variable_type())
                        .unwrap_or(false),
                    _ => false,
                }
            } else {
                // if `arg_symbol` is none,
                // it means that the given generic arg is a literal
                true
            };
            if is_matched {
                None
            } else {
                Some(format!("{param_type}"))
            }
        }
    };

    if let Some(expected) = expected {
        let actual = if let Some(type_symbol) = type_symbol {
            type_symbol.kind.to_kind_name()
        } else {
            arg.kind.to_string()
        };
        return Some(AnalyzerError::mismatch_type(
            &arg.to_string(),
            &expected,
            &actual,
            &arg.range,
        ));
    }

    if required.is_variable_type()
        && !arg_symbol
            .map(|x| is_referable_generic_arg(&x.full_path, &base.namespace))
            .unwrap_or(true)
    {
        return Some(AnalyzerError::unresolvable_generic_argument(
            &arg.to_string(),
            &arg.range,
            &base.token.into(),
        ));
    }

    None
}

fn resolve_alias(symbol: &Symbol) -> Option<Symbol> {
    let target_path = symbol.alias_target(true)?;
    let target_symbol =
        symbol_table::resolve((&target_path.generic_path(), &symbol.namespace)).ok()?;
    if let Some(proto) = target_symbol.found.proto() {
        Some(proto)
    } else {
        Some(target_symbol.found)
    }
}

fn check_inst(
    in_module: bool,
    header_token: &Token,
    arg: &ComponentInstantiation,
) -> Vec<AnalyzerError> {
    let mut errors = Vec::new();

    let mut connected_params = Vec::new();
    if let Some(ref x) = arg.component_instantiation_opt1
        && let Some(ref x) = x.inst_parameter.inst_parameter_opt
    {
        let items: Vec<_> = x.inst_parameter_list.as_ref().into();
        for item in items {
            connected_params.push(item.identifier.identifier_token.token.text);
        }
    }

    let mut connected_ports = Vec::new();
    if let Some(ref x) = arg.component_instantiation_opt2
        && let Some(ref x) = x.inst_port.inst_port_opt
    {
        let items: Vec<_> = x.inst_port_list.as_ref().into();
        for item in items {
            connected_ports.push(item.identifier.identifier_token.token.text);
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
            SymbolKind::Module(ref x) if in_module => {
                params.append(&mut x.parameters.clone());
                ports.append(&mut x.ports.clone());
                check_port_connection = true;
                None
            }
            SymbolKind::ProtoModule(ref x) if in_module => {
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
                if in_module {
                    Some("module or interface")
                } else {
                    Some("interface")
                }
            }
        };

        if let Some(expected) = type_expected {
            errors.push(AnalyzerError::mismatch_type(
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
                    && !attribute_table::contains(header_token, Attr::Allow(AllowItem::MissingPort))
                {
                    let port = resource_table::get_str_value(port.name()).unwrap();
                    errors.push(AnalyzerError::missing_port(
                        name,
                        &port,
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
            for param in &connected_params {
                if !params.iter().any(|x| &x.name == param) {
                    let param = resource_table::get_str_value(*param).unwrap();
                    errors.push(AnalyzerError::unknown_param(
                        name,
                        &param,
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
            for port in &connected_ports {
                if !ports.iter().any(|x| &x.name() == port) {
                    let port = resource_table::get_str_value(*port).unwrap();
                    errors.push(AnalyzerError::unknown_port(
                        name,
                        &port,
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
        }
    }

    errors
}

fn check_bind_target(identifier: &ScopedIdentifier, target: &Symbol) -> Option<AnalyzerError> {
    if !(target.is_module(false) || target.is_interface(false)) {
        let mut stringifier = Stringifier::new();
        stringifier.scoped_identifier(identifier);
        let name = stringifier.as_str();

        Some(AnalyzerError::mismatch_type(
            name,
            "module or interface",
            &target.kind.to_kind_name(),
            &identifier.into(),
        ))
    } else {
        None
    }
}

fn match_fixed_type(arg_type: &SymType, param_type: &SymType) -> bool {
    if arg_type.modifier.len() != param_type.modifier.len() {
        return false;
    }

    for i in 0..arg_type.modifier.len() {
        if arg_type.modifier[i].kind != param_type.modifier[i].kind {
            return false;
        }
    }

    if !arg_type.width.is_empty() || !arg_type.array.is_empty() {
        return false;
    }

    if matches!(param_type.kind, TypeKind::Bool | TypeKind::String) {
        arg_type.kind == param_type.kind
    } else {
        arg_type.kind.is_fixed()
    }
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
        SymbolKind::AliasModule(x) | SymbolKind::ProtoAliasModule(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type(&InstTypeSource::Path(path));
        }
        SymbolKind::AliasInterface(x) | SymbolKind::ProtoAliasInterface(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type(&InstTypeSource::Path(path));
        }
        SymbolKind::GenericInstance(x) => {
            return resolve_inst_type(&InstTypeSource::Id(x.base));
        }
        SymbolKind::GenericParameter(x) => {
            let proto = x.bound.resolve_proto_bound(&symbol.namespace)?;
            if let Some(symbol) = proto.get_symbol() {
                return resolve_inst_type(&InstTypeSource::Id(symbol.id));
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
                GenericBound::GenericProtoBound(_) => self.in_generic_parameter = true,
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
                if self.in_generic_parameter {
                    let is_valid = symbol.is_proto_module(false)
                        || symbol.is_proto_interface(false, false)
                        || symbol.is_proto_package(false)
                        || symbol.is_variable_type();
                    if !is_valid {
                        self.errors.push(AnalyzerError::mismatch_type(
                            &symbol.token.to_string(),
                            "proto module, proto interface, proto package or variable type",
                            &symbol.kind.to_kind_name(),
                            &arg.identifier().token.into(),
                        ));
                    }
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
                if !self.in_user_defined_type.is_empty()
                    && !self.in_generic_parameter
                    && self.in_generic_argument.is_empty()
                {
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
                            !symbol.is_casting_type()
                        } else {
                            !symbol.is_variable_type()
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

                    for (i, arg) in args.iter().enumerate() {
                        if let Some(param) = params.get(i) {
                            let bound = &param.1.bound;
                            match bound {
                                GenericBoundKind::Type => {
                                    if let Some(err) =
                                        check_generic_type_arg(arg, &namespace, &symbol.found)
                                    {
                                        self.errors.push(err);
                                    }
                                }
                                GenericBoundKind::Inst(_) => {
                                    if let Some(err) = check_generic_inst_arg(
                                        arg,
                                        &namespace,
                                        bound,
                                        &symbol.found,
                                    ) {
                                        self.errors.push(err);
                                    }
                                }
                                GenericBoundKind::Proto(_) => {
                                    if let Some(err) = check_generic_proto_arg(
                                        arg,
                                        &namespace,
                                        bound,
                                        &symbol.found,
                                    ) {
                                        self.errors.push(err);
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

    fn scalar_type(&mut self, arg: &ScalarType) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = arg.into();
            if let Some(modifier) = r#type.find_modifier(&TypeModifierKind::Signed)
                && let Some((atom_type, _)) = r#type.trace_user_defined(None)
                && atom_type.kind.is_fixed()
            {
                self.errors
                    .push(AnalyzerError::fixed_type_with_signed_modifier(
                        &modifier.token.token.into(),
                    ));
            }
        }
        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && arg.for_statement_opt.is_some()
            && let Some((r#type, _)) =
                SymType::from(arg.scalar_type.as_ref()).trace_user_defined(None)
            && !r#type.is_signed()
        {
            self.errors.push(
                AnalyzerError::unsigned_loop_variable_in_descending_order_for_loop(
                    &arg.scalar_type.as_ref().into(),
                ),
            );
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && self.in_interface
        {
            self.errors
                .push(AnalyzerError::invalid_type_declaration("enum", &arg.into()));
        }
        Ok(())
    }

    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && self.in_interface
        {
            let kind = match *arg.struct_union {
                StructUnion::Struct(_) => "struct",
                StructUnion::Union(_) => "union",
            };
            self.errors
                .push(AnalyzerError::invalid_type_declaration(kind, &arg.into()));
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

    fn modport_declaration(&mut self, arg: &ModportDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.modport_name = Some(arg.identifier.identifier_token.token),
            HandlerPoint::After => self.modport_name = None,
        }
        Ok(())
    }

    fn modport_default(&mut self, arg: &ModportDefault) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && let Some(modport_name) = self.modport_name
        {
            let default_member_identifier = match arg {
                ModportDefault::ConverseLParenIdentifierRParen(x) => x.identifier.as_ref(),
                ModportDefault::SameLParenIdentifierRParen(x) => x.identifier.as_ref(),
                _ => return Ok(()),
            };
            let Ok(symbol) = symbol_table::resolve(default_member_identifier) else {
                return Ok(());
            };

            if !matches!(symbol.found.kind, SymbolKind::Modport(_)) {
                // Check modport default member type
                self.errors.push(AnalyzerError::mismatch_type(
                    &symbol.found.token.to_string(),
                    "modport",
                    &symbol.found.kind.to_kind_name(),
                    &default_member_identifier.identifier_token.token.into(),
                ));
            } else if symbol.found.token.text == modport_name.text {
                // Check self reference
                self.errors.push(AnalyzerError::mismatch_type(
                    &symbol.found.token.to_string(),
                    "other modport",
                    "ownself",
                    &default_member_identifier.identifier_token.token.into(),
                ));
            }
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
            self.errors.append(&mut check_inst(
                self.in_module,
                &arg.inst.inst_token.token,
                &arg.component_instantiation,
            ));
        }
        Ok(())
    }

    fn bind_declaration(&mut self, arg: &BindDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref())
        {
            if let Some(err) = check_bind_target(&arg.scoped_identifier, &symbol.found) {
                self.errors.push(err);
                return Ok(());
            }

            self.errors.append(&mut check_inst(
                symbol.found.is_module(true),
                &arg.bind.bind_token.token,
                &arg.component_instantiation,
            ));
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
