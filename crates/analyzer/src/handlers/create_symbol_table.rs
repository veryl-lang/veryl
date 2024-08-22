use crate::analyzer_error::AnalyzerError;
use crate::attribute::AllowItem;
use crate::attribute::Attribute as Attr;
use crate::attribute_table;
use crate::evaluator::Evaluated;
use crate::evaluator::Evaluator;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol;
use crate::symbol::ClockDomain as SymClockDomain;
use crate::symbol::Direction as SymDirection;
use crate::symbol::Type as SymType;
use crate::symbol::{
    ConnectTarget, DocComment, EnumMemberProperty, EnumMemberValue, EnumProperty, FunctionProperty,
    GenericParameterProperty, InstanceProperty, InterfaceProperty, ModportFunctionMemberProperty,
    ModportProperty, ModportVariableMemberProperty, ModuleProperty, PackageProperty, Parameter,
    ParameterProperty, ParameterScope, ParameterValue, Port, PortProperty, StructMemberProperty,
    StructProperty, Symbol, SymbolId, SymbolKind, TestProperty, TestType, TypeDefProperty,
    TypeKind, UnionMemberProperty, UnionProperty, VariableAffiniation, VariableProperty,
};
use crate::symbol_path::{GenericSymbolPath, SymbolPath};
use crate::symbol_table;
use std::collections::{HashMap, HashSet};
use veryl_metadata::ClockType;
use veryl_metadata::{Build, ResetType};
use veryl_parser::doc_comment_table;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenRange, TokenSource};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateSymbolTable<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    build_opt: Build,
    point: HandlerPoint,
    namespace: Namespace,
    module_namspace_depth: usize,
    default_block: Option<StrId>,
    for_identifier: Option<Token>,
    anonymous_namespace: usize,
    attribute_lines: HashSet<u32>,
    struct_or_union: Option<StructOrUnion>,
    enum_member_prefix: Option<String>,
    enum_member_value: Option<EnumMemberValue>,
    enum_members: Vec<Option<SymbolId>>,
    struct_union_members: Vec<Option<SymbolId>>,
    affiniation: Vec<VariableAffiniation>,
    connect_targets: Vec<ConnectTarget>,
    connects: HashMap<Token, Vec<ConnectTarget>>,
    generic_parameters: Vec<Vec<SymbolId>>,
    parameters: Vec<Vec<Parameter>>,
    ports: Vec<Vec<Port>>,
    needs_default_generic_argument: bool,
    generic_references: Vec<GenericSymbolPath>,
    default_clock_candidates: Vec<SymbolId>,
    defualt_reset_candidates: Vec<SymbolId>,
    modport_member_ids: Vec<SymbolId>,
    function_ids: HashMap<StrId, SymbolId>,
    exist_clock_without_domain: bool,
}

#[derive(Clone)]
enum StructOrUnion {
    InStruct,
    InUnion,
}

impl<'a> CreateSymbolTable<'a> {
    pub fn new(text: &'a str, build_opt: &'a Build) -> Self {
        Self {
            text,
            build_opt: build_opt.clone(),
            ..Default::default()
        }
    }

    fn insert_symbol(&mut self, token: &Token, kind: SymbolKind, public: bool) -> Option<SymbolId> {
        self.insert_symbol_with_type(token, kind, public, None)
    }

    fn insert_symbol_with_type(
        &mut self,
        token: &Token,
        kind: SymbolKind,
        public: bool,
        r#type: Option<symbol::Type>,
    ) -> Option<SymbolId> {
        let line = token.line;
        let doc_comment = if let TokenSource::File(file) = token.source {
            if line == 0 {
                DocComment::default()
            } else if let Some(doc_comment) = doc_comment_table::get(file, line) {
                DocComment(vec![doc_comment])
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
                DocComment(ret)
            }
        } else {
            DocComment::default()
        };
        let mut symbol = Symbol::new(token, kind, &self.namespace, public, doc_comment);

        if attribute_table::contains(token, Attr::Allow(AllowItem::UnusedVariable)) {
            symbol.allow_unused = true;
        }

        symbol.r#type = r#type;
        let id = symbol_table::insert(token, symbol);
        if id.is_none() {
            self.errors.push(AnalyzerError::duplicated_identifier(
                &token.to_string(),
                self.text,
                &token.into(),
            ));
        }
        id
    }

    fn insert_clock_domain(&mut self, clock_domain: &ClockDomain) -> SymClockDomain {
        // '_ is implicit clock domain
        if clock_domain.identifier.identifier_token.to_string() == "_" {
            return SymClockDomain::Implicit;
        }

        let id = if let Ok(symbol) = symbol_table::resolve((
            &clock_domain.identifier.identifier_token.token,
            &self.namespace,
        )) {
            symbol.found.id
        } else {
            let token = &clock_domain.identifier.identifier_token.token;
            let symbol = Symbol::new(
                token,
                SymbolKind::ClockDomain,
                &self.namespace,
                false,
                DocComment::default(),
            );
            symbol_table::insert(token, symbol).unwrap()
        };
        SymClockDomain::Explicit(id)
    }

    fn get_signal_prefix_suffix(&self, kind: TypeKind) -> (Option<String>, Option<String>) {
        match kind {
            TypeKind::Clock => match self.build_opt.clock_type {
                ClockType::PosEdge => {
                    let prefix = self.build_opt.clock_posedge_prefix.clone();
                    let suffix = self.build_opt.clock_posedge_suffix.clone();
                    return (prefix, suffix);
                }
                ClockType::NegEdge => {
                    let prefix = self.build_opt.clock_negedge_prefix.clone();
                    let suffix = self.build_opt.clock_negedge_suffix.clone();
                    return (prefix, suffix);
                }
            },
            TypeKind::Reset => match self.build_opt.reset_type {
                ResetType::AsyncHigh | ResetType::SyncHigh => {
                    let prefix = self.build_opt.reset_high_prefix.clone();
                    let suffix = self.build_opt.reset_high_suffix.clone();
                    return (prefix, suffix);
                }
                ResetType::AsyncLow | ResetType::SyncLow => {
                    let prefix = self.build_opt.reset_low_prefix.clone();
                    let suffix = self.build_opt.reset_low_suffix.clone();
                    return (prefix, suffix);
                }
            },
            _ => {}
        }
        (None, None)
    }

    fn is_default_clock_candidate(&self, kind: SymbolKind) -> bool {
        if *self.affiniation.last().unwrap() != VariableAffiniation::Module
            || self.namespace.depth() != self.module_namspace_depth
        {
            return false;
        }

        match kind {
            SymbolKind::Port(x) => {
                if let Some(clock) = x.r#type.clone() {
                    match clock.kind {
                        TypeKind::Clock | TypeKind::ClockPosedge | TypeKind::ClockNegedge => {
                            clock.array.is_empty() && clock.width.is_empty()
                        }
                        _ => false,
                    }
                } else {
                    false
                }
            }
            SymbolKind::Variable(x) => {
                let clock = &x.r#type;
                match clock.kind {
                    TypeKind::Clock | TypeKind::ClockPosedge | TypeKind::ClockNegedge => {
                        clock.array.is_empty() && clock.width.is_empty()
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn is_default_reset_candidate(&self, kind: SymbolKind) -> bool {
        if *self.affiniation.last().unwrap() != VariableAffiniation::Module
            || self.namespace.depth() != self.module_namspace_depth
        {
            return false;
        }

        match kind {
            SymbolKind::Port(x) => {
                if let Some(reset) = x.r#type.clone() {
                    match reset.kind {
                        TypeKind::Reset
                        | TypeKind::ResetAsyncHigh
                        | TypeKind::ResetAsyncLow
                        | TypeKind::ResetSyncHigh
                        | TypeKind::ResetSyncLow => {
                            reset.array.is_empty() && reset.width.is_empty()
                        }
                        _ => false,
                    }
                } else {
                    false
                }
            }
            SymbolKind::Variable(x) => {
                let reset = &x.r#type;
                match reset.kind {
                    TypeKind::Reset
                    | TypeKind::ResetAsyncHigh
                    | TypeKind::ResetAsyncLow
                    | TypeKind::ResetSyncHigh
                    | TypeKind::ResetSyncLow => reset.array.is_empty() && reset.width.is_empty(),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn check_missing_clock_domain(&mut self, token: &Token, r#type: &SymType) {
        if r#type.kind.is_clock() {
            if self.exist_clock_without_domain {
                self.errors.push(AnalyzerError::missing_clock_domain(
                    self.text,
                    &token.into(),
                ));
            }
            self.exist_clock_without_domain = true;
        }
    }
}

impl<'a> Handler for CreateSymbolTable<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn scoped_identifier_tokens(arg: &ScopedIdentifier) -> Vec<Token> {
    let mut ret = Vec::new();
    ret.push(arg.identifier().token);
    for x in &arg.scoped_identifier_list {
        ret.push(x.identifier.identifier_token.token);
    }
    ret
}

impl<'a> VerylGrammarTrait for CreateSymbolTable<'a> {
    fn identifier(&mut self, arg: &Identifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.identifier_token.token.id;
            if let TokenSource::File(file) = arg.identifier_token.token.source {
                namespace_table::insert(id, file, &self.namespace);
            }
        }
        Ok(())
    }

    fn dollar_identifier(&mut self, arg: &DollarIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.dollar_identifier_token.token.id;
            if let TokenSource::File(file) = arg.dollar_identifier_token.token.source {
                namespace_table::insert(id, file, &self.namespace);
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                // Add symbols under $sv namespace
                if let ScopedIdentifierGroup::DollarIdentifier(x) =
                    arg.scoped_identifier_group.as_ref()
                {
                    if x.dollar_identifier.dollar_identifier_token.to_string() == "$sv" {
                        let mut namespace = Namespace::new();
                        for (i, token) in scoped_identifier_tokens(arg).iter().enumerate() {
                            if i != 0 {
                                let symbol = Symbol::new(
                                    token,
                                    SymbolKind::SystemVerilog,
                                    &namespace,
                                    false,
                                    DocComment::default(),
                                );
                                let _ = symbol_table::insert(token, symbol);
                            }
                            namespace.push(token.text);
                        }
                    }
                }
            }
            HandlerPoint::After => {
                let path: GenericSymbolPath = arg.into();
                if path.is_generic_reference() {
                    self.generic_references.push(path);
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        // This should be `After` not `Before`.
        // because namespace_table insertion of identifiers
        // in the expression_identifier should be done until `arg.into()`.
        if let HandlerPoint::After = self.point {
            self.connect_targets.push(arg.into());
        }
        Ok(())
    }

    fn attribute(&mut self, arg: &Attribute) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.attribute_lines.insert(arg.hash.hash_token.token.line);
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut r#type: SymType = arg.array_type.as_ref().into();
            r#type.is_const = true;
            let affiniation = self.affiniation.last().cloned().unwrap();
            let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
            let clock_domain = if let Some(ref x) = arg.let_statement_opt {
                self.insert_clock_domain(&x.clock_domain)
            } else if affiniation == VariableAffiniation::Module {
                self.check_missing_clock_domain(&arg.identifier.identifier_token.token, &r#type);
                SymClockDomain::Implicit
            } else {
                SymClockDomain::None
            };
            let property = VariableProperty {
                r#type,
                affiniation,
                prefix,
                suffix,
                clock_domain,
            };
            let kind = SymbolKind::Variable(property);

            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind.clone(), false)
            {
                if self.is_default_clock_candidate(kind.clone()) {
                    self.default_clock_candidates.push(id);
                } else if self.is_default_reset_candidate(kind.clone()) {
                    self.defualt_reset_candidates.push(id);
                }
            }
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
                let affiniation = self.affiniation.last().cloned().unwrap();
                let property = VariableProperty {
                    r#type,
                    affiniation,
                    prefix: None,
                    suffix: None,
                    clock_domain: SymClockDomain::None,
                };
                let kind = SymbolKind::Variable(property);
                self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
            }
            HandlerPoint::After => {
                self.namespace.pop();
            }
        }
        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut r#type: SymType = arg.array_type.as_ref().into();
            r#type.is_const = true;
            let affiniation = self.affiniation.last().cloned().unwrap();
            let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
            let clock_domain = if let Some(ref x) = arg.let_declaration_opt {
                self.insert_clock_domain(&x.clock_domain)
            } else if affiniation == VariableAffiniation::Module {
                self.check_missing_clock_domain(&arg.identifier.identifier_token.token, &r#type);
                SymClockDomain::Implicit
            } else {
                SymClockDomain::None
            };
            let property = VariableProperty {
                r#type,
                affiniation,
                prefix,
                suffix,
                clock_domain,
            };
            let kind = SymbolKind::Variable(property);

            if let Some(id) = self.insert_symbol_with_type(
                &arg.identifier.identifier_token.token,
                kind.clone(),
                false,
                None,
            ) {
                if self.is_default_clock_candidate(kind.clone()) {
                    self.default_clock_candidates.push(id);
                } else if self.is_default_reset_candidate(kind.clone()) {
                    self.defualt_reset_candidates.push(id);
                }
            }
        }
        Ok(())
    }

    fn var_declaration(&mut self, arg: &VarDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = arg.array_type.as_ref().into();
            let affiniation = self.affiniation.last().cloned().unwrap();
            let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
            let clock_domain = if let Some(ref x) = arg.var_declaration_opt {
                self.insert_clock_domain(&x.clock_domain)
            } else if affiniation == VariableAffiniation::Module {
                self.check_missing_clock_domain(&arg.identifier.identifier_token.token, &r#type);
                SymClockDomain::Implicit
            } else {
                SymClockDomain::None
            };
            let property = VariableProperty {
                r#type,
                affiniation,
                prefix,
                suffix,
                clock_domain,
            };
            let kind = SymbolKind::Variable(property);

            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind.clone(), false)
            {
                if self.is_default_clock_candidate(kind.clone()) {
                    self.default_clock_candidates.push(id);
                } else if self.is_default_reset_candidate(kind.clone()) {
                    self.defualt_reset_candidates.push(id);
                }
            }
        }
        Ok(())
    }

    fn local_declaration(&mut self, arg: &LocalDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let property = match &*arg.local_declaration_group {
                LocalDeclarationGroup::ArrayTypeEquExpression(x) => {
                    let r#type: SymType = x.array_type.as_ref().into();
                    let value = ParameterValue::Expression(*x.expression.clone());
                    ParameterProperty {
                        token,
                        r#type,
                        scope: ParameterScope::Local,
                        value,
                    }
                }
                LocalDeclarationGroup::TypeEquTypeExpression(x) => {
                    let r#type: SymType = SymType {
                        modifier: vec![],
                        kind: TypeKind::Type,
                        width: vec![],
                        array: vec![],
                        is_const: false,
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
            self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
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
                let kind = match &*item.direction {
                    Direction::Ref(_) | Direction::Modport(_) => {
                        continue;
                    }
                    Direction::Import(_) => {
                        let property = ModportFunctionMemberProperty {
                            function: SymbolId::default(),
                        };
                        SymbolKind::ModportFunctionMember(property)
                    }
                    _ => {
                        let direction: crate::symbol::Direction = item.direction.as_ref().into();
                        let property = ModportVariableMemberProperty { direction };
                        SymbolKind::ModportVariableMember(property)
                    }
                };

                if let Some(id) =
                    self.insert_symbol(&item.identifier.identifier_token.token, kind, false)
                {
                    members.push(id);
                    self.modport_member_ids.push(id);
                }
            }

            self.namespace.pop();

            let property = ModportProperty { members };
            let kind = SymbolKind::Modport(property);
            self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name);

                // default prefix
                self.enum_member_prefix = Some(arg.identifier.identifier_token.to_string());

                // reset enum value
                self.enum_member_value = None;

                // overridden prefix by attribute
                let attrs = attribute_table::get(&arg.r#enum.enum_token.token);
                for attr in attrs {
                    if let Attr::EnumMemberPrefix(x) = attr {
                        self.enum_member_prefix = Some(x.to_string());
                    }
                }
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.enum_member_prefix = None;

                let members: Vec<_> = self.enum_members.drain(0..).flatten().collect();
                let r#type = arg
                    .enum_declaration_opt
                    .as_ref()
                    .map(|x| x.scalar_type.as_ref().into());
                let property = EnumProperty { r#type, members };
                let kind = SymbolKind::Enum(property);
                self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
            }
        }
        Ok(())
    }

    fn enum_item(&mut self, arg: &EnumItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let value = if let Some(ref x) = arg.enum_item_opt {
                let mut evaluator = Evaluator::new();
                let evaluated = evaluator.expression(&x.expression);
                if let Evaluated::Fixed { value, .. } = evaluated {
                    EnumMemberValue::ExplicitValue(*x.expression.clone(), Some(value as usize))
                } else {
                    EnumMemberValue::ExplicitValue(*x.expression.clone(), None)
                }
            } else if let Some(ref x) = self.enum_member_value {
                let previous_value = match x {
                    EnumMemberValue::ExplicitValue(_expression, evaluated) => {
                        if let Some(value) = evaluated {
                            value
                        } else {
                            let name = arg.identifier.identifier_token.to_string();
                            self.errors.push(AnalyzerError::unevaluatable_enum_variant(
                                &name,
                                self.text,
                                &arg.identifier.as_ref().into(),
                            ));
                            return Ok(());
                        }
                    }
                    EnumMemberValue::ImplicitValue(value) => value,
                };
                EnumMemberValue::ImplicitValue(previous_value + 1)
            } else {
                EnumMemberValue::ImplicitValue(0)
            };
            let prefix = self.enum_member_prefix.clone().unwrap();
            let property = EnumMemberProperty {
                value: value.clone(),
                prefix,
            };
            let kind = SymbolKind::EnumMember(property);
            let id = self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
            self.enum_members.push(id);

            // add EnumMemberMangled to detect identifier conflict in generated SV
            let mut token = arg.identifier.identifier_token.token;
            let prefix = self.enum_member_prefix.clone().unwrap();
            token.text = resource_table::insert_str(&format!("{prefix}_{}", token.text));
            let kind = SymbolKind::EnumMemberMangled;

            // namespace of EnumMemberMangled is outside of enum
            let namespace = self.namespace.pop();
            self.insert_symbol(&token, kind, false);
            if let Some(namespace) = namespace {
                self.namespace.push(namespace);
            }

            self.enum_member_value = Some(value);
        }
        Ok(())
    }

    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;

        match self.point {
            HandlerPoint::Before => {
                self.struct_or_union = match &*arg.struct_union {
                    StructUnion::Struct(_) => Some(StructOrUnion::InStruct),
                    StructUnion::Union(_) => Some(StructOrUnion::InUnion),
                };

                self.generic_parameters.push(Vec::new());
                self.namespace.push(name);
            }
            HandlerPoint::After => {
                self.struct_or_union = None;
                self.namespace.pop();

                let generic_parameters: Vec<_> = self.generic_parameters.pop().unwrap();
                let generic_references: Vec<_> = self.generic_references.drain(..).collect();

                let members: Vec<_> = self.struct_union_members.drain(0..).flatten().collect();
                let kind = match &*arg.struct_union {
                    StructUnion::Struct(_) => {
                        let property = StructProperty {
                            members,
                            generic_parameters,
                            generic_references,
                        };
                        SymbolKind::Struct(property)
                    }
                    StructUnion::Union(_) => {
                        let property = UnionProperty {
                            members,
                            generic_parameters,
                            generic_references,
                        };
                        SymbolKind::Union(property)
                    }
                };
                self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
            }
        }
        Ok(())
    }

    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type = arg.array_type.as_ref().into();
            let property = TypeDefProperty { r#type };
            let kind = SymbolKind::TypeDef(property);
            self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
        }
        Ok(())
    }

    fn struct_union_item(&mut self, arg: &StructUnionItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = arg.scalar_type.as_ref().into();
            let kind = match self.struct_or_union.clone().unwrap() {
                StructOrUnion::InStruct => {
                    let property = StructMemberProperty { r#type };
                    SymbolKind::StructMember(property)
                }
                StructOrUnion::InUnion => {
                    let property = UnionMemberProperty { r#type };
                    SymbolKind::UnionMember(property)
                }
            };
            let id = self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
            self.struct_union_members.push(id);
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let type_name: SymbolPath = arg.scoped_identifier.as_ref().into();
            let type_name = type_name.to_vec();
            let connects = self.connects.drain().collect();
            let property = InstanceProperty {
                type_name,
                connects,
            };
            let kind = SymbolKind::Instance(property);
            self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.connect_targets.clear(),
            HandlerPoint::After => {
                let port = arg.identifier.identifier_token.token;
                let targets = if arg.inst_port_item_opt.is_some() {
                    self.connect_targets.drain(0..).collect()
                } else {
                    let target = ConnectTarget {
                        path: vec![(port.text, vec![])],
                    };
                    vec![target]
                };
                self.connects.insert(port, targets);
            }
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let scope = match &*arg.with_parameter_item_group {
                WithParameterItemGroup::Param(_) => ParameterScope::Global,
                WithParameterItemGroup::Local(_) => ParameterScope::Local,
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
                        is_const: false,
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
            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
            {
                let parameter = Parameter {
                    name: arg.identifier.identifier_token.token.text,
                    symbol: id,
                };
                self.parameters.last_mut().unwrap().push(parameter);
            }
        }
        Ok(())
    }

    fn with_generic_parameter_list(
        &mut self,
        _arg: &WithGenericParameterList,
    ) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.needs_default_generic_argument = false;
        }
        Ok(())
    }

    fn with_generic_parameter_item(
        &mut self,
        arg: &WithGenericParameterItem,
    ) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let default_value: Option<GenericSymbolPath> =
                if let Some(ref x) = arg.with_generic_parameter_item_opt {
                    self.needs_default_generic_argument = true;
                    match &*x.with_generic_argument_item {
                        WithGenericArgumentItem::ScopedIdentifier(x) => {
                            Some(x.scoped_identifier.as_ref().into())
                        }
                        WithGenericArgumentItem::Number(x) => Some(x.number.as_ref().into()),
                    }
                } else {
                    None
                };

            if !self.needs_default_generic_argument || default_value.is_some() {
                let property = GenericParameterProperty { default_value };
                let kind = SymbolKind::GenericParameter(property);
                if let Some(id) =
                    self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
                {
                    self.generic_parameters.last_mut().unwrap().push(id);
                }
            } else {
                self.errors.push(AnalyzerError::missing_default_argument(
                    &arg.identifier.identifier_token.token.to_string(),
                    self.text,
                    &arg.identifier.as_ref().into(),
                ));
            }
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let affiniation = self.affiniation.last().cloned().unwrap();
            let property = match &*arg.port_declaration_item_group {
                PortDeclarationItemGroup::PortTypeConcrete(x) => {
                    let x = x.port_type_concrete.as_ref();
                    let r#type: SymType = x.array_type.as_ref().into();
                    let direction: SymDirection = x.direction.as_ref().into();
                    let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
                    let clock_domain = if let Some(ref x) = x.port_type_concrete_opt {
                        self.insert_clock_domain(&x.clock_domain)
                    } else if affiniation == VariableAffiniation::Module {
                        self.check_missing_clock_domain(
                            &arg.identifier.identifier_token.token,
                            &r#type,
                        );
                        SymClockDomain::Implicit
                    } else {
                        SymClockDomain::None
                    };
                    PortProperty {
                        token,
                        r#type: Some(r#type),
                        direction,
                        prefix,
                        suffix,
                        clock_domain,
                    }
                }
                PortDeclarationItemGroup::PortTypeAbstract(x) => {
                    let x = &x.port_type_abstract;
                    let clock_domain = if let Some(ref x) = x.port_type_abstract_opt {
                        self.insert_clock_domain(&x.clock_domain)
                    } else if affiniation == VariableAffiniation::Module {
                        SymClockDomain::Implicit
                    } else {
                        SymClockDomain::None
                    };
                    PortProperty {
                        token,
                        r#type: None,
                        direction: SymDirection::Interface,
                        prefix: None,
                        suffix: None,
                        clock_domain,
                    }
                }
            };
            let kind = SymbolKind::Port(property);

            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind.clone(), false)
            {
                let port = Port {
                    name: arg.identifier.identifier_token.token.text,
                    symbol: id,
                };
                self.ports.last_mut().unwrap().push(port);
                if self.is_default_clock_candidate(kind.clone()) {
                    self.default_clock_candidates.push(id);
                } else if self.is_default_reset_candidate(kind.clone()) {
                    self.defualt_reset_candidates.push(id);
                }
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;
        match self.point {
            HandlerPoint::Before => {
                self.namespace.push(name);
                self.generic_parameters.push(Vec::new());
                self.ports.push(Vec::new());
                self.affiniation.push(VariableAffiniation::Function);
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiniation.pop();

                let generic_parameters: Vec<_> = self.generic_parameters.pop().unwrap();
                let generic_references: Vec<_> = self.generic_references.drain(..).collect();
                let ports: Vec<_> = self.ports.pop().unwrap();

                let ret = arg
                    .function_declaration_opt1
                    .as_ref()
                    .map(|x| (&*x.scalar_type).into());

                let range =
                    TokenRange::new(&arg.function.function_token, &arg.r_brace.r_brace_token);

                let property = FunctionProperty {
                    range,
                    generic_parameters,
                    generic_references,
                    ports,
                    ret,
                };

                if let Some(id) = self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Function(property),
                    false,
                ) {
                    self.function_ids
                        .insert(arg.identifier.identifier_token.token.text, id);
                }
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;
        match self.point {
            HandlerPoint::Before => {
                self.namespace.push(name);
                self.generic_parameters.push(Vec::new());
                self.parameters.push(Vec::new());
                self.ports.push(Vec::new());
                self.affiniation.push(VariableAffiniation::Module);
                self.module_namspace_depth = self.namespace.depth();
                self.function_ids.clear();
                self.exist_clock_without_domain = false;
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiniation.pop();
                self.module_namspace_depth = 0;

                let generic_parameters: Vec<_> = self.generic_parameters.pop().unwrap();
                let generic_references: Vec<_> = self.generic_references.drain(..).collect();
                let parameters: Vec<_> = self.parameters.pop().unwrap();
                let ports: Vec<_> = self.ports.pop().unwrap();

                let default_clock = if self.default_clock_candidates.len() == 1 {
                    Some(self.default_clock_candidates[0])
                } else {
                    None
                };
                let default_reset = if self.defualt_reset_candidates.len() == 1 {
                    Some(self.defualt_reset_candidates[0])
                } else {
                    None
                };

                self.default_clock_candidates.clear();
                self.defualt_reset_candidates.clear();

                let range = TokenRange::new(&arg.module.module_token, &arg.r_brace.r_brace_token);

                let property = ModuleProperty {
                    range,
                    generic_parameters,
                    generic_references,
                    parameters,
                    ports,
                    default_clock,
                    default_reset,
                };
                let public = arg.module_declaration_opt.is_some();
                self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Module(property),
                    public,
                );
            }
        }
        Ok(())
    }

    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.for_identifier = Some(arg.identifier.identifier_token.token);
        }
        Ok(())
    }

    fn module_named_block(&mut self, arg: &ModuleNamedBlock) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Block,
                    false,
                );

                let name = arg.identifier.identifier_token.token.text;
                self.default_block = Some(name);
                self.namespace.push(name);

                if self.for_identifier.is_some() {
                    let identifier = self.for_identifier.unwrap();
                    self.insert_symbol(&identifier, SymbolKind::Genvar, false);
                    self.for_identifier = None;
                }
            }
            HandlerPoint::After => {
                self.namespace.pop();
            }
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
                    self.insert_symbol(
                        &x.identifier.identifier_token.token,
                        SymbolKind::Block,
                        false,
                    );
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
            HandlerPoint::After => {
                self.namespace.pop();
            }
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;
        match self.point {
            HandlerPoint::Before => {
                self.namespace.push(name);
                self.generic_parameters.push(Vec::new());
                self.parameters.push(Vec::new());
                self.affiniation.push(VariableAffiniation::Intarface);
                self.function_ids.clear();
                self.modport_member_ids.clear();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiniation.pop();

                let generic_parameters: Vec<_> = self.generic_parameters.pop().unwrap();
                let generic_references: Vec<_> = self.generic_references.drain(..).collect();
                let parameters: Vec<_> = self.parameters.pop().unwrap();

                let range =
                    TokenRange::new(&arg.interface.interface_token, &arg.r_brace.r_brace_token);

                let property = InterfaceProperty {
                    range,
                    generic_parameters,
                    generic_references,
                    parameters,
                };
                let public = arg.interface_declaration_opt.is_some();
                self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Interface(property),
                    public,
                );

                //  link modport function
                for id in &self.modport_member_ids {
                    let mut mp_member = symbol_table::get(*id).unwrap();
                    if let SymbolKind::ModportFunctionMember(_) = mp_member.kind {
                        if let Some(id) = self.function_ids.get(&mp_member.token.text) {
                            let property = ModportFunctionMemberProperty { function: *id };
                            let kind = SymbolKind::ModportFunctionMember(property);
                            mp_member.kind = kind;
                            symbol_table::update(mp_member);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn interface_for_declaration(
        &mut self,
        arg: &InterfaceForDeclaration,
    ) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.for_identifier = Some(arg.identifier.identifier_token.token);
        }
        Ok(())
    }

    fn interface_named_block(&mut self, arg: &InterfaceNamedBlock) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Block,
                    false,
                );

                let name = arg.identifier.identifier_token.token.text;
                self.default_block = Some(name);
                self.namespace.push(name);

                if self.for_identifier.is_some() {
                    let identifier = self.for_identifier.unwrap();
                    self.insert_symbol(&identifier, SymbolKind::Genvar, false);
                    self.for_identifier = None;
                }
            }
            HandlerPoint::After => {
                self.namespace.pop();
            }
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
                    self.insert_symbol(
                        &x.identifier.identifier_token.token,
                        SymbolKind::Block,
                        false,
                    );
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
            HandlerPoint::After => {
                self.namespace.pop();
            }
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;
        match self.point {
            HandlerPoint::Before => {
                self.namespace.push(name);
                self.generic_parameters.push(Vec::new());
                self.affiniation.push(VariableAffiniation::Package);
                self.function_ids.clear();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiniation.pop();

                let generic_parameters: Vec<_> = self.generic_parameters.pop().unwrap();
                let generic_references: Vec<_> = self.generic_references.drain(..).collect();

                let range = TokenRange::new(&arg.package.package_token, &arg.r_brace.r_brace_token);

                let property = PackageProperty {
                    range,
                    generic_parameters,
                    generic_references,
                };
                let public = arg.package_declaration_opt.is_some();
                self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Package(property),
                    public,
                );
            }
        }
        Ok(())
    }

    fn embed_declaration(&mut self, arg: &EmbedDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let way = arg.identifier.identifier_token.to_string();
            let mut test_attr = None;

            let attrs = attribute_table::get(&arg.embed.embed_token.token);
            for attr in attrs {
                if let Attr::Test(x, y) = attr {
                    test_attr = Some((x, y));
                }
            }

            let content = &arg.embed_content.embed_content_token.token;
            let r#type = match way.as_str() {
                "inline" => Some(TestType::Inline),
                "cocotb" => Some(TestType::CocotbEmbed(content.text)),
                _ => None,
            };

            if let (Some((token, top)), Some(r#type)) = (test_attr, r#type) {
                let path = if let TokenSource::File(x) = content.source {
                    x
                } else {
                    unreachable!()
                };

                if top.is_none() && way == "cocotb" {
                    self.errors
                        .push(AnalyzerError::invalid_test("`cocotb` test requires top module name at the second argument of `#[test]` attribute", self.text, &token.into()));
                }

                let property = TestProperty { r#type, path, top };
                self.insert_symbol(&token, SymbolKind::Test(property), false);
            }
        }
        Ok(())
    }

    fn include_declaration(&mut self, arg: &IncludeDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let way = arg.identifier.identifier_token.to_string();
            let mut test_attr = None;

            let attrs = attribute_table::get(&arg.include.include_token.token);
            for attr in attrs {
                if let Attr::Test(x, y) = attr {
                    test_attr = Some((x, y));
                }
            }

            let content = &arg.string_literal.string_literal_token.token;
            let r#type = match way.as_str() {
                "inline" => Some(TestType::Inline),
                "cocotb" => Some(TestType::CocotbInclude(content.text)),
                _ => None,
            };

            if let (Some((token, top)), Some(r#type)) = (test_attr, r#type) {
                let path = if let TokenSource::File(x) = content.source {
                    x
                } else {
                    unreachable!()
                };

                if top.is_none() && way == "cocotb" {
                    self.errors
                        .push(AnalyzerError::invalid_test("`cocotb` test requires top module name at the second argument of `#[test]` attribute", self.text, &token.into()));
                }

                let property = TestProperty { r#type, path, top };
                self.insert_symbol(&token, SymbolKind::Test(property), false);
            }
        }
        Ok(())
    }
}
