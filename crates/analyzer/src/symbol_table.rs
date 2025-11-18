use crate::evaluator::{Evaluated, EvaluatedValue};
use crate::namespace::Namespace;
use crate::sv_system_function;
use crate::symbol::{
    ConnectTarget, Direction, DocComment, GenericBoundKind, GenericMap, GenericTable,
    GenericTables, InstanceProperty, Symbol, SymbolId, SymbolKind, TypeKind,
};
use crate::symbol_path::{
    GenericSymbol, GenericSymbolPath, GenericSymbolPathNamesapce, SymbolPath, SymbolPathNamespace,
};
use crate::var_ref::{Assign, VarRef, VarRefAffiliation};
use crate::{AnalyzerError, HashMap, namespace_table};
use log::trace;
use std::cell::RefCell;
use std::fmt;
use veryl_parser::resource_table::{PathId, StrId, TokenId};
use veryl_parser::token_collector::TokenCollector;
use veryl_parser::veryl_grammar_trait::Expression;
use veryl_parser::veryl_token::{Token, TokenSource};
use veryl_parser::veryl_walker::VerylWalker;

#[derive(Clone, Debug)]
pub struct ResolveResult {
    pub found: Symbol,
    pub full_path: Vec<SymbolId>,
    pub imported: bool,
    pub generic_tables: GenericTables,
}

#[derive(Clone, Debug)]
pub struct ResolveError {
    pub last_found: Option<Box<Symbol>>,
    pub cause: ResolveErrorCause,
}

#[derive(Clone, Debug)]
pub enum ResolveErrorCause {
    NotFound(StrId),
    Private,
    Invisible,
}

impl ResolveError {
    pub fn new(last_found: Option<&Symbol>, cause: ResolveErrorCause) -> Self {
        Self {
            last_found: last_found.map(|x| Box::new(x.clone())),
            cause,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: GenericSymbolPathNamesapce,
    pub namespace: Namespace,
    pub wildcard: bool,
}

#[derive(Clone, Debug)]
pub struct Bind {
    pub token: Token,
    pub target: GenericSymbolPathNamesapce,
    pub doc_comment: DocComment,
    pub property: InstanceProperty,
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    name_table: HashMap<StrId, Vec<SymbolId>>,
    symbol_table: HashMap<SymbolId, Symbol>,
    project_local_table: HashMap<StrId, HashMap<StrId, StrId>>,
    var_ref_list: HashMap<VarRefAffiliation, Vec<VarRef>>,
    import_list: Vec<Import>,
    bind_list: Vec<Bind>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut ret = Self::default();

        // add builtin symbols to "" namespace
        let namespace = Namespace::new();

        for func in DEFINED_NAMESPACES {
            let token = Token::new(func, 0, 0, 0, 0, TokenSource::Builtin);
            let symbol = Symbol::new(
                &token,
                SymbolKind::Namespace,
                &namespace,
                true,
                DocComment::default(),
            );
            let _ = ret.insert(&token, symbol);
        }

        sv_system_function::insert_symbols(&mut ret, &namespace);

        ret
    }

    pub fn insert(&mut self, token: &Token, symbol: Symbol) -> Option<SymbolId> {
        let entry = self.name_table.entry(token.text).or_default();
        for id in entry.iter() {
            let item = self.symbol_table.get(id).unwrap();
            let symbol = &symbol.namespace;
            let item = &item.namespace;

            let same_namespace = symbol.paths == item.paths;
            let define_exclusive = symbol.define_context.exclusive(&item.define_context);

            let conflict = same_namespace && !define_exclusive;
            if conflict {
                return None;
            }
        }
        let id = symbol.id;
        entry.push(id);
        self.symbol_table.insert(id, symbol);
        Some(id)
    }

    pub fn get(&self, id: SymbolId) -> Option<Symbol> {
        self.symbol_table.get(&id).cloned()
    }

    pub fn update(&mut self, symbol: Symbol) {
        let id = symbol.id;
        self.symbol_table.insert(id, symbol);
    }

    fn match_nested_generic_instance(&self, context: &ResolveContext, found: &Symbol) -> bool {
        if let Some(last_found) = context.last_found
            && let (SymbolKind::GenericInstance(_), SymbolKind::GenericInstance(_)) =
                (&last_found.kind, &found.kind)
        {
            let namespace = last_found.inner_namespace();
            return namespace.matched(&found.namespace);
        }
        false
    }

    fn trace_type_kind<'a>(
        &self,
        mut context: ResolveContext<'a>,
        kind: &TypeKind,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let TypeKind::UserDefined(x) = kind {
            // Detect infinite loop in trace_type_kind
            if let Some(last_found) = context.last_found
                && x.path.paths.first().unwrap().base.text == last_found.token.text
            {
                return Ok(context);
            }

            let mut path = x.path.clone();
            path.resolve_imported(&context.namespace, None);

            let symbol = self.resolve(
                &path.generic_path(),
                &path.generic_arguments(),
                context.push(),
            )?;
            match &symbol.found.kind {
                SymbolKind::SystemVerilog => context.sv_member = true,
                SymbolKind::Parameter(x) => {
                    if matches!(x.r#type.kind, TypeKind::Type) {
                        let value = x.value.as_ref().unwrap();
                        return self.trace_type_parameter(context, value, &symbol.found);
                    }
                }
                SymbolKind::TypeDef(x) => {
                    context.namespace = symbol.found.namespace;
                    return self.trace_type_kind(context, &x.r#type.kind);
                }
                SymbolKind::ProtoTypeDef(x) => {
                    if let Some(r#type) = &x.r#type {
                        context.namespace = symbol.found.namespace;
                        return self.trace_type_kind(context, &r#type.kind);
                    }
                }
                SymbolKind::GenericParameter(x) => {
                    if matches!(x.bound, GenericBoundKind::Type) {
                        return self.trace_generic_parameter(context.clone(), &symbol.found);
                    }
                }
                _ => (),
            }
            context.namespace = symbol.found.inner_namespace();
            context.last_found_type = Some(symbol.found.id);
            context.inner = true;
            context.generic_tables = symbol.generic_tables;
        } else {
            // assign a new empty namespace becuase
            // factor types and abstruct interface type have no members.
            context.namespace = Namespace::new();
            context.inner = true;
        }
        Ok(context)
    }

    fn trace_type_path<'a>(
        &self,
        mut context: ResolveContext<'a>,
        path: &GenericSymbolPath,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        let symbol = self.resolve(
            &path.generic_path(),
            &path.generic_arguments(),
            context.push(),
        )?;
        context.generic_tables = symbol.generic_tables.clone();

        match symbol.found.kind {
            SymbolKind::AliasModule(x) => self.trace_type_path(context, &x.target),
            SymbolKind::AliasInterface(x) => self.trace_type_path(context, &x.target),
            SymbolKind::AliasPackage(x) => self.trace_type_path(context, &x.target),
            SymbolKind::GenericInstance(_) => self.trace_generic_instance(context, &symbol.found),
            SymbolKind::GenericParameter(_) => self.trace_generic_parameter(context, &symbol.found),
            _ => {
                if matches!(symbol.found.kind, SymbolKind::SystemVerilog) {
                    context.sv_member = true;
                }
                context.namespace = symbol.found.inner_namespace();
                context.last_found_type = Some(symbol.found.id);
                context.inner = true;
                Ok(context)
            }
        }
    }

    fn trace_generic_instance<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Symbol,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let SymbolKind::GenericInstance(x) = &found.kind {
            let base = self.symbol_table.get(&x.base).unwrap();
            context.namespace = base.inner_namespace();
            context.last_found_type = Some(base.id);
            context.inner = true;
            context
                .generic_namespace_map
                .insert(base.token.text, found.token.text);
        }
        Ok(context)
    }

    fn trace_generic_parameter<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Symbol,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let SymbolKind::GenericParameter(x) = &found.kind {
            if let Some(x) = self.resolve_generic_parameter(context.clone(), found) {
                return x;
            }

            let symbol = match &x.bound {
                GenericBoundKind::Type if x.default_value.is_some() => {
                    let path = x.default_value.as_ref().unwrap().generic_path();
                    let mut ctxt = ResolveContext::new(&found.namespace);
                    ctxt.depth = context.depth + 1;
                    &self.resolve(&path, &[], ctxt)?.found
                }
                GenericBoundKind::Inst(proto) => {
                    let mut ctxt = ResolveContext::new(&found.namespace);
                    ctxt.depth = context.depth + 1;
                    &self.resolve(proto, &[], ctxt)?.found
                }
                GenericBoundKind::Proto(proto) => {
                    if let Some(x) = proto.get_user_defined() {
                        let mut ctxt = ResolveContext::new(&found.namespace);
                        ctxt.depth = context.depth + 1;
                        &self.resolve(&x.path.generic_path(), &[], ctxt)?.found
                    } else {
                        found
                    }
                }
                _ => found,
            };

            context.namespace = symbol.inner_namespace();
            context.last_found_type = Some(symbol.id);
            context.inner = true;
        }
        Ok(context)
    }

    fn resolve_generic_parameter<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Symbol,
    ) -> Option<Result<ResolveContext<'a>, ResolveError>> {
        if let SymbolKind::GenericParameter(_) = &found.kind
            && let Some(generic_table) = context.generic_tables.get(&found.namespace)
            && let Some(path) = generic_table.get(&found.token.text)
        {
            let result = self.resolve(
                &path.generic_path(),
                &path.generic_arguments(),
                context.push(),
            );
            if let Ok(symbol) = result {
                context.namespace = symbol.found.inner_namespace();
                context.last_found_type = Some(symbol.found.id);
                context.inner = true;
                return Some(Ok(context));
            } else {
                return result.err().map(Err);
            }
        }

        None
    }

    fn trace_type_parameter<'a>(
        &self,
        mut context: ResolveContext<'a>,
        expression: &Expression,
        found: &Symbol,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let Some(identifier) = expression.unwrap_identifier() {
            let mut ctxt = ResolveContext::new(&found.namespace);
            ctxt.depth = context.depth + 1;

            let symbol = self.resolve(&identifier.into(), &[], ctxt)?;
            match &symbol.found.kind {
                SymbolKind::Parameter(x) => {
                    if matches!(x.r#type.kind, TypeKind::Type) {
                        let value = x.value.as_ref().unwrap();
                        return self.trace_type_parameter(context, value, &symbol.found);
                    }
                }
                SymbolKind::TypeDef(x) => {
                    return self.trace_type_kind(context, &x.r#type.kind);
                }
                SymbolKind::ProtoTypeDef(x) => {
                    if let Some(ref r#type) = x.r#type {
                        return self.trace_type_kind(context, &r#type.kind);
                    }
                }
                SymbolKind::GenericParameter(x) => {
                    if matches!(x.bound, GenericBoundKind::Type)
                        && let Some(x) =
                            self.resolve_generic_parameter(context.clone(), &symbol.found)
                    {
                        return x;
                    }
                }
                _ => {}
            }

            context.namespace = symbol.found.inner_namespace();
            context.last_found_type = Some(symbol.found.id);
            context.inner = true;
            context.generic_tables = symbol.generic_tables;
        }

        Ok(context)
    }

    fn is_public(&self, context: &ResolveContext, found: &Symbol) -> bool {
        match found.kind {
            SymbolKind::Module(_)
            | SymbolKind::ProtoModule(_)
            | SymbolKind::AliasModule(_)
            | SymbolKind::Interface(_)
            | SymbolKind::ProtoInterface(_)
            | SymbolKind::AliasInterface(_)
            | SymbolKind::Package(_)
            | SymbolKind::ProtoPackage(_)
            | SymbolKind::AliasPackage(_) => !context.other_prj || found.public,
            SymbolKind::Namespace => !context.root_prj || found.public,
            _ => true,
        }
    }

    fn is_visible(&self, context: &ResolveContext, found: &Symbol) -> bool {
        if context.last_found.is_none() || matches!(found.kind, SymbolKind::SystemVerilog) {
            return true;
        }

        let last_found = context.last_found.unwrap();
        let last_found_type = context.last_found_type.map(|x| {
            let symbol = self.symbol_table.get(&x).unwrap();
            symbol.kind.clone()
        });
        let via_modport = match &last_found.kind {
            SymbolKind::Port(x) => matches!(x.direction, Direction::Modport | Direction::Interface),
            _ => false,
        };
        let via_interface_instance = match &last_found.kind {
            SymbolKind::Instance(_) => matches!(
                last_found_type,
                Some(SymbolKind::Interface(_))
                    | Some(SymbolKind::ProtoInterface(_))
                    | Some(SymbolKind::AliasInterface(_))
            ),
            SymbolKind::GenericParameter(x) => {
                matches!(&x.bound, GenericBoundKind::Inst(_))
                    && matches!(
                        last_found_type,
                        Some(SymbolKind::Interface(_)) | Some(SymbolKind::ProtoInterface(_))
                    )
            }
            _ => false,
        };
        let via_interface = match &last_found.kind {
            SymbolKind::Interface(_)
            | SymbolKind::ProtoInterface(_)
            | SymbolKind::AliasInterface(_) => true,
            SymbolKind::GenericInstance(_) => {
                matches!(last_found_type, Some(SymbolKind::Interface(_)))
            }
            SymbolKind::GenericParameter(x) => {
                matches!(&x.bound, GenericBoundKind::Proto(_))
                    && matches!(last_found_type, Some(SymbolKind::ProtoInterface(_)))
            }
            _ => false,
        };
        let via_pacakge = match &last_found.kind {
            SymbolKind::Package(_) | SymbolKind::ProtoPackage(_) | SymbolKind::AliasPackage(_) => {
                true
            }
            SymbolKind::GenericInstance(_) => {
                matches!(last_found_type, Some(SymbolKind::Package(_)))
            }
            SymbolKind::GenericParameter(_) => {
                matches!(
                    last_found_type,
                    Some(SymbolKind::ProtoPackage(_)) | Some(SymbolKind::Package(_))
                )
            }
            _ => false,
        };
        let via_enum = match &last_found.kind {
            SymbolKind::Enum(_) => true,
            SymbolKind::TypeDef(_) => matches!(last_found_type, Some(SymbolKind::Enum(_))),
            SymbolKind::ProtoTypeDef(x) => {
                x.r#type.is_some() && matches!(last_found_type, Some(SymbolKind::Enum(_)))
            }
            _ => false,
        };
        let via_namespace = matches!(last_found.kind, SymbolKind::Namespace);

        match &found.kind {
            SymbolKind::Variable(_)
            | SymbolKind::ModportFunctionMember(_)
            | SymbolKind::ModportVariableMember(_) => via_modport || via_interface_instance,
            SymbolKind::StructMember(_) | SymbolKind::UnionMember(_) => matches!(
                last_found.kind,
                SymbolKind::Port(_)
                    | SymbolKind::ModportVariableMember(_)
                    | SymbolKind::Variable(_)
                    | SymbolKind::Parameter(_)
                    | SymbolKind::ProtoConst(_)
                    | SymbolKind::StructMember(_)
                    | SymbolKind::UnionMember(_)
                    | SymbolKind::GenericParameter(_)
            ),
            SymbolKind::Parameter(_)
            | SymbolKind::ProtoConst(_)
            | SymbolKind::TypeDef(_)
            | SymbolKind::ProtoTypeDef(_)
            | SymbolKind::Enum(_)
            | SymbolKind::Struct(_)
            | SymbolKind::Union(_)
            | SymbolKind::AliasModule(_)
            | SymbolKind::ProtoAliasModule(_)
            | SymbolKind::AliasInterface(_)
            | SymbolKind::ProtoAliasInterface(_)
            | SymbolKind::AliasPackage(_)
            | SymbolKind::ProtoAliasPackage(_)
            | SymbolKind::ProtoFunction(_) => via_pacakge,
            SymbolKind::Function(_) => via_modport || via_interface_instance || via_pacakge,
            SymbolKind::EnumMember(_) | SymbolKind::EnumMemberMangled => via_enum,
            SymbolKind::Modport(_) => via_interface || via_interface_instance,
            SymbolKind::GenericInstance(_) => {
                // A generic instance in this context is for generic type or function
                // defined in a packge or for generic component defined in other project
                via_pacakge || via_namespace
            }
            _ => via_namespace,
        }
    }

    fn resolve<'a>(
        &'a self,
        path: &SymbolPath,
        generic_arguments: &[Vec<GenericSymbolPath>],
        mut context: ResolveContext<'a>,
    ) -> Result<ResolveResult, ResolveError> {
        let mut path = path.clone();

        // replace project local name
        let prj = context.namespace.paths[0];
        let path_head = path.0[0];
        if let Some(map) = self.project_local_table.get(&prj) {
            context.root_prj = false;
            if let Some(id) = map.get(&path_head) {
                path.0[0] = *id;
            }
        }

        trace!("symbol_table: {}resolve   '{}'", context.indent(), path);

        let namespace_generic_map = self.get_namespace_generic_map(&context.namespace);
        for (i, name) in path.as_slice().iter().enumerate() {
            let mut max_depth = 0;
            context.found = None;

            let generic_argument = if let (Some(args), Some(map)) =
                (generic_arguments.get(i), &namespace_generic_map)
            {
                // Generic argumetns will be resolved in the namespace of base component.
                // Therefore, generic parameters given as generic arguments should be replaced
                // with thier types.
                // See: https://github.com/veryl-lang/veryl/issues/1714#issuecomment-2967149726
                let args: Vec<_> = args
                    .iter()
                    .map(|arg| {
                        let mut arg = arg.clone();
                        arg.apply_map(map);
                        arg
                    })
                    .collect();
                Some(args)
            } else {
                generic_arguments.get(i).cloned()
            };

            if context.sv_member {
                let token = Token::new(&name.to_string(), 0, 0, 0, 0, TokenSource::External);
                let symbol = Symbol::new(
                    &token,
                    SymbolKind::SystemVerilog,
                    &context.namespace,
                    false,
                    DocComment::default(),
                );
                return Ok(ResolveResult {
                    found: symbol,
                    full_path: context.full_path,
                    imported: context.imported,
                    generic_tables: context.generic_tables,
                });
            }

            if let Some(ids) = self.name_table.get(name) {
                for id in ids {
                    let symbol = self.symbol_table.get(id).unwrap();
                    let (included, imported) = if context.inner {
                        (
                            self.match_nested_generic_instance(&context, symbol)
                                || context.namespace.matched(&symbol.namespace),
                            false,
                        )
                    } else {
                        let imported = symbol
                            .imported
                            .iter()
                            .any(|x| context.namespace.included(&x.namespace));
                        (
                            context.namespace.included(&symbol.namespace) || imported,
                            imported,
                        )
                    };
                    if included && symbol.namespace.depth() >= max_depth {
                        symbol.evaluate();
                        context.found = Some(symbol);
                        context.imported = imported;
                        max_depth = symbol.namespace.depth();
                    }
                }

                if let Some(found) = context.found {
                    if !self.is_public(&context, found) {
                        trace!("symbol_table: {}private '{}'", context.indent(), path);
                        return Err(ResolveError::new(context.found, ResolveErrorCause::Private));
                    } else if !self.is_visible(&context, found) {
                        trace!("symbol_table: {}invisible '{}'", context.indent(), path);
                        return Err(ResolveError::new(
                            context.found,
                            ResolveErrorCause::Invisible,
                        ));
                    }

                    if let Some(x) = generic_argument {
                        context
                            .generic_tables
                            .insert(found.inner_namespace(), found.generic_table(&x));
                    }

                    context.last_found = context.found;
                    context.full_path.push(found.id);

                    trace!(
                        "symbol_table: {}- path    '{}' : {} @ {}",
                        context.indent(),
                        name,
                        found.kind,
                        context.namespace,
                    );

                    if (i + 1) < path.len() {
                        match &found.kind {
                            SymbolKind::Variable(x) => {
                                context = self.trace_type_kind(context, &x.r#type.kind)?;
                            }
                            SymbolKind::StructMember(x) => {
                                context = self.trace_type_kind(context, &x.r#type.kind)?;
                            }
                            SymbolKind::UnionMember(x) => {
                                context = self.trace_type_kind(context, &x.r#type.kind)?;
                            }
                            SymbolKind::Parameter(x) => {
                                if matches!(x.r#type.kind, TypeKind::Type) {
                                    let value = x.value.as_ref().unwrap();
                                    context = self.trace_type_parameter(context, value, found)?;
                                } else {
                                    context = self.trace_type_kind(context, &x.r#type.kind)?;
                                }
                            }
                            SymbolKind::ProtoConst(x) => {
                                context = self.trace_type_kind(context, &x.r#type.kind)?;
                            }
                            SymbolKind::TypeDef(x) => {
                                context = self.trace_type_kind(context, &x.r#type.kind)?;
                            }
                            SymbolKind::ProtoTypeDef(x) => {
                                if let Some(ref r#type) = x.r#type {
                                    context = self.trace_type_kind(context, &r#type.kind)?;
                                }
                            }
                            SymbolKind::Port(x) => {
                                context = self.trace_type_kind(context, &x.r#type.kind)?;
                            }
                            SymbolKind::ModportVariableMember(_) => {
                                let path = SymbolPath::new(&[found.token.text]);
                                context.namespace = found.namespace.clone();
                                context.namespace.pop();
                                let symbol = self.resolve(&path, &[], context.push())?;
                                if let SymbolKind::Variable(x) = &symbol.found.kind {
                                    context = self.trace_type_kind(context, &x.r#type.kind)?;
                                }
                            }
                            SymbolKind::Module(_)
                            | SymbolKind::Interface(_)
                            | SymbolKind::ProtoInterface(_)
                            | SymbolKind::Package(_)
                            | SymbolKind::ProtoPackage(_) => {
                                context.namespace = found.inner_namespace();
                                context.inner = true;
                            }
                            SymbolKind::AliasModule(x) | SymbolKind::ProtoAliasModule(x) => {
                                context = self.trace_type_path(context, &x.target)?;
                            }
                            SymbolKind::AliasInterface(x) | SymbolKind::ProtoAliasInterface(x) => {
                                context = self.trace_type_path(context, &x.target)?;
                            }
                            SymbolKind::AliasPackage(x) | SymbolKind::ProtoAliasPackage(x) => {
                                context = self.trace_type_path(context, &x.target)?;
                            }
                            SymbolKind::Enum(_) | SymbolKind::Namespace => {
                                context.namespace = found.inner_namespace();
                                context.inner = true;
                            }
                            SymbolKind::SystemVerilog => {
                                context.namespace = found.inner_namespace();
                                context.inner = true;
                                context.sv_member = true;
                            }
                            SymbolKind::Instance(x) => {
                                let mut type_name = x.type_name.clone();
                                type_name.resolve_imported(&context.namespace, None);
                                context = self.trace_type_path(context, &type_name)?;
                            }
                            SymbolKind::GenericInstance(_) => {
                                context = self.trace_generic_instance(context, found)?;
                            }
                            SymbolKind::GenericParameter(_) => {
                                context = self.trace_generic_parameter(context, found)?;
                            }
                            // don't trace inner item
                            SymbolKind::Function(_)
                            | SymbolKind::ProtoFunction(_)
                            | SymbolKind::ProtoModule(_)
                            | SymbolKind::Struct(_)
                            | SymbolKind::Union(_)
                            | SymbolKind::Modport(_)
                            | SymbolKind::ModportFunctionMember(_)
                            | SymbolKind::EnumMember(_)
                            | SymbolKind::EnumMemberMangled
                            | SymbolKind::Block
                            | SymbolKind::SystemFunction(_)
                            | SymbolKind::Genvar
                            | SymbolKind::ClockDomain
                            | SymbolKind::Test(_)
                            | SymbolKind::Embed => (),
                        }
                    }
                } else {
                    trace!(
                        "symbol_table: {}not found '{}' @ {}",
                        context.indent(),
                        path,
                        context.namespace
                    );

                    return Err(ResolveError::new(
                        context.last_found,
                        ResolveErrorCause::NotFound(*name),
                    ));
                }
            } else {
                // If symbol is not found, the name is treated as namespace
                context.namespace = Namespace::new();
                context.namespace.push(*name);
                context.inner = true;
                context.other_prj = true;
            }
        }
        if let Some(found) = context.found {
            let mut found = found.clone();

            // replace namespace path to generic version
            let generic_namespace = found.namespace.replace(&context.generic_namespace_map);
            found.namespace = generic_namespace;

            trace!(
                "symbol_table: {}found     '{}' : {}",
                context.indent(),
                path,
                found.kind
            );

            Ok(ResolveResult {
                found,
                full_path: context.full_path,
                imported: context.imported,
                generic_tables: context.generic_tables,
            })
        } else {
            trace!("symbol_table: {}not found '{}'", context.indent(), path);

            let cause = ResolveErrorCause::NotFound(context.namespace.pop().unwrap());
            Err(ResolveError::new(context.last_found, cause))
        }
    }

    fn get_namespace_generic_map(&self, namespace: &Namespace) -> Option<Vec<GenericMap>> {
        if namespace.depth() <= 1 {
            return None;
        }

        let mut namespace = namespace.clone();

        // Remove anonymous blocks
        namespace
            .paths
            .retain(|x| x.to_string().find('@').is_none());

        let path = namespace.pop().map(|x| SymbolPath::new(&[x]))?;
        let context = ResolveContext::new(&namespace);
        let params = self
            .resolve(&path, &[], context)
            .map(|x| self.collect_generic_bounds(&x.found))
            .ok()?;

        if params.is_empty() {
            return None;
        }

        let mut map = GenericTable::default();
        for (key, bound) in params {
            if let GenericBoundKind::Proto(x) = bound {
                let TypeKind::UserDefined(x) = x.kind else {
                    continue;
                };
                if let Some(path) = self.append_project_prefix_to_generic_bound(&x.path, &namespace)
                {
                    map.insert(key, path);
                } else {
                    map.insert(key, x.path);
                }
            }
        }

        let map = GenericMap { id: None, map };
        Some(vec![map])
    }

    fn collect_generic_bounds(&self, symbol: &Symbol) -> Vec<(StrId, GenericBoundKind)> {
        fn get_generic_bound(table: &SymbolTable, id: SymbolId) -> (StrId, GenericBoundKind) {
            let symbol = table.get(id).unwrap();
            let SymbolKind::GenericParameter(x) = symbol.kind else {
                unreachable!();
            };
            (symbol.token.text, x.bound)
        }

        match &symbol.kind {
            SymbolKind::Function(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_bound(self, *x))
                .collect(),
            SymbolKind::Module(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_bound(self, *x))
                .collect(),
            SymbolKind::Interface(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_bound(self, *x))
                .collect(),
            SymbolKind::Package(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_bound(self, *x))
                .collect(),
            SymbolKind::Block => {
                let Some(parent) = symbol.get_parent() else {
                    return Vec::new();
                };
                self.collect_generic_bounds(&parent)
            }
            SymbolKind::Struct(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_bound(self, *x))
                .collect(),
            SymbolKind::Union(x) => x
                .generic_parameters
                .iter()
                .map(|x| get_generic_bound(self, *x))
                .collect(),
            SymbolKind::GenericInstance(x) => {
                let symbol = self.get(x.base).unwrap();
                self.collect_generic_bounds(&symbol)
            }
            _ => Vec::new(),
        }
    }

    fn append_project_prefix_to_generic_bound(
        &self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
    ) -> Option<GenericSymbolPath> {
        let context = ResolveContext::new(namespace);
        let symbol = self.resolve(&path.generic_path(), &[], context).ok()?;
        if !matches!(
            symbol.found.kind,
            SymbolKind::ProtoModule(_)
                | SymbolKind::ProtoInterface(_)
                | SymbolKind::ProtoPackage(_)
        ) {
            return None;
        }

        let project_symbol = self.find_project_symbol(namespace.paths[0])?;
        let project_path = GenericSymbol {
            base: project_symbol.token,
            arguments: vec![],
        };

        let mut path = path.clone();
        path.paths.insert(0, project_path);

        Some(path)
    }

    pub fn get_all(&self) -> Vec<Symbol> {
        let mut ret = Vec::new();
        for symbol in self.symbol_table.values() {
            symbol.evaluate();
            ret.push(symbol.clone());
        }
        ret
    }

    pub fn dump(&self) -> String {
        for symbol in self.symbol_table.values() {
            symbol.evaluate();
        }
        format!("{self}")
    }

    pub fn dump_assign_list(&self) -> String {
        let assign_list = self.get_assign_list();

        let mut ret = "AssignList [\n".to_string();

        let mut path_width = 0;
        let mut pos_width = 0;
        for assign in &assign_list {
            path_width = path_width.max(assign.path.to_string().len());
            pos_width = pos_width.max(assign.position.to_string().len());
        }

        for assign in &assign_list {
            let last_token = assign.position.0.last().unwrap().token();

            ret.push_str(&format!(
                "    {:path_width$} / {:pos_width$} @ {}:{}:{}\n",
                assign.path,
                assign.position,
                last_token.source,
                last_token.line,
                last_token.column,
                path_width = path_width,
                pos_width = pos_width,
            ));
        }
        ret.push(']');
        ret
    }

    pub fn drop(&mut self, file_path: PathId) {
        let drop_list: Vec<_> = self
            .symbol_table
            .iter()
            .filter(|x| x.1.token.source == file_path)
            .map(|x| *x.0)
            .collect();

        for id in &drop_list {
            self.symbol_table.remove(id);
        }

        for (_, symbols) in self.name_table.iter_mut() {
            symbols.retain(|x| !drop_list.contains(x));
        }

        for (_, symbol) in self.symbol_table.iter_mut() {
            symbol.references.retain(|x| x.source != file_path);
        }

        for (affiliation, list) in self.var_ref_list.iter_mut() {
            if affiliation.token().source == file_path {
                list.clear();
            }
        }
    }

    pub fn add_reference(&mut self, target: SymbolId, token: &Token) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.id == target {
                symbol.references.push(token.to_owned());
                break;
            }
        }
    }

    pub fn add_generic_instance(&mut self, target: SymbolId, instance: SymbolId) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.id == target && !symbol.generic_instances.contains(&instance) {
                symbol.generic_instances.push(instance);
                break;
            }
        }
    }

    fn add_imported_item(&mut self, target: TokenId, import: &Import) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.token.id == target {
                symbol.imported.push(import.to_owned());
            }
        }
    }

    fn add_imported_package(&mut self, target: &Namespace, import: &Import) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.namespace.matched(target) {
                symbol.imported.push(import.to_owned());
            }
        }
    }

    pub fn add_import(&mut self, import: Import) {
        self.import_list.push(import);
    }

    pub fn get_imported_symbols(&self) -> Vec<(Import, Symbol)> {
        let mut ret = Vec::new();
        for import in &self.import_list {
            let context = ResolveContext::new(&import.path.1);
            if let Ok(symbol) = self.resolve(&import.path.0.generic_path(), &[], context) {
                ret.push((import.clone(), symbol.found));
            }
        }
        ret
    }

    pub fn apply_import(&mut self, symbols: &[(Import, Symbol)]) {
        for (import, symbol) in symbols {
            if import.wildcard {
                if let Some(pkg) = self.get_package(symbol, false) {
                    let target = pkg.inner_namespace();
                    self.add_imported_package(&target, import);
                }
            } else if !matches!(symbol.kind, SymbolKind::SystemVerilog) {
                self.add_imported_item(symbol.token.id, import);
            }
        }
        self.import_list.clear();
    }

    fn get_package(&self, symbol: &Symbol, include_proto: bool) -> Option<Symbol> {
        match &symbol.kind {
            SymbolKind::Package(_) => return Some(symbol.clone()),
            SymbolKind::ProtoPackage(_) if include_proto => return Some(symbol.clone()),
            SymbolKind::AliasPackage(x) => {
                let context = ResolveContext::new(&symbol.namespace);
                // Use `&[]` instead of `x.target.generic_arguments()`
                // because symbol_table::get is forbidden in apply_import which borrows `&mut self`
                // It is not necessary for apply_import
                if let Ok(symbol) = self.resolve(&x.target.generic_path(), &[], context) {
                    return self.get_package(&symbol.found, include_proto);
                }
            }
            SymbolKind::GenericInstance(x) => {
                let symbol = self.get(x.base).unwrap();
                return self.get_package(&symbol, false);
            }
            SymbolKind::GenericParameter(x) => {
                if let GenericBoundKind::Proto(proto) = &x.bound
                    && let Some(x) = proto.get_user_defined()
                {
                    let context = ResolveContext::new(&symbol.namespace);
                    if let Ok(symbol) = self.resolve(&x.path.generic_path(), &[], context) {
                        return self.get_package(&symbol.found, true);
                    }
                }
            }
            _ => {}
        }

        None
    }

    pub fn add_bind(&mut self, bind: Bind) {
        self.bind_list.push(bind);
    }

    pub fn apply_bind(&mut self) -> Vec<AnalyzerError> {
        let mut errors = Vec::new();

        let bind_list: Vec<Bind> = self.bind_list.drain(0..).collect();
        for bind in bind_list {
            let Ok(target) = self.resolve_generic_path(&bind.target.0, &bind.target.1) else {
                continue;
            };

            let Some(namespace) = self.resolve_bind_target_namespace(&target.found) else {
                continue;
            };
            let symbol = Symbol::new(
                &bind.token,
                SymbolKind::Instance(bind.property.clone()),
                &namespace,
                false,
                bind.doc_comment.clone(),
            );

            if self.insert(&bind.token, symbol).is_some() {
                if let TokenSource::File { path, .. } = bind.token.source {
                    namespace_table::insert(bind.token.id, path, &namespace);

                    for target in bind.property.parameter_connects.values() {
                        Self::update_connect_target_namespace(target, path, &namespace);
                    }
                    for target in bind.property.port_connects.values() {
                        Self::update_connect_target_namespace(target, path, &namespace);
                    }
                }
            } else {
                errors.push(AnalyzerError::duplicated_identifier(
                    &bind.token.to_string(),
                    &bind.token.into(),
                ));
            }
        }

        errors
    }

    fn resolve_generic_path(
        &mut self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
    ) -> Result<ResolveResult, ResolveError> {
        let context = ResolveContext::new(namespace);
        self.resolve(&path.generic_path(), &[], context)
    }

    fn resolve_bind_target_namespace(&mut self, target: &Symbol) -> Option<Namespace> {
        match &target.kind {
            SymbolKind::Module(_) => Some(target.inner_namespace()),
            SymbolKind::AliasModule(x) => {
                let Ok(target) = self.resolve_generic_path(&x.target, &target.namespace) else {
                    return None;
                };
                self.resolve_bind_target_namespace(&target.found)
            }
            SymbolKind::Interface(_) => Some(target.inner_namespace()),
            SymbolKind::AliasInterface(x) => {
                let Ok(target) = self.resolve_generic_path(&x.target, &target.namespace) else {
                    return None;
                };
                self.resolve_bind_target_namespace(&target.found)
            }
            _ => None,
        }
    }

    fn update_connect_target_namespace(
        target: &ConnectTarget,
        path: PathId,
        namespace: &Namespace,
    ) {
        let mut collector = TokenCollector::new(false);
        collector.expression(&target.expression);

        for token in &collector.tokens {
            namespace_table::insert(token.id, path, namespace);
        }
    }

    pub fn get_user_defined(&self) -> Vec<(SymbolId, SymbolId)> {
        let mut resolved = Vec::new();
        for symbol in self.symbol_table.values() {
            if let Some(x) = symbol.kind.get_type()
                && let TypeKind::UserDefined(x) = &x.kind
            {
                let context = ResolveContext::new(&symbol.namespace);
                if let Ok(type_symbol) =
                    self.resolve(&x.path.generic_path(), &x.path.generic_arguments(), context)
                {
                    resolved.push((symbol.id, type_symbol.found.id));
                }
            }
        }
        resolved
    }

    pub fn set_user_defined(&mut self, resolved: Vec<(SymbolId, SymbolId)>) {
        for (id, type_id) in resolved {
            let symbol = self.symbol_table.get_mut(&id).unwrap();
            if let Some(x) = symbol.kind.get_type_mut()
                && let TypeKind::UserDefined(x) = &mut x.kind
            {
                x.symbol = Some(type_id);
            }
        }
    }

    pub fn find_project_symbol(&self, prj: StrId) -> Option<Symbol> {
        for symbol in self.symbol_table.values() {
            if matches!(symbol.kind, SymbolKind::Namespace) && symbol.token.text == prj {
                return Some(symbol.clone());
            }
        }

        None
    }

    pub fn add_project_local(&mut self, prj: StrId, from: StrId, to: StrId) {
        self.project_local_table
            .entry(prj)
            .and_modify(|x| {
                x.insert(from, to);
            })
            .or_insert(HashMap::from_iter([(from, to)]));
    }

    pub fn get_project_local(&self, prj: StrId) -> Option<HashMap<StrId, StrId>> {
        self.project_local_table.get(&prj).cloned()
    }

    pub fn add_var_ref(&mut self, var_ref: &VarRef) {
        self.var_ref_list
            .entry(var_ref.affiliation)
            .and_modify(|x| x.push(var_ref.clone()))
            .or_insert(vec![var_ref.clone()]);
    }

    pub fn get_var_ref_list(&self) -> HashMap<VarRefAffiliation, Vec<VarRef>> {
        self.var_ref_list.clone()
    }

    pub fn get_assign_list(&self) -> Vec<Assign> {
        self.var_ref_list
            .values()
            .flat_map(|l| l.iter().filter(|r| r.is_assign()))
            .map(Assign::new)
            .collect()
    }

    pub fn clear(&mut self) {
        self.clone_from(&Self::new());
    }

    pub fn clear_evaluated_cache(&mut self, path: &Namespace) {
        for x in self.symbol_table.values_mut() {
            if x.namespace.included(path) {
                x.evaluated.borrow_mut().take();
            }
        }
    }

    pub fn push_override(&mut self, id: SymbolId, value: Evaluated) {
        if let Some(x) = self.symbol_table.get_mut(&id) {
            x.overrides.push(value);
        }
    }

    pub fn pop_override(&mut self, id: SymbolId) {
        if let Some(x) = self.symbol_table.get_mut(&id) {
            x.overrides.pop();
        }
    }
}

impl fmt::Display for SymbolTable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "SymbolTable [")?;
        let mut symbol_width = 0;
        let mut namespace_width = 0;
        let mut reference_width = 0;
        let mut import_width = 0;
        let mut vec: Vec<_> = self.name_table.iter().collect();
        vec.sort_by(|x, y| format!("{}", x.0).cmp(&format!("{}", y.0)));
        for (k, v) in &vec {
            symbol_width = symbol_width.max(format!("{k}").len());
            for id in *v {
                let symbol = self.symbol_table.get(id).unwrap();
                namespace_width = namespace_width.max(format!("{}", symbol.namespace).len());
                reference_width = reference_width.max(format!("{}", symbol.references.len()).len());
                import_width = import_width.max(format!("{}", symbol.imported.len()).len());
            }
        }
        for (k, v) in &vec {
            for id in *v {
                let symbol = self.symbol_table.get(id).unwrap();
                let evaluated = if let Some(evaluated) = symbol.evaluated.borrow().as_ref() {
                    match evaluated.value {
                        EvaluatedValue::Unknown => "".to_string(),
                        _ => format!(" ( {evaluated:?} )"),
                    }
                } else {
                    "".to_string()
                };
                writeln!(
                    f,
                    "    {:symbol_width$} @ {:namespace_width$} {{ref: {:reference_width$}, import: {:import_width$}}}: {}{},",
                    k,
                    symbol.namespace,
                    symbol.references.len(),
                    symbol.imported.len(),
                    symbol.kind,
                    evaluated,
                    symbol_width = symbol_width,
                    namespace_width = namespace_width,
                    reference_width = reference_width,
                    import_width = import_width,
                )?;
            }
        }
        writeln!(f, "]")?;

        Ok(())
    }
}

#[derive(Clone)]
struct ResolveContext<'a> {
    found: Option<&'a Symbol>,
    last_found: Option<&'a Symbol>,
    last_found_type: Option<SymbolId>,
    full_path: Vec<SymbolId>,
    namespace: Namespace,
    generic_namespace_map: HashMap<StrId, StrId>,
    generic_tables: GenericTables,
    inner: bool,
    other_prj: bool,
    root_prj: bool,
    sv_member: bool,
    imported: bool,
    depth: usize,
}

impl ResolveContext<'_> {
    fn new(namespace: &Namespace) -> Self {
        Self {
            found: None,
            last_found: None,
            last_found_type: None,
            full_path: vec![],
            namespace: namespace.clone(),
            generic_namespace_map: HashMap::default(),
            generic_tables: GenericTables::default(),
            inner: false,
            other_prj: false,
            root_prj: true,
            sv_member: false,
            imported: false,
            depth: 0,
        }
    }

    fn push(&self) -> Self {
        let mut ret = ResolveContext::new(&self.namespace);
        ret.generic_tables = self.generic_tables.clone();
        ret.depth = self.depth + 1;
        ret
    }

    fn indent(&self) -> String {
        "  ".repeat(self.depth)
    }
}

const DEFINED_NAMESPACES: [&str; 2] = ["$sv", "$std"];

// Refer IEEE Std 1800-2023 Table B.1 - Reserved keywords
// This list must be sorted to enable binary search
const SYSTEMVERILOG_KEYWORDS: [&str; 248] = [
    "accept_on",
    "alias",
    "always",
    "always_comb",
    "always_ff",
    "always_latch",
    "and",
    "assert",
    "assign",
    "assume",
    "automatic",
    "before",
    "begin",
    "bind",
    "bins",
    "binsof",
    "bit",
    "break",
    "buf",
    "bufif0",
    "bufif1",
    "byte",
    "case",
    "casex",
    "casez",
    "cell",
    "chandle",
    "checker",
    "class",
    "clocking",
    "cmos",
    "config",
    "const",
    "constraint",
    "context",
    "continue",
    "cover",
    "covergroup",
    "coverpoint",
    "cross",
    "deassign",
    "default",
    "defparam",
    "design",
    "disable",
    "dist",
    "do",
    "edge",
    "else",
    "end",
    "endcase",
    "endchecker",
    "endclass",
    "endclocking",
    "endconfig",
    "endfunction",
    "endgenerate",
    "endgroup",
    "endinterface",
    "endmodule",
    "endpackage",
    "endprimitive",
    "endprogram",
    "endproperty",
    "endspecify",
    "endsequence",
    "endtable",
    "endtask",
    "enum",
    "event",
    "eventually",
    "expect",
    "export",
    "extends",
    "extern",
    "final",
    "first_match",
    "for",
    "force",
    "foreach",
    "forever",
    "fork",
    "forkjoin",
    "function",
    "generate",
    "genvar",
    "global",
    "highz0",
    "highz1",
    "if",
    "iff",
    "ifnone",
    "ignore_bins",
    "illegal_bins",
    "implements",
    "implies",
    "import",
    "incdir",
    "include",
    "initial",
    "inout",
    "input",
    "inside",
    "instance",
    "int",
    "integer",
    "interconnect",
    "interface",
    "intersect",
    "join",
    "join_any",
    "join_none",
    "large",
    "let",
    "liblist",
    "library",
    "local",
    "localparam",
    "logic",
    "longint",
    "macromodule",
    "matches",
    "medium",
    "modport",
    "module",
    "nand",
    "negedge",
    "nettype",
    "new",
    "nexttime",
    "nmos",
    "nor",
    "noshowcancelled",
    "not",
    "notif0",
    "notif1",
    "null",
    "or",
    "output",
    "package",
    "packed",
    "parameter",
    "pmos",
    "posedge",
    "primitive",
    "priority",
    "program",
    "property",
    "protected",
    "pull0",
    "pull1",
    "pulldown",
    "pullup",
    "pulsestyle_ondetect",
    "pulsestyle_onevent",
    "pure",
    "rand",
    "randc",
    "randcase",
    "randsequence",
    "rcmos",
    "real",
    "realtime",
    "ref",
    "reg",
    "reject_on",
    "release",
    "repeat",
    "restrict",
    "return",
    "rnmos",
    "rpmos",
    "rtran",
    "rtranif0",
    "rtranif1",
    "s_always",
    "s_eventually",
    "s_nexttime",
    "s_until",
    "s_until_with",
    "scalared",
    "sequence",
    "shortint",
    "shortreal",
    "showcancelled",
    "signed",
    "small",
    "soft",
    "solve",
    "specify",
    "specparam",
    "static",
    "string",
    "strong",
    "strong0",
    "strong1",
    "struct",
    "super",
    "supply0",
    "supply1",
    "sync_accept_on",
    "sync_reject_on",
    "table",
    "tagged",
    "task",
    "this",
    "throughout",
    "time",
    "timeprecision",
    "timeunit",
    "tran",
    "tranif0",
    "tranif1",
    "tri",
    "tri0",
    "tri1",
    "triand",
    "trior",
    "trireg",
    "type",
    "typedef",
    "union",
    "unique",
    "unique0",
    "unsigned",
    "until",
    "until_with",
    "untyped",
    "use",
    "uwire",
    "var",
    "vectored",
    "virtual",
    "void",
    "wait",
    "wait_order",
    "wand",
    "weak",
    "weak0",
    "weak1",
    "while",
    "wildcard",
    "wire",
    "with",
    "within",
    "wor",
    "xnor",
    "xor",
];

pub fn is_sv_keyword(s: &str) -> bool {
    SYSTEMVERILOG_KEYWORDS.binary_search(&s).is_ok()
}

thread_local!(static SYMBOL_TABLE: RefCell<SymbolTable> = RefCell::new(SymbolTable::new()));
thread_local!(static SYMBOL_CACHE: RefCell<HashMap<SymbolPathNamespace, ResolveResult>> = RefCell::new(HashMap::default()));

pub fn insert(token: &Token, symbol: Symbol) -> Option<SymbolId> {
    SYMBOL_TABLE.with(|f| f.borrow_mut().insert(token, symbol))
}

pub fn get(id: SymbolId) -> Option<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get(id))
}

pub fn update(symbol: Symbol) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().update(symbol))
}

pub fn resolve<T: Into<SymbolPathNamespace>>(path: T) -> Result<ResolveResult, ResolveError> {
    let path: SymbolPathNamespace = path.into();

    if let Some(x) = SYMBOL_CACHE.with(|f| f.borrow().get(&path).cloned()) {
        Ok(x)
    } else {
        let context = ResolveContext::new(&path.1);
        let ret = SYMBOL_TABLE.with(|f| f.borrow().resolve(&path.0, &[], context));
        if let Ok(x) = &ret {
            SYMBOL_CACHE.with(|f| f.borrow_mut().insert(path, x.clone()));
        }
        ret
    }
}

pub fn get_all() -> Vec<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get_all())
}

pub fn dump() -> String {
    SYMBOL_TABLE.with(|f| f.borrow().dump())
}

pub fn dump_assign_list() -> String {
    SYMBOL_TABLE.with(|f| f.borrow().dump_assign_list())
}

pub fn drop(file_path: PathId) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().drop(file_path))
}

pub fn add_reference(target: SymbolId, token: &Token) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_reference(target, token))
}

pub fn add_generic_instance(target: SymbolId, instance: SymbolId) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_generic_instance(target, instance))
}

pub fn add_import(import: Import) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_import(import))
}

pub fn apply_import() {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    let symbols = SYMBOL_TABLE.with(|f| f.borrow().get_imported_symbols());
    SYMBOL_TABLE.with(|f| f.borrow_mut().apply_import(&symbols));
}

pub fn add_bind(bind: Bind) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_bind(bind))
}

pub fn apply_bind() -> Vec<AnalyzerError> {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().apply_bind())
}

pub fn resolve_user_defined() {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    let resolved = SYMBOL_TABLE.with(|f| f.borrow().get_user_defined());
    SYMBOL_TABLE.with(|f| f.borrow_mut().set_user_defined(resolved));
}

pub fn find_project_symbol(prj: StrId) -> Option<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().find_project_symbol(prj))
}

pub fn add_project_local(prj: StrId, from: StrId, to: StrId) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_project_local(prj, from, to))
}

pub fn get_project_local(prj: StrId) -> Option<HashMap<StrId, StrId>> {
    SYMBOL_TABLE.with(|f| f.borrow().get_project_local(prj))
}

pub fn add_var_ref(var_ref: &VarRef) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_var_ref(var_ref))
}

pub fn get_var_ref_list() -> HashMap<VarRefAffiliation, Vec<VarRef>> {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().get_var_ref_list())
}

pub fn get_assign_list() -> Vec<Assign> {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().get_assign_list())
}

pub fn clear() {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().clear())
}

pub fn clear_evaluated_cache(path: &Namespace) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().clear_evaluated_cache(path))
}

pub fn push_override(id: SymbolId, value: Evaluated) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().push_override(id, value))
}

pub fn pop_override(id: SymbolId) {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().pop_override(id))
}

#[cfg(test)]
mod tests {
    use crate::namespace::Namespace;
    use crate::symbol_table::{ResolveError, ResolveResult, SymbolPath};
    use crate::{Analyzer, symbol_table};
    use veryl_metadata::Metadata;
    use veryl_parser::{Parser, resource_table};

    const CODE: &str = r##"
    module ModuleA #(
        param paramA: u32 = 1,
        param paramB: PackageA::StructA = 1,
    ) (
        portA: input logic<10>,
        portB: modport InterfaceA::modportA,
    ) {
        const localA: u32 = 1;
        const localB: PackageA::StructA = 1;

        type TypeA = PackageA::StructA;

        var memberA: logic;
        var memberB: PackageA::StructA;
        var memberC: TypeA;
        var memberD: $sv::SvTypeA;
        var memberE: PackageA::UnionA;

        inst instA: InterfaceA;
    }

    interface InterfaceA #(
        param paramA: u32 = 1,
        param paramB: PackageA::StructA = 1,
    ) {
        const localA: u32 = 1;
        const localB: PackageA::StructA = 1;

        type TypeA = PackageA::StructA;

        var memberA: logic;
        var memberB: PackageA::StructA;
        var memberC: TypeA;

        modport modportA {
            memberA: input,
            memberB: output,
            memberC: output,
        }
    }

    package PackageA {
        const localA: u32 = 1;

        struct StructA {
            memberA: logic,
            memberB: StructB,
        }

        struct StructB {
            memberA: logic,
        }

        enum EnumA: logic<2> {
            memberA,
        }

        union UnionA {
            memberA: logic<2>,
            memberB: EnumA,
        }
    }
    "##;

    fn parse() {
        let metadata = Metadata::create_default("prj").unwrap();
        let parser = Parser::parse(&CODE, &"").unwrap();
        let analyzer = Analyzer::new(&metadata);
        analyzer.analyze_pass1(&"prj", &"", &parser.veryl);
    }

    #[track_caller]
    fn check_found(result: Result<ResolveResult, ResolveError>, expect: &str) {
        assert_eq!(format!("{}", result.unwrap().found.namespace), expect);
    }

    #[track_caller]
    fn check_not_found(result: Result<ResolveResult, ResolveError>) {
        assert!(result.is_err());
    }

    fn create_path(paths: &[&str]) -> SymbolPath {
        let mut ret = SymbolPath::default();

        for path in paths {
            ret.push(resource_table::insert_str(path));
        }

        ret
    }

    fn create_namespace(paths: &[&str]) -> Namespace {
        let mut ret = Namespace::default();

        for path in paths {
            ret.push(resource_table::insert_str(path));
        }

        ret
    }

    fn resolve(paths: &[&str], namespace: &[&str]) -> Result<ResolveResult, ResolveError> {
        let path = create_path(paths);
        let namespace = create_namespace(namespace);
        symbol_table::resolve((&path, &namespace))
    }

    #[test]
    fn module() {
        parse();

        let symbol = resolve(&["ModuleA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn interface() {
        parse();

        let symbol = resolve(&["InterfaceA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn package() {
        parse();

        let symbol = resolve(&["PackageA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn param() {
        parse();

        let symbol = resolve(&["paramA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["paramA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["paramA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["paramA"], &["PackageA"]);
        check_not_found(symbol);

        let symbol = resolve(&["paramB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["paramB", "memberB"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["paramB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["paramB", "memberB", "memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn local() {
        parse();

        let symbol = resolve(&["localA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["localA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["localA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["localA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");

        let symbol = resolve(&["localB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["localB", "memberB"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["localB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["localB", "memberB", "memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn port() {
        parse();

        let symbol = resolve(&["portA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["portA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["portA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["portA"], &["PackageA"]);
        check_not_found(symbol);
    }

    #[test]
    fn variable() {
        parse();

        let symbol = resolve(&["memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["memberA"], &["PackageA"]);
        check_not_found(symbol);
    }

    #[test]
    fn r#struct() {
        parse();

        let symbol = resolve(&["StructA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn struct_member() {
        parse();

        let symbol = resolve(&["memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberA"], &["PackageA", "StructA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberB", "memberX"], &["ModuleA"]);
        check_not_found(symbol);
    }

    #[test]
    fn r#enum() {
        parse();

        let symbol = resolve(&["EnumA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn enum_member() {
        parse();

        let symbol = resolve(&["EnumA", "memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA::EnumA");
    }

    #[test]
    fn union() {
        parse();

        let symbol = resolve(&["UnionA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn union_member() {
        parse();

        let symbol = resolve(&["memberE"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberE"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberE", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::UnionA");

        let symbol = resolve(&["memberE", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::UnionA");
    }

    #[test]
    fn modport() {
        parse();

        let symbol = resolve(&["portB"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["portB"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["portB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["portB", "memberC"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberC", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberC", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberC", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn typedef() {
        parse();

        let symbol = resolve(&["memberC"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberC"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberC", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberC", "memberX"], &["ModuleA"]);
        check_not_found(symbol);
    }

    #[test]
    fn sv_member() {
        parse();

        let symbol = resolve(&["memberD"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberD"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberD", "memberA"], &["ModuleA"]);
        check_found(symbol, "$sv::SvTypeA");

        let symbol = resolve(&["memberD", "memberA", "memberA", "memberA"], &["ModuleA"]);
        check_found(symbol, "$sv::SvTypeA");
    }

    #[test]
    fn inst() {
        parse();

        let symbol = resolve(&["instA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["instA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["instA", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["instA", "memberB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }
}
