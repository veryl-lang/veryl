use crate::allow_table;
use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::Direction as SymDirection;
use crate::symbol::Type as SymType;
use crate::symbol::{
    EnumMemberProperty, EnumProperty, FunctionProperty, InstanceProperty, InterfaceProperty,
    ModportMember, ModportProperty, ModuleProperty, ParameterProperty, ParameterScope,
    ParameterValue, PortProperty, StructMemberProperty, Symbol, SymbolKind, TypeKind,
    VariableProperty,
};
use crate::symbol_table;
use std::collections::HashSet;
use veryl_parser::doc_comment_table;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateSymbolTable<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    namespace: Namespace,
    default_block: Option<StrId>,
    for_identifier: Option<VerylToken>,
    anonymous_namespace: usize,
    attribute_lines: HashSet<usize>,
}

impl<'a> CreateSymbolTable<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    fn insert_symbol(&mut self, token: &VerylToken, kind: SymbolKind) {
        let file = token.token.file_path;
        let line = token.token.line;
        let doc_comment = if line == 0 {
            vec![]
        } else if let Some(doc_comment) = doc_comment_table::get(file, line) {
            vec![doc_comment]
        } else {
            let mut candidate_line = line - 1;
            while self.attribute_lines.contains(&candidate_line) {
                if candidate_line == 0 {
                    break;
                }
                candidate_line -= 1;
            }
            let mut ret = Vec::new();
            while let Some(doc_comment) = doc_comment_table::get(file, candidate_line) {
                ret.push(doc_comment);
                candidate_line -= 1;
            }
            ret.reverse();
            ret
        };
        let mut symbol = Symbol::new(&token.token, kind, &self.namespace, doc_comment);

        if allow_table::contains("unused_variable") {
            symbol.allow_unused = true;
        }

        if !symbol_table::insert(&token.token, symbol) {
            let text = resource_table::get_str_value(token.token.text).unwrap();
            self.errors.push(AnalyzerError::duplicated_identifier(
                &text, self.text, token,
            ));
        }
    }
}

impl<'a> Handler for CreateSymbolTable<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateSymbolTable<'a> {
    fn identifier(&mut self, arg: &Identifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.identifier_token.token.id;
            let file_path = arg.identifier_token.token.file_path;
            namespace_table::insert(id, file_path, &self.namespace);
        }
        Ok(())
    }

    fn attribute(&mut self, arg: &Attribute) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.attribute_lines.insert(arg.hash.hash_token.token.line);
        }
        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let name = format!("@{}", self.anonymous_namespace);
                let name = resource_table::insert_str(&name);
                self.namespace.push(name);
                self.anonymous_namespace += 1;

                let r#type: SymType = arg.scalar_type.as_ref().into();
                let property = VariableProperty { r#type };
                let kind = SymbolKind::Variable(property);
                self.insert_symbol(&arg.identifier.identifier_token, kind);
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn var_declaration(&mut self, arg: &VarDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = arg.array_type.as_ref().into();
            let property = VariableProperty { r#type };
            let kind = SymbolKind::Variable(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let property = match &*arg.localparam_declaration_group {
                LocalparamDeclarationGroup::ArrayTypeEquExpression(x) => {
                    let r#type: SymType = x.array_type.as_ref().into();
                    let value = ParameterValue::Expression(*x.expression.clone());
                    ParameterProperty {
                        token,
                        r#type,
                        scope: ParameterScope::Local,
                        value,
                    }
                }
                LocalparamDeclarationGroup::TypeEquTypeExpression(x) => {
                    let r#type: SymType = SymType {
                        modifier: vec![],
                        kind: TypeKind::Type,
                        width: vec![],
                        array: vec![],
                    };
                    let value = ParameterValue::TypeExpression(*x.type_expression.clone());
                    ParameterProperty {
                        token,
                        r#type,
                        scope: ParameterScope::Local,
                        value,
                    }
                }
            };
            let kind = SymbolKind::Parameter(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn modport_declaration(&mut self, arg: &ModportDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut members = Vec::new();
            let items: Vec<ModportItem> = arg.modport_list.as_ref().into();

            self.namespace
                .push(arg.identifier.identifier_token.token.text);

            for item in items {
                let member = ModportMember {
                    name: item.identifier.identifier_token.token.text,
                    direction: item.direction.as_ref().into(),
                };
                members.push(member);
                self.insert_symbol(&item.identifier.identifier_token, SymbolKind::ModportMember);
            }

            self.namespace.pop();

            let property = ModportProperty { members };
            let kind = SymbolKind::Modport(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let r#type = arg.scalar_type.as_ref().into();
                let property = EnumProperty { r#type };
                let kind = SymbolKind::Enum(property);
                self.insert_symbol(&arg.identifier.identifier_token, kind);

                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn enum_item(&mut self, arg: &EnumItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let value = arg.enum_item_opt.as_ref().map(|x| *x.expression.clone());
            let property = EnumMemberProperty { value };
            let kind = SymbolKind::EnumMember(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn struct_declaration(&mut self, arg: &StructDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let kind = SymbolKind::Struct;
                self.insert_symbol(&arg.identifier.identifier_token, kind);

                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn struct_item(&mut self, arg: &StructItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type = arg.scalar_type.as_ref().into();
            let property = StructMemberProperty { r#type };
            let kind = SymbolKind::StructMember(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut type_name = vec![arg.scoped_identifier.identifier.identifier_token.token.text];
            for x in &arg.scoped_identifier.scoped_identifier_list {
                type_name.push(x.identifier.identifier_token.token.text);
            }
            let property = InstanceProperty { type_name };
            let kind = SymbolKind::Instance(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let scope = match &*arg.with_parameter_item_group {
                WithParameterItemGroup::Parameter(_) => ParameterScope::Global,
                WithParameterItemGroup::Localparam(_) => ParameterScope::Local,
            };
            let property = match &*arg.with_parameter_item_group0 {
                WithParameterItemGroup0::ArrayTypeEquExpression(x) => {
                    let r#type: SymType = x.array_type.as_ref().into();
                    let value = ParameterValue::Expression(*x.expression.clone());
                    ParameterProperty {
                        token,
                        r#type,
                        scope,
                        value,
                    }
                }
                WithParameterItemGroup0::TypeEquTypeExpression(x) => {
                    let r#type: SymType = SymType {
                        modifier: vec![],
                        kind: TypeKind::Type,
                        width: vec![],
                        array: vec![],
                    };
                    let value = ParameterValue::TypeExpression(*x.type_expression.clone());
                    ParameterProperty {
                        token,
                        r#type,
                        scope,
                        value,
                    }
                }
            };
            let kind = SymbolKind::Parameter(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let property = match &*arg.port_declaration_item_group {
                PortDeclarationItemGroup::DirectionArrayType(x) => {
                    let r#type: SymType = x.array_type.as_ref().into();
                    let direction: SymDirection = x.direction.as_ref().into();
                    PortProperty {
                        token,
                        r#type: Some(r#type),
                        direction,
                    }
                }
                PortDeclarationItemGroup::InterfacePortDeclarationItemOpt(_) => PortProperty {
                    token,
                    r#type: None,
                    direction: SymDirection::Interface,
                },
            };
            let kind = SymbolKind::Port(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let mut parameters = Vec::new();
                if let Some(ref x) = arg.function_declaration_opt {
                    if let Some(ref x) = x.with_parameter.with_parameter_opt {
                        let items: Vec<WithParameterItem> = x.with_parameter_list.as_ref().into();
                        for item in items {
                            parameters.push((&item).into());
                        }
                    }
                }
                let mut ports = Vec::new();
                if let Some(ref x) = arg.function_declaration_opt0 {
                    if let Some(ref x) = x.port_declaration.port_declaration_opt {
                        let items: Vec<PortDeclarationItem> =
                            x.port_declaration_list.as_ref().into();
                        for item in items {
                            ports.push((&item).into());
                        }
                    }
                }
                let property = FunctionProperty { parameters, ports };
                self.insert_symbol(
                    &arg.identifier.identifier_token,
                    SymbolKind::Function(property),
                );

                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let mut parameters = Vec::new();
                if let Some(ref x) = arg.module_declaration_opt {
                    if let Some(ref x) = x.with_parameter.with_parameter_opt {
                        let items: Vec<WithParameterItem> = x.with_parameter_list.as_ref().into();
                        for item in items {
                            parameters.push((&item).into());
                        }
                    }
                }
                let mut ports = Vec::new();
                if let Some(ref x) = arg.module_declaration_opt0 {
                    if let Some(ref x) = x.port_declaration.port_declaration_opt {
                        let items: Vec<PortDeclarationItem> =
                            x.port_declaration_list.as_ref().into();
                        for item in items {
                            ports.push((&item).into());
                        }
                    }
                }
                let property = ModuleProperty { parameters, ports };
                self.insert_symbol(
                    &arg.identifier.identifier_token,
                    SymbolKind::Module(property),
                );

                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.for_identifier = Some(arg.identifier.identifier_token.clone());
        }
        Ok(())
    }

    fn module_named_block(&mut self, arg: &ModuleNamedBlock) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Block);

                let name = arg.identifier.identifier_token.token.text;
                self.default_block = Some(name);
                self.namespace.push(name);

                if self.for_identifier.is_some() {
                    let identifier = self.for_identifier.clone().unwrap();
                    self.insert_symbol(&identifier, SymbolKind::Genvar);
                    self.for_identifier = None;
                }
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn module_optional_named_block(
        &mut self,
        arg: &ModuleOptionalNamedBlock,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let name = if let Some(ref x) = arg.module_optional_named_block_opt {
                    self.insert_symbol(&x.identifier.identifier_token, SymbolKind::Block);
                    x.identifier.identifier_token.token.text
                } else {
                    let name = format!(
                        "{}@{}",
                        self.default_block.unwrap(),
                        self.anonymous_namespace
                    );
                    self.anonymous_namespace += 1;
                    resource_table::insert_str(&name)
                };

                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let mut parameters = Vec::new();
                if let Some(ref x) = arg.interface_declaration_opt {
                    if let Some(ref x) = x.with_parameter.with_parameter_opt {
                        let items: Vec<WithParameterItem> = x.with_parameter_list.as_ref().into();
                        for item in items {
                            parameters.push((&item).into());
                        }
                    }
                }
                let property = InterfaceProperty { parameters };
                self.insert_symbol(
                    &arg.identifier.identifier_token,
                    SymbolKind::Interface(property),
                );

                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn interface_for_declaration(
        &mut self,
        arg: &InterfaceForDeclaration,
    ) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.for_identifier = Some(arg.identifier.identifier_token.clone());
        }
        Ok(())
    }

    fn interface_named_block(&mut self, arg: &InterfaceNamedBlock) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Block);

                let name = arg.identifier.identifier_token.token.text;
                self.default_block = Some(name);
                self.namespace.push(name);

                if self.for_identifier.is_some() {
                    let identifier = self.for_identifier.clone().unwrap();
                    self.insert_symbol(&identifier, SymbolKind::Genvar);
                    self.for_identifier = None;
                }
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn interface_optional_named_block(
        &mut self,
        arg: &InterfaceOptionalNamedBlock,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let name = if let Some(ref x) = arg.interface_optional_named_block_opt {
                    self.insert_symbol(&x.identifier.identifier_token, SymbolKind::Block);
                    x.identifier.identifier_token.token.text
                } else {
                    let name = format!(
                        "{}@{}",
                        self.default_block.unwrap(),
                        self.anonymous_namespace
                    );
                    self.anonymous_namespace += 1;
                    resource_table::insert_str(&name)
                };

                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Package);

                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }
}
