use crate::analyze_error::AnalyzeError;
use crate::namespace_table;
use crate::symbol_table::Direction as SymDirection;
use crate::symbol_table::Type as SymType;
use crate::symbol_table::{self, Namespace, ParameterScope, Symbol, SymbolKind};
use veryl_parser::global_table;
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CreateSymbolTable<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    point: HandlerPoint,
    namespace: Namespace,
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
            let text = global_table::get_str_value(token.token.text).unwrap();
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

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = (&*arg.r#type).into();
            let kind = SymbolKind::Variable { r#type };
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = (&*arg.r#type).into();
            let kind = SymbolKind::Parameter {
                r#type,
                scope: ParameterScope::Local,
            };
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let mut name = Vec::new();
            name.push(arg.scoped_identifier.identifier.identifier_token.token.text);
            for x in &arg.scoped_identifier.scoped_identifier_list {
                name.push(x.identifier.identifier_token.token.text);
            }
            let kind = SymbolKind::Instance { name };
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
            let kind = SymbolKind::Parameter { r#type, scope };
            self.insert_symbol(&arg.identifier.identifier_token, kind);
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let direction: SymDirection = (&*arg.direction).into();
            let kind = SymbolKind::Port { direction };
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
                self.insert_symbol(
                    &arg.identifier.identifier_token,
                    SymbolKind::Function { parameters, ports },
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
                self.insert_symbol(
                    &arg.identifier.identifier_token,
                    SymbolKind::Module { parameters, ports },
                );

                let name = arg.identifier.identifier_token.token.text;
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
                self.insert_symbol(
                    &arg.identifier.identifier_token,
                    SymbolKind::Interface { parameters },
                );

                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name)
            }
            HandlerPoint::After => self.namespace.pop(),
        }
        Ok(())
    }
}
