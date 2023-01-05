use crate::analyze_error::AnalyzeError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::Direction as SymDirection;
use crate::symbol::Type as SymType;
use crate::symbol::{
    FunctionProperty, InstanceProperty, InterfaceProperty, ModuleProperty, ParameterProperty,
    ParameterScope, PortProperty, Symbol, SymbolKind, VariableProperty,
};
use crate::symbol_table;
use veryl_parser::miette::Result;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CreateSymbolTable<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    point: HandlerPoint,
    namespace: Namespace,
    default_block: Option<StrId>,
}

impl<'a> CreateSymbolTable<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    fn insert_symbol(&mut self, token: &VerylToken, kind: SymbolKind) {
        let symbol = Symbol::new(&token.token, kind, &self.namespace);
        if !symbol_table::insert(&token.token, symbol) {
            let text = resource_table::get_str_value(token.token.text).unwrap();
            self.errors
                .push(AnalyzeError::duplicated_identifier(&text, self.text, token));
        }
    }
}

impl<'a> Handler for CreateSymbolTable<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateSymbolTable<'a> {
    fn identifier(&mut self, arg: &Identifier) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let id = arg.identifier_token.token.id;
            let file_path = arg.identifier_token.token.file_path;
            namespace_table::insert(id, file_path, &self.namespace);
        }
        Ok(())
    }

    fn var_declaration(&mut self, arg: &VarDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = (&*arg.r#type).into();
            let property = VariableProperty { r#type };
            let kind = SymbolKind::Variable(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = (&*arg.r#type).into();
            let value = *arg.expression.clone();
            let kind = SymbolKind::Parameter(ParameterProperty {
                r#type,
                scope: ParameterScope::Local,
                value,
            });
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let type_name = arg.identifier0.identifier_token.token.text;
            let property = InstanceProperty { type_name };
            let kind = SymbolKind::Instance(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let scope = match &*arg.with_parameter_item_group {
                WithParameterItemGroup::Parameter(_) => ParameterScope::Global,
                WithParameterItemGroup::Localparam(_) => ParameterScope::Local,
            };
            let r#type: SymType = (&*arg.r#type).into();
            let value = *arg.expression.clone();
            let property = ParameterProperty {
                r#type,
                scope,
                value,
            };
            let kind = SymbolKind::Parameter(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let direction: SymDirection = (&*arg.direction).into();
            let property = PortProperty { direction };
            let kind = SymbolKind::Port(property);
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let mut parameters = Vec::new();
                if let Some(ref x) = arg.function_declaration_opt {
                    if let Some(ref x) = x.with_parameter.with_parameter_opt {
                        let x = &x.with_parameter_list;
                        parameters.push((&*x.with_parameter_item).into());
                        for x in &x.with_parameter_list_list {
                            parameters.push((&*x.with_parameter_item).into());
                        }
                    }
                }
                let mut ports = Vec::new();
                if let Some(ref x) = arg.function_declaration_opt0 {
                    if let Some(ref x) = x.port_declaration.port_declaration_opt {
                        let x = &x.port_declaration_list;
                        ports.push((&*x.port_declaration_item).into());
                        for x in &x.port_declaration_list_list {
                            ports.push((&*x.port_declaration_item).into());
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

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let mut parameters = Vec::new();
                if let Some(ref x) = arg.module_declaration_opt {
                    if let Some(ref x) = x.with_parameter.with_parameter_opt {
                        let x = &x.with_parameter_list;
                        parameters.push((&*x.with_parameter_item).into());
                        for x in &x.with_parameter_list_list {
                            parameters.push((&*x.with_parameter_item).into());
                        }
                    }
                }
                let mut ports = Vec::new();
                if let Some(ref x) = arg.module_declaration_opt0 {
                    if let Some(ref x) = x.port_declaration.port_declaration_opt {
                        let x = &x.port_declaration_list;
                        ports.push((&*x.port_declaration_item).into());
                        for x in &x.port_declaration_list_list {
                            ports.push((&*x.port_declaration_item).into());
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

    fn module_named_block(&mut self, arg: &ModuleNamedBlock) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Block);

                let name = arg.identifier.identifier_token.token.text;
                self.default_block = Some(name);
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn module_optional_named_block(&mut self, arg: &ModuleOptionalNamedBlock) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = if let Some(ref x) = arg.module_optional_named_block_opt {
                    self.insert_symbol(&x.identifier.identifier_token, SymbolKind::Block);
                    x.identifier.identifier_token.token.text
                } else {
                    self.default_block.unwrap()
                };

                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let mut parameters = Vec::new();
                if let Some(ref x) = arg.interface_declaration_opt {
                    if let Some(ref x) = x.with_parameter.with_parameter_opt {
                        let x = &x.with_parameter_list;
                        parameters.push((&*x.with_parameter_item).into());
                        for x in &x.with_parameter_list_list {
                            parameters.push((&*x.with_parameter_item).into());
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

    fn interface_named_block(&mut self, arg: &InterfaceNamedBlock) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(&arg.identifier.identifier_token, SymbolKind::Block);

                let name = arg.identifier.identifier_token.token.text;
                self.default_block = Some(name);
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }

    fn interface_optional_named_block(&mut self, arg: &InterfaceOptionalNamedBlock) -> Result<()> {
        match self.point {
            HandlerPoint::Before => {
                let name = if let Some(ref x) = arg.interface_optional_named_block_opt {
                    self.insert_symbol(&x.identifier.identifier_token, SymbolKind::Block);
                    x.identifier.identifier_token.token.text
                } else {
                    self.default_block.unwrap()
                };

                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }
}
