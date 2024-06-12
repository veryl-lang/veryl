use crate::analyzer_error::AnalyzerError;
use crate::symbol::{GenericMap, Symbol, SymbolKind, TypeKind};
use crate::symbol_path::GenericSymbolPath;
use crate::{namespace_table, symbol_table};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct CheckVariableType<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    generic_maps: Vec<GenericMap>,
    in_variable_type: bool,
}

impl<'a> CheckVariableType<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
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
        SymbolKind::GenericInstance(x) => {
            let base = symbol_table::get(x.base).unwrap();
            is_variable_type(&base)
        }
        _ => false,
    }
}

impl<'a> VerylWalker for CheckVariableType<'a> {
    /// Semantic action for non-terminal 'StructUnionDeclaration'
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for map in maps {
            self.generic_maps.push(map.clone());
            self.struct_union_list(&arg.struct_union_list);
            self.generic_maps.pop();
        }
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for map in maps {
            self.generic_maps.push(map.clone());
            if let Some(ref x) = arg.function_declaration_opt0 {
                self.port_declaration(&x.port_declaration);
            }
            if let Some(ref x) = arg.function_declaration_opt1 {
                self.minus_g_t(&x.minus_g_t);
                self.scalar_type(&x.scalar_type);
            }
            for x in &arg.function_declaration_list {
                self.function_item(&x.function_item);
            }
            self.generic_maps.pop();
        }
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for map in maps {
            self.generic_maps.push(map.clone());
            if let Some(ref x) = arg.module_declaration_opt1 {
                self.with_parameter(&x.with_parameter);
            }
            if let Some(ref x) = arg.module_declaration_opt2 {
                self.port_declaration(&x.port_declaration);
            }
            for x in &arg.module_declaration_list {
                self.module_group(&x.module_group);
            }
            self.generic_maps.pop();
        }
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for map in maps {
            self.generic_maps.push(map.clone());
            if let Some(ref x) = arg.interface_declaration_opt1 {
                self.with_parameter(&x.with_parameter);
            }
            for x in &arg.interface_declaration_list {
                self.interface_group(&x.interface_group);
            }
            self.generic_maps.pop();
        }
    }

    /// Semantic action for non-terminal 'PackageDeclaration'
    fn package_declaration(&mut self, arg: &PackageDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for map in maps {
            self.generic_maps.push(map.clone());
            for x in &arg.package_declaration_list {
                self.package_group(&x.package_group);
            }
            self.generic_maps.pop();
        }
    }

    /// Semantic action for non-terminal 'VariableType'
    fn variable_type(&mut self, arg: &VariableType) {
        if let VariableTypeGroup::ScopedIdentifier(x) = &*arg.variable_type_group {
            self.in_variable_type = true;
            self.scoped_identifier(&x.scoped_identifier);
            self.in_variable_type = false;
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        if let PortDeclarationItemGroup::DirectionArrayType(x) = &*arg.port_declaration_item_group {
            let is_modport = matches!(&*x.direction, Direction::Modport(_));
            if !is_modport {
                self.array_type(&x.array_type);
            }
        }
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        if !self.in_variable_type {
            return;
        }

        let namespace = namespace_table::get(arg.identifier().token.id).unwrap();
        let mut path: GenericSymbolPath = arg.into();

        for i in 0..path.len() {
            let base_path = path.base_path(i);
            if let Ok(symbol) = symbol_table::resolve((&base_path, &namespace)) {
                let params = symbol.found.generic_parameters();
                let n_args = path.paths[i].arguments.len();

                for param in params.iter().skip(n_args) {
                    path.paths[i]
                        .arguments
                        .push(param.1.as_ref().unwrap().clone());
                }
            }
        }

        path.apply_map(&self.generic_maps);
        if let Ok(symbol) = symbol_table::resolve((&path.mangled_path(), &namespace)) {
            if !is_variable_type(&symbol.found) {
                self.errors.push(AnalyzerError::mismatch_type(
                    &symbol.found.token.to_string(),
                    "enum or union or struct",
                    &symbol.found.kind.to_kind_name(),
                    self.text,
                    &arg.identifier().token.into(),
                ));
            }
        } else if !path.is_resolvable() {
            let text = path.base_path(0).0[0].to_string();
            self.errors.push(AnalyzerError::mismatch_type(
                &text,
                "enum or union or struct",
                &path.kind.to_string(),
                self.text,
                &arg.identifier().token.into(),
            ));
        }
    }
}
