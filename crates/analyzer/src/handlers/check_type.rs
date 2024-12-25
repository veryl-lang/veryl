use crate::analyzer_error::AnalyzerError;
use crate::attribute::AllowItem;
use crate::attribute::Attribute as Attr;
use crate::attribute_table;
use crate::namespace_table;
use crate::symbol::{GenericBoundKind, Symbol, SymbolKind, TypeKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};
use veryl_parser::{ParolError, Stringifier};

#[derive(Default)]
pub struct CheckType<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    in_module: bool,
    in_user_defined_type: Vec<()>,
    in_casting_type: Vec<()>,
    in_generic_argument: Vec<()>,
    in_modport: bool,
}

impl<'a> CheckType<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl Handler for CheckType<'_> {
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
        // U32/U64 can be used as casting type
        SymbolKind::Parameter(x) => matches!(
            x.r#type.kind,
            TypeKind::Type | TypeKind::U32 | TypeKind::U64
        ),
        _ => is_variable_type(symbol),
    }
}

impl VerylGrammarTrait for CheckType<'_> {
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
                // Check enum variant should be scoped by enum
                // refs: https://github.com/veryl-lang/veryl/issues/1114
                if matches!(symbol.found.kind, SymbolKind::EnumMember(_)) {
                    let preceed_id = symbol.full_path.get(symbol.full_path.len() - 2).unwrap();
                    let preceed_symbol = symbol_table::get(*preceed_id).unwrap();
                    if !matches!(
                        preceed_symbol.kind,
                        SymbolKind::Enum(_) | SymbolKind::TypeDef(_)
                    ) {
                        self.errors.push(AnalyzerError::mismatch_type(
                            &preceed_symbol.token.to_string(),
                            "enum",
                            &preceed_symbol.kind.to_kind_name(),
                            self.text,
                            &preceed_symbol.token.into(),
                        ));
                    }
                }

                // Check variable type
                if !self.in_user_defined_type.is_empty() && self.in_generic_argument.is_empty() {
                    if self.in_modport {
                        if !matches!(symbol.found.kind, SymbolKind::Modport(_)) {
                            self.errors.push(AnalyzerError::mismatch_type(
                                &symbol.found.token.to_string(),
                                "modport",
                                &symbol.found.kind.to_kind_name(),
                                self.text,
                                &arg.identifier().token.into(),
                            ));
                        }
                    } else {
                        let type_error = if !self.in_casting_type.is_empty() {
                            !is_casting_type(&symbol.found)
                        } else {
                            !is_variable_type(&symbol.found)
                        };
                        if type_error {
                            self.errors.push(AnalyzerError::mismatch_type(
                                &symbol.found.token.to_string(),
                                "enum or union or struct",
                                &symbol.found.kind.to_kind_name(),
                                self.text,
                                &arg.identifier().token.into(),
                            ));
                        }
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
                                            self.text,
                                            &arg.range,
                                        ));
                                    }
                                }
                                GenericBoundKind::Proto(proto) => {
                                    let proto_match = if arg.is_resolvable() {
                                        if let Ok(symbol) =
                                            symbol_table::resolve((&arg.generic_path(), &namespace))
                                        {
                                            if let Some(ref x) = symbol.found.kind.proto() {
                                                let actual = symbol_table::resolve((x, &namespace));
                                                let required = symbol_table::resolve((
                                                    proto,
                                                    &defined_namespace,
                                                ));
                                                if let (Ok(actual), Ok(required)) =
                                                    (actual, required)
                                                {
                                                    actual.found.id == required.found.id
                                                } else {
                                                    false
                                                }
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    };

                                    if !proto_match {
                                        self.errors.push(AnalyzerError::mismatch_type(
                                            &symbol.found.token.to_string(),
                                            &format!("proto {proto}"),
                                            &symbol.found.kind.to_kind_name(),
                                            self.text,
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

    fn module_declaration(&mut self, _arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_module = true,
            HandlerPoint::After => self.in_module = false,
        };
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut connected_params = Vec::new();
            if let Some(ref x) = arg.inst_declaration_opt0 {
                if let Some(ref x) = x.inst_parameter.inst_parameter_opt {
                    let items: Vec<InstParameterItem> = x.inst_parameter_list.as_ref().into();
                    for item in items {
                        connected_params.push(item.identifier.identifier_token.token.text);
                    }
                }
            }

            let mut connected_ports = Vec::new();
            if let Some(ref x) = arg.inst_declaration_opt1 {
                if let Some(ref x) = x.inst_declaration_opt2 {
                    let items: Vec<InstPortItem> = x.inst_port_list.as_ref().into();
                    for item in items {
                        connected_ports.push(item.identifier.identifier_token.token.text);
                    }
                }
            }

            if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                let mut stringifier = Stringifier::new();
                stringifier.scoped_identifier(&arg.scoped_identifier);
                let name = stringifier.as_str();

                let mut params = vec![];
                let mut ports = vec![];
                let mut check_port_connection = false;

                let type_expected = match symbol.found.kind {
                    SymbolKind::Module(ref x) if self.in_module => {
                        params.append(&mut x.parameters.clone());
                        ports.append(&mut x.ports.clone());
                        check_port_connection = true;
                        None
                    }
                    SymbolKind::Interface(_) | SymbolKind::SystemVerilog => None,
                    SymbolKind::GenericInstance(ref x) => {
                        let base = symbol_table::get(x.base).unwrap();
                        match base.kind {
                            SymbolKind::Module(ref x) if self.in_module => {
                                params.append(&mut x.parameters.clone());
                                ports.append(&mut x.ports.clone());
                                check_port_connection = true;
                                None
                            }
                            SymbolKind::Interface(_) | SymbolKind::SystemVerilog => None,
                            _ => {
                                if self.in_module {
                                    Some("module or interface")
                                } else {
                                    Some("interface")
                                }
                            }
                        }
                    }
                    SymbolKind::GenericParameter(ref x) => {
                        if let GenericBoundKind::Proto(ref x) = x.bound {
                            if let Ok(symbol) = symbol_table::resolve((x, &symbol.found.namespace))
                            {
                                if let SymbolKind::ProtoModule(x) = symbol.found.kind {
                                    params.append(&mut x.parameters.clone());
                                    ports.append(&mut x.ports.clone());
                                    check_port_connection = true;
                                    None
                                } else {
                                    Some("module or interface")
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
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
                        &symbol.found.kind.to_kind_name(),
                        self.text,
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
                                self.text,
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
                                self.text,
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
                                self.text,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
