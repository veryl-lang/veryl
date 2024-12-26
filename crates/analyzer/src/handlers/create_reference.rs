use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{
    Direction, GenericBoundKind, GenericMap, ParameterKind, Port, Symbol, SymbolKind,
};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table::{self, ResolveError, ResolveErrorCause};
use crate::type_dag::{self, Context, DagError};
use std::collections::HashMap;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{is_anonymous_text, Token, TokenRange};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateReference<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    inst_ports: Vec<Port>,
    inst_sv_module: bool,
    is_anonymous_identifier: bool,
    port_direction: Option<Direction>,
    dag_scope_parent: Vec<u32>,
    dag_scope_context: Vec<Context>,
    dag_type_parent: Vec<u32>,
    dag_type_context: Vec<Context>,
    dag_owned: HashMap<u32, Vec<u32>>,
    dag_file_imports: Vec<u32>,
}

impl<'a> CreateReference<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    fn push_resolve_error(
        &mut self,
        err: ResolveError,
        token: &TokenRange,
        generics_token: Option<Token>,
    ) {
        if let Some(last_found) = err.last_found {
            let name = last_found.token.to_string();
            match err.cause {
                ResolveErrorCause::NotFound(not_found) => {
                    let is_generic_if = if let SymbolKind::Port(ref port) = last_found.kind {
                        port.direction == Direction::Interface
                    } else {
                        false
                    };

                    if !is_generic_if {
                        let member = format!("{}", not_found);
                        self.errors.push(AnalyzerError::unknown_member(
                            &name, &member, self.text, token,
                        ));
                    }
                }
                ResolveErrorCause::Private => {
                    self.errors
                        .push(AnalyzerError::private_member(&name, self.text, token));
                }
            }
        } else if let ResolveErrorCause::NotFound(not_found) = err.cause {
            let name = format!("{}", not_found);
            if let Some(generics_token) = generics_token {
                self.errors
                    .push(AnalyzerError::unresolvable_generic_argument(
                        &name,
                        self.text,
                        token,
                        &generics_token.into(),
                    ));
            } else if is_anonymous_text(not_found) {
                self.errors
                    .push(AnalyzerError::anonymous_identifier_usage(self.text, token));
            } else {
                self.errors
                    .push(AnalyzerError::undefined_identifier(&name, self.text, token));
            }
        } else {
            unreachable!();
        }
    }

    fn generic_symbol_path(
        &mut self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
        generics_token: Option<Token>,
    ) {
        if path.is_generic_reference() {
            return;
        }

        let mut path = path.clone();
        path.resolve_imported(namespace);

        for i in 0..path.len() {
            let base_path = path.base_path(i);

            match symbol_table::resolve((&base_path, namespace)) {
                Ok(symbol) => {
                    symbol_table::add_reference(symbol.found.id, &path.paths[0].base);

                    // Check number of arguments
                    let params = symbol.found.generic_parameters();
                    let n_args = path.paths[i].arguments.len();
                    let match_artiy = if params.len() > n_args {
                        params[n_args].1.default_value.is_some()
                    } else {
                        params.len() == n_args
                    };

                    if !match_artiy {
                        self.errors.push(AnalyzerError::mismatch_generics_arity(
                            &path.paths[i].base.to_string(),
                            params.len(),
                            n_args,
                            self.text,
                            &path.range,
                        ));
                        continue;
                    }

                    let generic_args: Vec<_> = path.paths[i]
                        .arguments
                        .iter()
                        .filter_map(|x| {
                            symbol_table::resolve((&x.mangled_path(), namespace))
                                .map(|x| x.found)
                                .ok()
                        })
                        .collect();
                    self.insert_base_path_dag_node(&symbol.found, &generic_args);

                    let mut path = path.paths[i].clone();

                    for param in params.iter().skip(n_args) {
                        //  apply default value
                        path.arguments
                            .push(param.1.default_value.as_ref().unwrap().clone());
                    }

                    if let Some((token, new_symbol)) = path.get_generic_instance(&symbol.found) {
                        if let Some(ref x) = symbol_table::insert(&token, new_symbol) {
                            symbol_table::add_generic_instance(symbol.found.id, *x);
                        }

                        let table = symbol.found.generic_table(&path.arguments);
                        let map = vec![GenericMap {
                            name: "".to_string(),
                            map: table,
                        }];
                        let mut references = symbol.found.generic_references();
                        for path in &mut references {
                            path.apply_map(&map);
                            self.generic_symbol_path(
                                path,
                                &symbol.found.inner_namespace(),
                                Some(symbol.found.token),
                            );
                        }
                    }
                }
                Err(err) => {
                    let single_path = path.paths.len() == 1;
                    if single_path && !path.is_resolvable() {
                        return;
                    }

                    self.push_resolve_error(err, &path.range, generics_token);
                }
            }
        }
    }

    fn insert_declaration_dag_node(&mut self, symbol: &Symbol) -> Option<u32> {
        if let Some(child) = self.insert_dag_node(symbol) {
            if let Some(parent) = self.dag_scope_parent.last().cloned() {
                self.insert_dag_owned(parent, child);
                self.insert_dag_edge(child, parent, *self.dag_scope_context.last().unwrap());
            }
            Some(child)
        } else {
            None
        }
    }

    fn insert_scope_declaration_dag_node(&mut self, symbol: &Symbol, context: Context) {
        if let Some(child) = self.insert_declaration_dag_node(symbol) {
            self.dag_scope_parent.push(child);
            self.dag_scope_context.push(context);

            for import_item in &self.dag_file_imports.clone() {
                self.insert_dag_edge(child, *import_item, context);
            }
        }
    }

    fn pop_scope_dag(&mut self) {
        self.dag_scope_parent.pop();
        self.dag_scope_context.pop();
    }

    fn insert_type_declaration_dag_node(&mut self, symbol: &Symbol, context: Context) {
        if let Some(child) = self.insert_declaration_dag_node(symbol) {
            self.dag_type_parent.push(child);
            self.dag_type_context.push(context);
        }
    }

    fn pop_type_dag(&mut self) {
        self.dag_type_parent.pop();
        self.dag_type_context.pop();
    }

    fn insert_base_path_dag_node(&mut self, base: &Symbol, generic_args: &[Symbol]) {
        if let Some(base) = self.insert_dag_node(base) {
            if let Some(parent) = self.dag_scope_parent.last() {
                if !self.is_dag_owned(*parent, base) {
                    self.insert_dag_edge(*parent, base, *self.dag_scope_context.last().unwrap());
                }
            }
            if let Some(parent) = self.dag_type_parent.last() {
                if !self.is_dag_owned(*parent, base) {
                    self.insert_dag_edge(*parent, base, *self.dag_type_context.last().unwrap());
                }
            }

            for arg in generic_args {
                if let Some(arg) = self.insert_dag_node(arg) {
                    self.insert_dag_edge(base, arg, Context::GenericInstance);
                }
            }
        }
    }

    fn insert_dag_node(&mut self, symbol: &Symbol) -> Option<u32> {
        let is_dag_symbol = match symbol.kind {
            SymbolKind::Module(_)
            | SymbolKind::Interface(_)
            | SymbolKind::Modport(_)
            | SymbolKind::Package(_)
            | SymbolKind::Enum(_)
            | SymbolKind::TypeDef(_)
            | SymbolKind::Struct(_)
            | SymbolKind::Union(_)
            | SymbolKind::Function(_) => true,
            SymbolKind::Parameter(ref x) => matches!(x.kind, ParameterKind::Const),
            _ => false,
        };
        if !is_dag_symbol {
            return None;
        }

        let name = symbol.token.to_string();
        match type_dag::insert_node(symbol.id, &name) {
            Ok(n) => Some(n),
            Err(error) => {
                self.push_cyclic_type_dependency_error(error);
                None
            }
        }
    }

    fn insert_dag_edge(&mut self, start: u32, end: u32, edge: Context) {
        // Reversing this order to make traversal work
        match type_dag::insert_edge(end, start, edge) {
            Ok(_) => {}
            Err(error) => {
                self.push_cyclic_type_dependency_error(error);
            }
        }
    }

    fn insert_dag_owned(&mut self, parent: u32, child: u32) {
        // If there is already edge to owned type, remove it.
        // Argument order should be the same as insert_edge.
        if type_dag::exist_edge(child, parent) {
            type_dag::remove_edge(child, parent);
        }
        self.dag_owned
            .entry(parent)
            .and_modify(|x| x.push(child))
            .or_insert(vec![child]);
    }

    fn is_dag_owned(&self, parent: u32, child: u32) -> bool {
        if let Some(owned) = self.dag_owned.get(&parent) {
            owned.contains(&child)
        } else {
            false
        }
    }

    fn push_cyclic_type_dependency_error(&mut self, error: DagError) {
        let DagError::Cyclic(s, e) = error;
        let start = match resource_table::get_str_value(s.token.text) {
            Some(s) => s,
            None => "<unknown StrId>".into(),
        };
        let end = match resource_table::get_str_value(e.token.text) {
            Some(s) => s,
            None => "<unknown StrId>".into(),
        };
        self.errors.push(AnalyzerError::cyclic_type_dependency(
            self.text,
            &start,
            &end,
            &e.token.into(),
        ));
    }
}

impl Handler for CreateReference<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CreateReference<'_> {
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for id in symbol.full_path {
                        symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                    }
                }
                Err(err) => {
                    // hierarchical identifier is used for:
                    //  - LHS of assign declaratoin
                    //  - identifier to specfy clock/reset in always_ff event list
                    // therefore, it should be known indentifer
                    // and we don't have to consider it is anonymous

                    // TODO check SV-side member to suppress error
                    self.push_resolve_error(err, &arg.into(), None);
                }
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !self.is_anonymous_identifier {
                let ident = arg.identifier().token;
                let path: GenericSymbolPath = arg.into();
                let namespace = namespace_table::get(ident.id).unwrap();

                self.generic_symbol_path(&path, &namespace, None);
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        // Should be executed after scoped_identifier to handle hierarchical access only
        if let HandlerPoint::After = self.point {
            let ident = arg.identifier().token;
            let namespace = namespace_table::get(ident.id).unwrap();
            let mut path: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
            path.resolve_imported(&namespace);
            let mut path = path.mangled_path();

            for x in &arg.expression_identifier_list0 {
                path.push(x.identifier.identifier_token.token.text);

                match symbol_table::resolve((&path, &namespace)) {
                    Ok(symbol) => {
                        symbol_table::add_reference(symbol.found.id, &ident);
                    }
                    Err(err) => {
                        self.push_resolve_error(err, &arg.into(), None);
                    }
                }
            }
        }
        Ok(())
    }

    fn const_declaration(&mut self, arg: &ConstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_type_declaration_dag_node(&symbol.found, Context::Const);
            }
            HandlerPoint::After => self.pop_type_dag(),
        }
        Ok(())
    }

    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_type_declaration_dag_node(&symbol.found, Context::TypeDef);
            }
            HandlerPoint::After => self.pop_type_dag(),
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_type_declaration_dag_node(&symbol.found, Context::Enum);
            }
            HandlerPoint::After => self.pop_type_dag(),
        }
        Ok(())
    }

    fn modport_declaration(&mut self, arg: &ModportDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_type_declaration_dag_node(&symbol.found, Context::Modport);
            }
            HandlerPoint::After => self.pop_type_dag(),
        }
        Ok(())
    }

    fn modport_item(&mut self, arg: &ModportItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg.identifier.as_ref()) {
                Ok(symbol) => {
                    for id in symbol.full_path {
                        symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                    }
                }
                Err(err) => {
                    self.push_resolve_error(err, &arg.identifier.as_ref().into(), None);
                }
            }
        }
        Ok(())
    }

    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                let context = match &*arg.struct_union {
                    StructUnion::Struct(_) => Context::Struct,
                    StructUnion::Union(_) => Context::Union,
                };
                self.insert_type_declaration_dag_node(&symbol.found, context);
            }
            HandlerPoint::After => self.pop_type_dag(),
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                    match symbol.found.kind {
                        SymbolKind::Module(x) => self.inst_ports.extend(x.ports),
                        SymbolKind::GenericParameter(x) => {
                            if let GenericBoundKind::Proto(ref prot) = x.bound {
                                if let SymbolKind::ProtoModule(prot) =
                                    symbol_table::resolve((prot, &symbol.found.namespace))
                                        .unwrap()
                                        .found
                                        .kind
                                {
                                    self.inst_ports.extend(prot.ports);
                                }
                            }
                        }
                        SymbolKind::SystemVerilog => self.inst_sv_module = true,
                        _ => {}
                    }
                }
            }
            HandlerPoint::After => {
                self.inst_ports.clear();
                self.inst_sv_module = false;
            }
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Some(ref x) = arg.inst_port_item_opt {
                    if let Some(port) = self
                        .inst_ports
                        .iter()
                        .find(|x| x.name() == arg.identifier.identifier_token.token.text)
                    {
                        if let SymbolKind::Port(port) = symbol_table::get(port.symbol).unwrap().kind
                        {
                            self.is_anonymous_identifier = port.direction == Direction::Output
                                && is_anonymous_expression(&x.expression);
                        }
                    } else if self.inst_sv_module {
                        // For SV module, any ports can be connected with anonymous identifier
                        self.is_anonymous_identifier = is_anonymous_expression(&x.expression);
                    }
                } else {
                    // implicit port connection by name
                    match symbol_table::resolve(arg.identifier.as_ref()) {
                        Ok(symbol) => {
                            for id in symbol.full_path {
                                symbol_table::add_reference(
                                    id,
                                    &arg.identifier.identifier_token.token,
                                );
                            }
                        }
                        Err(err) => {
                            self.push_resolve_error(err, &arg.identifier.as_ref().into(), None);
                        }
                    }
                }
            }
            HandlerPoint::After => self.is_anonymous_identifier = false,
        }
        Ok(())
    }

    fn port_type_concrete(&mut self, arg: &PortTypeConcrete) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if arg.port_type_concrete_opt0.is_some() {
                    self.port_direction = Some(arg.direction.as_ref().into());
                }
            }
            HandlerPoint::After => self.port_direction = None,
        }
        Ok(())
    }

    /// Semantic action for non-terminal 'PortDefaultValue'
    fn port_default_value(&mut self, arg: &PortDefaultValue) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.is_anonymous_identifier =
                    matches!(self.port_direction.unwrap(), Direction::Output)
                        && is_anonymous_expression(&arg.expression);
            }
            _ => self.is_anonymous_identifier = false,
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_type_declaration_dag_node(&symbol.found, Context::Function);
            }
            HandlerPoint::After => self.pop_type_dag(),
        }
        Ok(())
    }

    fn import_declaration(&mut self, arg: &ImportDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let is_wildcard = arg.import_declaration_opt.is_some();
            match symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                Ok(symbol) => {
                    let symbol = symbol.found;
                    match symbol.kind {
                        SymbolKind::Package(_) if is_wildcard => (),
                        SymbolKind::SystemVerilog => (),
                        _ if is_wildcard => {
                            self.errors.push(AnalyzerError::invalid_import(
                                self.text,
                                &arg.scoped_identifier.as_ref().into(),
                            ));
                        }
                        _ => (),
                    }
                }
                Err(err) => {
                    self.push_resolve_error(err, &arg.scoped_identifier.as_ref().into(), None);
                }
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_scope_declaration_dag_node(&symbol.found, Context::Module);
            }
            HandlerPoint::After => self.pop_scope_dag(),
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_scope_declaration_dag_node(&symbol.found, Context::Interface);
            }
            HandlerPoint::After => self.pop_scope_dag(),
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
                self.insert_scope_declaration_dag_node(&symbol.found, Context::Package);
            }
            HandlerPoint::After => self.pop_scope_dag(),
        }
        Ok(())
    }

    fn veryl(&mut self, arg: &Veryl) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                for x in &arg.veryl_list {
                    let items: Vec<DescriptionItem> = x.description_group.as_ref().into();
                    for item in items {
                        if let DescriptionItem::ImportDeclaration(x) = item {
                            if let Ok(symbol) = symbol_table::resolve(
                                x.import_declaration.scoped_identifier.as_ref(),
                            ) {
                                if let Some(child) = self.insert_dag_node(&symbol.found) {
                                    self.dag_file_imports.push(child);
                                }
                            }
                        }
                    }
                }
            }
            HandlerPoint::After => self.dag_file_imports.clear(),
        }
        Ok(())
    }
}
