use crate::analyzer_error::AnalyzerError;
use crate::attribute::Attribute as Attr;
use crate::attribute::{AllowItem, EnumEncodingItem};
use crate::attribute_table;
use crate::definition_table::{self, Definition};
use crate::evaluator::{EvaluatedValue, Evaluator};
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::reference_table::{self, ReferenceCandidate};
use crate::symbol::ClockDomain as SymClockDomain;
use crate::symbol::Direction as SymDirection;
use crate::symbol::ModportDefault as SymModportDefault;
use crate::symbol::Type as SymType;
use crate::symbol::{
    AliasInterfaceProperty, AliasModuleProperty, AliasPackageProperty, ConnectTarget,
    ConnectTargetIdentifier, DocComment, EnumMemberProperty, EnumMemberValue, EnumProperty,
    FunctionProperty, GenericBoundKind, GenericParameterProperty, InstanceProperty,
    InterfaceProperty, ModportFunctionMemberProperty, ModportProperty,
    ModportVariableMemberProperty, ModuleProperty, PackageProperty, Parameter, ParameterKind,
    ParameterProperty, Port, PortProperty, ProtoConstProperty, ProtoInterfaceProperty,
    ProtoModuleProperty, ProtoPackageProperty, ProtoTypeDefProperty, StructMemberProperty,
    StructProperty, Symbol, SymbolId, SymbolKind, TestProperty, TestType, TypeDefProperty,
    TypeKind, TypeModifierKind, UnionMemberProperty, UnionProperty, VariableAffiliation,
    VariableProperty,
};
use crate::symbol_path::{GenericSymbolPath, GenericSymbolPathNamesapce};
use crate::symbol_table;
use crate::symbol_table::Import as SymImport;
use crate::type_dag::{self, Context, TypeDagCandidate};
use std::collections::{HashMap, HashSet};
use veryl_metadata::ClockType;
use veryl_metadata::{Build, ResetType};
use veryl_parser::ParolError;
use veryl_parser::doc_comment_table;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenSource};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
struct GenericContext {
    parameters: Vec<Vec<SymbolId>>,
    references: Vec<Vec<GenericSymbolPath>>,
}

impl GenericContext {
    pub fn push(&mut self) {
        self.parameters.push(Vec::new());
        self.references.push(Vec::new());
    }

    pub fn pop(&mut self) -> (Vec<SymbolId>, Vec<GenericSymbolPath>) {
        (
            self.parameters.pop().unwrap(),
            self.references.pop().unwrap(),
        )
    }

    pub fn push_parameter(&mut self, id: SymbolId) {
        self.parameters.last_mut().unwrap().push(id);
    }

    pub fn push_reference(&mut self, path: GenericSymbolPath) {
        if !self.references.is_empty() && path.may_be_generic_reference() {
            self.references.last_mut().unwrap().push(path);
        }
    }
}

#[derive(Default)]
pub struct CreateSymbolTable {
    pub errors: Vec<AnalyzerError>,
    build_opt: Build,
    point: HandlerPoint,
    namespace: Namespace,
    project_namespace: Namespace,
    module_namspace_depth: usize,
    default_block: Option<StrId>,
    for_identifier: Option<Token>,
    anonymous_namespace: usize,
    attribute_lines: HashSet<u32>,
    struct_or_union: Option<StructOrUnion>,
    enum_encoding: EnumEncodingItem,
    enum_member_prefix: Option<String>,
    enum_member_width: usize,
    enum_member_value: Option<EnumMemberValue>,
    enum_members: Vec<Option<SymbolId>>,
    struct_union_members: Vec<Option<SymbolId>>,
    declaration_items: Vec<SymbolId>,
    affiliation: Vec<VariableAffiliation>,
    connect_target_identifiers: Vec<ConnectTargetIdentifier>,
    connects: HashMap<Token, ConnectTarget>,
    parameters: Vec<Vec<Parameter>>,
    ports: Vec<Vec<Port>>,
    needs_default_generic_argument: bool,
    generic_context: GenericContext,
    default_clock: Option<SymbolId>,
    default_clock_candidates: Vec<SymbolId>,
    default_reset: Option<SymbolId>,
    defualt_reset_candidates: Vec<SymbolId>,
    variable_ids: HashMap<StrId, SymbolId>,
    modport_member_ids: Vec<SymbolId>,
    modport_ids: Vec<SymbolId>,
    function_ids: HashMap<StrId, SymbolId>,
    exist_clock_without_domain: bool,
    in_proto: bool,
    in_import: bool,
    file_scope_import_item: Vec<GenericSymbolPathNamesapce>,
    file_scope_import_wildcard: Vec<GenericSymbolPathNamesapce>,
    is_public: bool,
    identifier_factor_names: Vec<ExpressionIdentifier>,
    in_named_argument: Vec<bool>,
    in_argument_expression: Vec<()>,
    type_dag_candidates: Vec<Vec<TypeDagCandidate>>,
}

#[derive(Clone)]
enum StructOrUnion {
    InStruct,
    InUnion,
}

fn calc_width(value: usize) -> usize {
    (usize::BITS - value.leading_zeros()) as usize
}

impl CreateSymbolTable {
    pub fn new(build_opt: &Build) -> Self {
        Self {
            build_opt: build_opt.clone(),
            project_namespace: namespace_table::get_default(),
            ..Default::default()
        }
    }

    fn insert_namespace(&self, token: &Token) {
        if let TokenSource::File { path, .. } = token.source {
            let namespace = self.get_namespace(token);
            namespace_table::insert(token.id, path, &namespace);
        }
    }

    fn check_identifer_with_type_path(
        &mut self,
        identifier: &Identifier,
        type_path: &GenericSymbolPath,
    ) -> bool {
        let identifier_token = identifier.identifier_token.token;
        let type_base = type_path.paths[0].base;
        if identifier_token.text == type_base.text {
            self.errors.push(AnalyzerError::duplicated_identifier(
                &identifier_token.to_string(),
                &identifier_token.into(),
            ));
            false
        } else {
            true
        }
    }

    fn check_identifer_with_type(&mut self, identifier: &Identifier, r#type: &SymType) -> bool {
        if let Some(user_defined) = r#type.get_user_defined() {
            self.check_identifer_with_type_path(identifier, &user_defined.path)
        } else {
            true
        }
    }

    fn get_namespace(&self, token: &Token) -> Namespace {
        let attrs = attribute_table::get(token);
        let mut ret = self.namespace.clone();
        ret.define_context = attrs.as_slice().into();
        ret
    }

    fn insert_symbol(&mut self, token: &Token, kind: SymbolKind, public: bool) -> Option<SymbolId> {
        let line = token.line;
        let doc_comment = if let TokenSource::File { path, .. } = token.source {
            if line == 0 {
                DocComment::default()
            } else if let Some(doc_comment) = doc_comment_table::get(path, line) {
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
                while let Some(doc_comment) = doc_comment_table::get(path, candidate_line) {
                    ret.push(doc_comment);
                    candidate_line -= 1;
                }
                ret.reverse();
                DocComment(ret)
            }
        } else {
            DocComment::default()
        };
        let mut symbol = Symbol::new(token, kind, &self.get_namespace(token), public, doc_comment);

        if attribute_table::contains(token, Attr::Allow(AllowItem::UnusedVariable)) {
            symbol.allow_unused = true;
        }

        let id = symbol_table::insert(token, symbol);
        if id.is_some() {
            // Some symbols (e.g. module declaration) are inserted at Hander::After phase.
            // For such symbols, namespace assosiated with the symbol and namespace included in
            // namespace_table are different.
            // Need to insert the namespace again to resolve this namespace mismatch.
            self.insert_namespace(token);
        } else {
            self.errors.push(AnalyzerError::duplicated_identifier(
                &token.to_string(),
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

        let token = &clock_domain.identifier.identifier_token.token;
        let id = if let Ok(symbol) = symbol_table::resolve((token, &self.get_namespace(token))) {
            symbol.found.id
        } else {
            let symbol = Symbol::new(
                token,
                SymbolKind::ClockDomain,
                &self.get_namespace(token),
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

    fn push_default_clock_reset(&mut self, identifier: &Token, id: SymbolId, kind: &SymbolKind) {
        let r#type = match kind {
            SymbolKind::Variable(x) => &x.r#type,
            SymbolKind::Port(x) => &x.r#type,
            _ => unreachable!(),
        };
        let can_be_default_clock = r#type.can_be_default_clock();
        let can_be_default_reest = r#type.can_be_default_reset();
        let in_module_top_hierarchy = *self.affiliation.last().unwrap()
            == VariableAffiliation::Module
            && self.namespace.depth() == self.module_namspace_depth;

        if let Some(default_modifier) = r#type.find_modifier(&TypeModifierKind::Default) {
            let error_reason = if !in_module_top_hierarchy {
                Some("here is not the module top layer")
            } else if !(can_be_default_clock || can_be_default_reest) {
                Some("the given type is not a single bit clock nor a single bit reset")
            } else {
                None
            };
            if let Some(reason) = error_reason {
                self.errors.push(AnalyzerError::invalid_modifier(
                    &default_modifier.to_string(),
                    reason,
                    &default_modifier.token.token.into(),
                ));
                return;
            }

            if can_be_default_clock && self.default_clock.is_none() {
                self.default_clock = Some(id);
            } else if can_be_default_clock {
                self.errors.push(AnalyzerError::multiple_default_clock(
                    &identifier.to_string(),
                    &identifier.into(),
                ));
            }

            if can_be_default_reest && self.default_reset.is_none() {
                self.default_reset = Some(id);
            } else if can_be_default_reest {
                self.errors.push(AnalyzerError::multiple_default_reset(
                    &identifier.to_string(),
                    &identifier.into(),
                ));
            }
        } else if in_module_top_hierarchy {
            if can_be_default_clock {
                self.default_clock_candidates.push(id);
            } else if can_be_default_reest {
                self.defualt_reset_candidates.push(id);
            }
        }
    }

    fn check_missing_clock_domain(&mut self, token: &Token, r#type: &SymType) {
        if r#type.kind.is_clock() {
            if self.exist_clock_without_domain {
                self.errors
                    .push(AnalyzerError::missing_clock_domain(&token.into()));
            }
            self.exist_clock_without_domain = true;
        }
    }

    fn evaluate_enum_value(&mut self, arg: &EnumItem) -> EnumMemberValue {
        if let Some(ref x) = arg.enum_item_opt {
            let evaluated = Evaluator::new(&[]).expression(&x.expression);
            if let EvaluatedValue::Fixed(value) = evaluated.value {
                let valid_variant = match self.enum_encoding {
                    EnumEncodingItem::OneHot => value.count_ones() == 1,
                    EnumEncodingItem::Gray => {
                        if let Some(expected) = self.enum_variant_next_value() {
                            (value as usize) == expected
                        } else {
                            true
                        }
                    }
                    _ => true,
                };
                if !valid_variant {
                    self.errors.push(AnalyzerError::invalid_enum_variant_value(
                        &arg.identifier.identifier_token.to_string(),
                        &self.enum_encoding.to_string(),
                        &arg.identifier.as_ref().into(),
                    ));
                }

                EnumMemberValue::ExplicitValue(*x.expression.clone(), Some(value as usize))
            } else if self.enum_encoding == EnumEncodingItem::Sequential {
                EnumMemberValue::ExplicitValue(*x.expression.clone(), None)
            } else {
                self.errors.push(AnalyzerError::unevaluatable_enum_variant(
                    &arg.identifier.identifier_token.to_string(),
                    &arg.identifier.as_ref().into(),
                ));
                EnumMemberValue::UnevaluableValue
            }
        } else if let Some(value) = self.enum_variant_next_value() {
            EnumMemberValue::ImplicitValue(value)
        } else {
            self.errors.push(AnalyzerError::unevaluatable_enum_variant(
                &arg.identifier.identifier_token.to_string(),
                &arg.identifier.as_ref().into(),
            ));
            EnumMemberValue::UnevaluableValue
        }
    }

    fn enum_variant_next_value(&mut self) -> Option<usize> {
        if let Some(value) = &self.enum_member_value {
            if let Some(value) = value.value() {
                match self.enum_encoding {
                    EnumEncodingItem::Sequential => Some(value + 1),
                    EnumEncodingItem::OneHot => Some(value << 1),
                    EnumEncodingItem::Gray => Some(((value + 1) >> 1) ^ (value + 1)),
                }
            } else {
                None
            }
        } else {
            match self.enum_encoding {
                EnumEncodingItem::OneHot => Some(1),
                _ => Some(0),
            }
        }
    }

    fn apply_file_scope_import(&self) {
        for x in &self.file_scope_import_item {
            let import = SymImport {
                path: x.clone(),
                namespace: self.namespace.clone(),
                wildcard: false,
            };
            symbol_table::add_import(import);
        }

        for x in &self.file_scope_import_wildcard {
            let import = SymImport {
                path: x.clone(),
                namespace: self.namespace.clone(),
                wildcard: true,
            };
            symbol_table::add_import(import);
        }
    }

    fn link_modport_members(&self) {
        for id in &self.modport_member_ids {
            let mut mp_member = symbol_table::get(*id).unwrap();
            match mp_member.kind {
                SymbolKind::ModportFunctionMember(_) => {
                    if let Some(id) = self.function_ids.get(&mp_member.token.text) {
                        let property = ModportFunctionMemberProperty { function: *id };
                        let kind = SymbolKind::ModportFunctionMember(property);
                        mp_member.kind = kind;
                        symbol_table::update(mp_member);
                    }
                }
                SymbolKind::ModportVariableMember(x) => {
                    if let Some(id) = self.variable_ids.get(&mp_member.token.text) {
                        let mut property = x;
                        property.variable = *id;
                        let kind = SymbolKind::ModportVariableMember(property);
                        mp_member.kind = kind;
                        symbol_table::update(mp_member);
                    }
                }
                _ => (),
            }
        }
    }

    fn expand_modport_default_member(&mut self, interface_id: SymbolId) {
        // collect all members in modport for default member
        let mut directions = HashMap::new();
        for id in &self.modport_ids {
            let mp = symbol_table::get(*id).unwrap();
            if let SymbolKind::Modport(ref x) = mp.kind {
                for x in &x.members {
                    let member = symbol_table::get(*x).unwrap();
                    if let SymbolKind::ModportVariableMember(x) = &member.kind {
                        directions.insert((mp.token.text, member.token.text), x.direction);
                    }
                }
            }
        }

        let interface_namesapce = symbol_table::get(interface_id)
            .map(|x| x.inner_namespace())
            .unwrap();
        for id in &self.modport_ids.clone() {
            let mut mp = symbol_table::get(*id).unwrap();
            if let SymbolKind::Modport(ref x) = mp.kind {
                // add default members
                let mut members = x.members.clone();
                members.append(&mut self.get_modport_default_members(
                    &mp,
                    &interface_namesapce,
                    &directions,
                ));

                let property = ModportProperty {
                    interface: interface_id,
                    members,
                    default: x.default.clone(),
                };
                let kind = SymbolKind::Modport(property);
                mp.kind = kind;
                symbol_table::update(mp);
            }
        }
    }

    fn get_modport_default_members(
        &mut self,
        mp: &Symbol,
        interface_namesapce: &Namespace,
        directions: &HashMap<(StrId, StrId), SymDirection>,
    ) -> Vec<SymbolId> {
        let mut ret = Vec::new();

        if let SymbolKind::Modport(ref x) = mp.kind
            && let Some(ref default) = x.default
        {
            let explicit_members: HashSet<_> = x
                .members
                .iter()
                .map(|x| symbol_table::get(*x).unwrap().token.text)
                .collect();
            let mut default_members: Vec<_> = self
                .variable_ids
                .iter()
                .filter(|(x, y)| {
                    !explicit_members.contains(x)
                        && symbol_table::get(**y)
                            .map(|x| {
                                // member variables should belong to
                                // the interface directly
                                x.namespace.matched(interface_namesapce)
                            })
                            .unwrap()
                })
                .collect();

            // Sort by SymbolId to keep inserting order as the same as definition order
            default_members.sort_by(|x, y| x.1.cmp(y.1));

            let namespace = mp.inner_namespace();
            for (text, id) in default_members {
                let direction = match default {
                    SymModportDefault::Input => Some(SymDirection::Input),
                    SymModportDefault::Output => Some(SymDirection::Output),
                    SymModportDefault::Same(tgt) => directions.get(&(tgt.text, *text)).copied(),
                    SymModportDefault::Converse(tgt) => {
                        directions.get(&(tgt.text, *text)).map(|x| x.converse())
                    }
                };

                if let Some(direction) = direction {
                    let path = mp.token.source.get_path().unwrap();
                    let token = Token::generate(*text, path);
                    namespace_table::insert(token.id, path, &namespace);

                    let property = ModportVariableMemberProperty {
                        direction,
                        variable: *id,
                    };
                    let kind = SymbolKind::ModportVariableMember(property);
                    let symbol =
                        Symbol::new(&token, kind, &namespace, false, DocComment::default());
                    if let Some(id) = symbol_table::insert(&token, symbol) {
                        ret.push(id);
                    }
                }
            }
        }

        ret
    }

    fn push_declaration_item(&mut self, id: SymbolId) {
        if matches!(
            self.affiliation.last(),
            Some(&VariableAffiliation::Interface) | Some(&VariableAffiliation::Package)
        ) {
            self.declaration_items.push(id);
        }
    }

    fn push_type_dag_cand(&mut self) {
        self.type_dag_candidates.push(Vec::new());
    }

    fn pop_type_dag_cand(&mut self, symbol: Option<(SymbolId, Context, bool)>) {
        if let Some(candidates) = self.type_dag_candidates.pop() {
            for mut cand in candidates {
                if let Some(symbol) = &symbol {
                    cand.set_parent((symbol.0, symbol.1));
                }

                type_dag::add(cand);
            }
        }

        if let Some(x) = self.type_dag_candidates.last_mut()
            && let Some(symbol) = &symbol
        {
            let import = if symbol.2 {
                let mut import = self.file_scope_import_item.clone();
                import.extend(self.file_scope_import_wildcard.clone());
                import
            } else {
                Vec::new()
            };
            let cand = TypeDagCandidate::Symbol {
                id: symbol.0,
                context: symbol.1,
                parent: None,
                import,
            };
            x.push(cand);
        }
    }
}

impl Handler for CreateSymbolTable {
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

impl VerylGrammarTrait for CreateSymbolTable {
    fn identifier(&mut self, arg: &Identifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.insert_namespace(&arg.identifier_token.token);
        }
        Ok(())
    }

    fn dollar_identifier(&mut self, arg: &DollarIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.insert_namespace(&arg.dollar_identifier_token.token);
        }
        Ok(())
    }

    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            reference_table::add(arg.into());
        }

        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if *self.in_named_argument.last().unwrap_or(&false)
                    && !self.in_argument_expression.is_empty()
                {
                    let func_pos = self.identifier_factor_names.len() - 2;
                    let function = self.identifier_factor_names.get(func_pos).unwrap().clone();
                    let cand = ReferenceCandidate::NamedArgument {
                        arg: arg.clone(),
                        function,
                    };
                    reference_table::add(cand);
                } else {
                    reference_table::add((arg, self.in_import).into());
                }

                // Add symbols under $sv namespace
                if let ScopedIdentifierGroup::DollarIdentifier(x) =
                    arg.scoped_identifier_group.as_ref()
                    && x.dollar_identifier.dollar_identifier_token.to_string() == "$sv"
                {
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

                let ident = arg.identifier().token;
                if ident.to_string() != "_" {
                    let path: GenericSymbolPath = arg.into();
                    let namespace = self.get_namespace(&ident);
                    let cand = TypeDagCandidate::Path {
                        path,
                        namespace,
                        project_namespace: self.project_namespace.clone(),
                        parent: None,
                    };

                    if let Some(x) = self.type_dag_candidates.last_mut() {
                        x.push(cand);
                    }
                }
            }
            HandlerPoint::After => {
                self.generic_context.push_reference(arg.into());
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            // This should be `After` not `Before`.
            // because namespace_table insertion of identifiers
            // in the expression_identifier should be done until `arg.into()`.
            self.connect_target_identifiers.push(arg.into());

            // This should be `After` not `Before`.
            // Because this should be executed after scoped_identifier to handle hierarchical access only
            reference_table::add(arg.into());
        }
        Ok(())
    }

    fn identifier_factor(&mut self, arg: &IdentifierFactor) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.identifier_factor_names
                    .push(*arg.expression_identifier.clone());
            }
            HandlerPoint::After => {
                self.identifier_factor_names.pop();
            }
        }
        Ok(())
    }

    fn argument_item(&mut self, arg: &ArgumentItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_named_argument.push(arg.argument_item_opt.is_some());
            }
            HandlerPoint::After => {
                self.in_named_argument.pop();
            }
        }
        Ok(())
    }

    fn argument_expression(&mut self, _arg: &ArgumentExpression) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_argument_expression.push(());
            }
            HandlerPoint::After => {
                self.in_argument_expression.pop();
            }
        }
        Ok(())
    }

    fn struct_constructor_item(&mut self, arg: &StructConstructorItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let cand = ReferenceCandidate::StructConstructorItem {
                arg: arg.clone(),
                r#type: self.identifier_factor_names.last().unwrap().clone(),
            };
            reference_table::add(cand);
        }
        Ok(())
    }

    fn attribute(&mut self, arg: &Attribute) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let line = arg.hash_l_bracket.hash_l_bracket_token.token.line;
            self.attribute_lines.insert(line);
        }
        Ok(())
    }

    fn statement_block(&mut self, _arg: &StatementBlock) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let name = format!("@{}", self.anonymous_namespace);
                let name = resource_table::insert_str(&name);
                self.namespace.push(name);
                self.anonymous_namespace += 1;
                self.affiliation.push(VariableAffiliation::StatementBlock);
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut r#type: SymType = arg.array_type.as_ref().into();
            if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                return Ok(());
            }

            r#type.is_const = true;
            let affiliation = self.affiliation.last().cloned().unwrap();
            let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
            let clock_domain = if let Some(ref x) = arg.let_statement_opt {
                self.insert_clock_domain(&x.clock_domain)
            } else if affiliation == VariableAffiliation::Module {
                self.check_missing_clock_domain(&arg.identifier.identifier_token.token, &r#type);
                SymClockDomain::Implicit
            } else {
                SymClockDomain::None
            };
            let property = VariableProperty {
                r#type,
                affiliation,
                prefix,
                suffix,
                clock_domain,
                loop_variable: false,
            };
            let kind = SymbolKind::Variable(property);

            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind.clone(), false)
            {
                self.push_default_clock_reset(&arg.identifier.identifier_token.token, id, &kind);
            }
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.identifier_factor_names
                    .push(*arg.expression_identifier.clone());
            }
            HandlerPoint::After => {
                self.identifier_factor_names.pop();
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
                if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                    return Ok(());
                }

                let affiliation = self.affiliation.last().cloned().unwrap();
                let property = VariableProperty {
                    r#type,
                    affiliation,
                    prefix: None,
                    suffix: None,
                    clock_domain: SymClockDomain::None,
                    loop_variable: true,
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
            if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                return Ok(());
            }

            r#type.is_const = true;
            let affiliation = self.affiliation.last().cloned().unwrap();
            let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
            let clock_domain = if let Some(ref x) = arg.let_declaration_opt {
                self.insert_clock_domain(&x.clock_domain)
            } else if affiliation == VariableAffiliation::Module {
                self.check_missing_clock_domain(&arg.identifier.identifier_token.token, &r#type);
                SymClockDomain::Implicit
            } else {
                SymClockDomain::None
            };
            let property = VariableProperty {
                r#type,
                affiliation,
                prefix,
                suffix,
                clock_domain,
                loop_variable: false,
            };
            let kind = SymbolKind::Variable(property);

            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind.clone(), false)
            {
                self.push_declaration_item(id);
                self.push_default_clock_reset(&arg.identifier.identifier_token.token, id, &kind);
            }
        }
        Ok(())
    }

    fn var_declaration(&mut self, arg: &VarDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = arg.array_type.as_ref().into();
            if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                return Ok(());
            }

            let affiliation = self.affiliation.last().cloned().unwrap();
            let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
            let clock_domain = if let Some(ref x) = arg.var_declaration_opt {
                self.insert_clock_domain(&x.clock_domain)
            } else if affiliation == VariableAffiliation::Module {
                self.check_missing_clock_domain(&arg.identifier.identifier_token.token, &r#type);
                SymClockDomain::Implicit
            } else {
                SymClockDomain::None
            };
            let property = VariableProperty {
                r#type,
                affiliation,
                prefix,
                suffix,
                clock_domain,
                loop_variable: false,
            };
            let kind = SymbolKind::Variable(property);

            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind.clone(), false)
            {
                self.push_declaration_item(id);
                self.push_default_clock_reset(&arg.identifier.identifier_token.token, id, &kind);
                let text = arg.identifier.identifier_token.token.text;
                self.variable_ids.insert(text, id);
            }
        }
        Ok(())
    }

    fn const_declaration(&mut self, arg: &ConstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                let token = arg.identifier.identifier_token.token;
                let value = Some(*arg.expression.clone());
                let property = match &*arg.const_declaration_group {
                    ConstDeclarationGroup::ArrayType(x) => {
                        let r#type: SymType = x.array_type.as_ref().into();
                        if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                            self.pop_type_dag_cand(None);
                            return Ok(());
                        }

                        ParameterProperty {
                            token,
                            r#type,
                            kind: ParameterKind::Const,
                            value,
                        }
                    }
                    ConstDeclarationGroup::Type(_) => {
                        let r#type: SymType = SymType {
                            modifier: vec![],
                            kind: TypeKind::Type,
                            width: vec![],
                            array: vec![],
                            array_type: None,
                            is_const: false,
                        };
                        ParameterProperty {
                            token,
                            r#type,
                            kind: ParameterKind::Const,
                            value,
                        }
                    }
                };
                let kind = SymbolKind::Parameter(property);
                if let Some(id) = self.insert_symbol(&token, kind, false) {
                    self.push_declaration_item(id);
                    self.pop_type_dag_cand(Some((id, Context::Const, false)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn modport_declaration(&mut self, arg: &ModportDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.namespace
                    .push(arg.identifier.identifier_token.token.text);
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                let mut members = Vec::new();
                let items: Vec<ModportItem> = if let Some(ref x) = arg.modport_declaration_opt {
                    x.modport_list.as_ref().into()
                } else {
                    Vec::new()
                };

                for item in items {
                    let kind = match &*item.direction {
                        Direction::Modport(_) => {
                            continue;
                        }
                        Direction::Import(_) => {
                            let property = ModportFunctionMemberProperty {
                                function: SymbolId::default(),
                            };
                            SymbolKind::ModportFunctionMember(property)
                        }
                        _ => {
                            let direction: crate::symbol::Direction =
                                item.direction.as_ref().into();
                            let property = ModportVariableMemberProperty {
                                direction,
                                variable: SymbolId::default(),
                            };
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

                let default = if let Some(ref x) = arg.modport_declaration_opt0 {
                    match x.modport_default.as_ref() {
                        ModportDefault::Input(_) => Some(crate::symbol::ModportDefault::Input),
                        ModportDefault::Output(_) => Some(crate::symbol::ModportDefault::Output),
                        ModportDefault::SameLParenIdentifierRParen(x) => {
                            reference_table::add(x.identifier.as_ref().into());
                            Some(crate::symbol::ModportDefault::Same(
                                x.identifier.identifier_token.token,
                            ))
                        }
                        ModportDefault::ConverseLParenIdentifierRParen(x) => {
                            reference_table::add(x.identifier.as_ref().into());
                            Some(crate::symbol::ModportDefault::Converse(
                                x.identifier.identifier_token.token,
                            ))
                        }
                    }
                } else {
                    None
                };

                let property = ModportProperty {
                    // Dummy SymbolId, the actual value is inserted at interface_declaration
                    interface: SymbolId(0),
                    members,
                    default,
                };
                let kind = SymbolKind::Modport(property);
                if let Some(id) =
                    self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
                {
                    self.push_declaration_item(id);
                    self.modport_ids.push(id);
                    self.pop_type_dag_cand(Some((id, Context::Modport, false)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn modport_item(&mut self, arg: &ModportItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            reference_table::add(arg.into());
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

                // reset enum encoding/width/value
                self.enum_encoding = EnumEncodingItem::Sequential;
                self.enum_member_width = 1;
                self.enum_member_value = None;

                let attrs = attribute_table::get(&arg.r#enum.enum_token.token);
                for attr in attrs {
                    if let Attr::EnumMemberPrefix(x) = attr {
                        // overridden prefix by attribute
                        self.enum_member_prefix = Some(x.to_string());
                    } else if let Attr::EnumEncoding(x) = attr {
                        // overridden encoding by attribute
                        self.enum_encoding = x;
                    }
                }

                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.enum_member_prefix = None;

                let members: Vec<_> = self.enum_members.drain(0..).flatten().collect();
                let r#type = arg
                    .enum_declaration_opt
                    .as_ref()
                    .map(|x| x.scalar_type.as_ref().into());
                if let Some(r#type) = &r#type
                    && !self.check_identifer_with_type(&arg.identifier, r#type)
                {
                    self.pop_type_dag_cand(None);
                    return Ok(());
                }

                let width = if let Some(x) = r#type.clone() {
                    if let Some(x) = Evaluator::new(&[]).type_width(x) {
                        *x.first().unwrap_or(&0)
                    } else {
                        0
                    }
                } else {
                    calc_width(members.len() - 1).max(self.enum_member_width)
                };

                let property = EnumProperty {
                    r#type,
                    width,
                    members,
                    encoding: self.enum_encoding,
                };
                let kind = SymbolKind::Enum(property);
                if let Some(id) =
                    self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
                {
                    self.push_declaration_item(id);
                    self.pop_type_dag_cand(Some((id, Context::Enum, false)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn enum_item(&mut self, arg: &EnumItem) -> Result<(), ParolError> {
        // Namespaces of identifiers on RHS of enum item need
        // to be inserted before resolving symbols of the identifiers.
        // Therefore, the symbol of enum item needs to be created at Hander::After phase
        // because namespaces have been created at Hander::Before phase.
        if let HandlerPoint::After = self.point {
            let token = arg.identifier.identifier_token.token;

            let value = self.evaluate_enum_value(arg);
            let prefix = self.enum_member_prefix.clone().unwrap();
            let property = EnumMemberProperty {
                value: value.clone(),
                prefix,
            };
            let kind = SymbolKind::EnumMember(property);
            let id = self.insert_symbol(&token, kind, false);
            self.enum_members.push(id);

            // add EnumMemberMangled to detect identifier conflict in generated SV
            let prefix = self.enum_member_prefix.clone().unwrap();
            let path = token.source.get_path().unwrap();
            let text = resource_table::insert_str(&format!("{prefix}_{}", token.text));
            let mangled_token = Token::generate(text, path);
            let kind = SymbolKind::EnumMemberMangled;

            // namespace of EnumMemberMangled is outside of enum
            let namespace = self.namespace.pop();
            self.insert_symbol(&mangled_token, kind, false);
            if let Some(namespace) = namespace {
                self.namespace.push(namespace);
            }

            self.enum_member_width = self
                .enum_member_width
                .max(calc_width(value.value().unwrap_or(0)));
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

                if arg.struct_union_declaration_opt.is_some() {
                    self.generic_context.push();
                }
                self.namespace.push(name);
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                self.struct_or_union = None;
                self.namespace.pop();

                let (generic_parameters, generic_references) =
                    if arg.struct_union_declaration_opt.is_some() {
                        self.generic_context.pop()
                    } else {
                        (vec![], vec![])
                    };

                let members: Vec<_> = self.struct_union_members.drain(0..).flatten().collect();
                let (context, kind) = match &*arg.struct_union {
                    StructUnion::Struct(_) => {
                        let property = StructProperty {
                            members,
                            generic_parameters,
                            generic_references,
                        };
                        (Context::Struct, SymbolKind::Struct(property))
                    }
                    StructUnion::Union(_) => {
                        let property = UnionProperty {
                            members,
                            generic_parameters,
                            generic_references,
                        };
                        (Context::Union, SymbolKind::Union(property))
                    }
                };
                if let Some(id) =
                    self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
                {
                    self.push_declaration_item(id);
                    self.pop_type_dag_cand(Some((id, context, false)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                let r#type = arg.array_type.as_ref().into();
                if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                    self.pop_type_dag_cand(None);
                    return Ok(());
                }

                let property = TypeDefProperty { r#type };
                let kind = SymbolKind::TypeDef(property);
                if let Some(id) =
                    self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
                {
                    self.push_declaration_item(id);
                    self.pop_type_dag_cand(Some((id, Context::TypeDef, false)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn struct_union_item(&mut self, arg: &StructUnionItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let r#type: SymType = arg.scalar_type.as_ref().into();
            if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                return Ok(());
            }

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
            let array: Vec<Expression> = if let Some(x) = &arg.inst_declaration_opt0 {
                x.array.as_ref().into()
            } else {
                Vec::new()
            };
            let type_name: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
            if !self.check_identifer_with_type_path(&arg.identifier, &type_name) {
                return Ok(());
            }

            let connects = self.connects.drain().collect();
            let clock_domain = if let Some(ref x) = arg.inst_declaration_opt {
                self.insert_clock_domain(&x.clock_domain)
            } else {
                // Clock domain will be updated to 'Implicit' if the instance is for an interface
                SymClockDomain::None
            };
            let property = InstanceProperty {
                array,
                type_name,
                connects,
                clock_domain,
            };
            let kind = SymbolKind::Instance(property);
            self.insert_symbol(&arg.identifier.identifier_token.token, kind, false);
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                reference_table::add(arg.into());
                self.connect_target_identifiers.clear();
            }
            HandlerPoint::After => {
                let port = arg.identifier.identifier_token.token;
                let identifiers = if arg.inst_port_item_opt.is_some() {
                    self.connect_target_identifiers.drain(0..).collect()
                } else {
                    vec![ConnectTargetIdentifier {
                        path: vec![(port.text, vec![])],
                    }]
                };
                let expression = if let Some(x) = &arg.inst_port_item_opt {
                    x.expression.as_ref().clone()
                } else {
                    arg.identifier.as_ref().into()
                };
                let target = ConnectTarget {
                    identifiers,
                    expression,
                };
                self.connects.insert(port, target);
            }
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let kind = match &*arg.with_parameter_item_group {
                WithParameterItemGroup::Param(_) => ParameterKind::Param,
                WithParameterItemGroup::Const(_) => ParameterKind::Const,
            };
            let value = arg
                .with_parameter_item_opt
                .as_ref()
                .map(|x| x.expression.as_ref().clone());
            let property = match &*arg.with_parameter_item_group0 {
                WithParameterItemGroup0::ArrayType(x) => {
                    let r#type: SymType = x.array_type.as_ref().into();
                    if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                        return Ok(());
                    }

                    ParameterProperty {
                        token,
                        r#type,
                        kind,
                        value,
                    }
                }
                WithParameterItemGroup0::Type(_) => {
                    let r#type: SymType = SymType {
                        modifier: vec![],
                        kind: TypeKind::Type,
                        width: vec![],
                        array: vec![],
                        array_type: None,
                        is_const: false,
                    };
                    ParameterProperty {
                        token,
                        r#type,
                        kind,
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
                    Some(x.with_generic_argument_item.as_ref().into())
                } else {
                    None
                };

            let bound = match arg.generic_bound.as_ref() {
                GenericBound::Type(_) => GenericBoundKind::Type,
                GenericBound::InstScopedIdentifier(x) => {
                    let type_path: GenericSymbolPath = x.scoped_identifier.as_ref().into();
                    if !self.check_identifer_with_type_path(&arg.identifier, &type_path) {
                        return Ok(());
                    }
                    GenericBoundKind::Inst(type_path.mangled_path())
                }
                GenericBound::GenericProtoBound(x) => {
                    let r#type: SymType = x.generic_proto_bound.as_ref().into();
                    if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                        return Ok(());
                    }
                    GenericBoundKind::Proto(r#type)
                }
            };

            if !self.needs_default_generic_argument || default_value.is_some() {
                let property = GenericParameterProperty {
                    bound,
                    default_value,
                };
                let kind = SymbolKind::GenericParameter(property);
                if let Some(id) =
                    self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
                {
                    self.generic_context.push_parameter(id);
                }
            } else {
                self.errors.push(AnalyzerError::missing_default_argument(
                    &arg.identifier.identifier_token.token.to_string(),
                    &arg.identifier.as_ref().into(),
                ));
            }
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let affiliation = self.affiliation.last().cloned().unwrap();
            let property = match &*arg.port_declaration_item_group {
                PortDeclarationItemGroup::PortTypeConcrete(x) => {
                    let x = x.port_type_concrete.as_ref();
                    let r#type: SymType = x.array_type.as_ref().into();
                    if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                        return Ok(());
                    }

                    let direction: SymDirection = x.direction.as_ref().into();
                    let (prefix, suffix) = self.get_signal_prefix_suffix(r#type.kind.clone());
                    let clock_domain = if let Some(ref x) = x.port_type_concrete_opt {
                        self.insert_clock_domain(&x.clock_domain)
                    } else if affiliation == VariableAffiliation::Module {
                        self.check_missing_clock_domain(
                            &arg.identifier.identifier_token.token,
                            &r#type,
                        );
                        SymClockDomain::Implicit
                    } else {
                        SymClockDomain::None
                    };
                    let default_value = x
                        .port_type_concrete_opt0
                        .as_ref()
                        .map(|x| *x.port_default_value.expression.clone());
                    PortProperty {
                        token,
                        r#type,
                        direction,
                        prefix,
                        suffix,
                        clock_domain,
                        default_value,
                        is_proto: self.in_proto,
                    }
                }
                PortDeclarationItemGroup::PortTypeAbstract(x) => {
                    let x = &x.port_type_abstract;
                    let clock_domain = if let Some(ref x) = x.port_type_abstract_opt {
                        self.insert_clock_domain(&x.clock_domain)
                    } else if affiliation == VariableAffiliation::Module {
                        SymClockDomain::Implicit
                    } else {
                        SymClockDomain::None
                    };
                    let kind = if let Some(ref x) = x.port_type_abstract_opt0 {
                        TypeKind::AbstractInterface(Some(x.identifier.identifier_token.token.text))
                    } else {
                        TypeKind::AbstractInterface(None)
                    };
                    let array: Vec<Expression> = if let Some(ref x) = x.port_type_abstract_opt1 {
                        x.array.as_ref().into()
                    } else {
                        Vec::new()
                    };
                    let r#type = SymType {
                        kind,
                        modifier: vec![],
                        width: vec![],
                        array,
                        array_type: None,
                        is_const: false,
                    };
                    PortProperty {
                        token,
                        r#type,
                        direction: SymDirection::Interface,
                        prefix: None,
                        suffix: None,
                        clock_domain,
                        default_value: None,
                        is_proto: self.in_proto,
                    }
                }
            };
            let kind = SymbolKind::Port(property);

            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind.clone(), false)
            {
                let port = Port {
                    token: arg.identifier.identifier_token.clone(),
                    symbol: id,
                };
                self.ports.last_mut().unwrap().push(port);
                self.push_default_clock_reset(&arg.identifier.identifier_token.token, id, &kind);
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;
        match self.point {
            HandlerPoint::Before => {
                self.namespace.push(name);
                if arg.function_declaration_opt.is_some() {
                    self.generic_context.push();
                }
                self.ports.push(Vec::new());
                self.affiliation.push(VariableAffiliation::Function);
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();

                let (generic_parameters, generic_references) =
                    if arg.function_declaration_opt.is_some() {
                        self.generic_context.pop()
                    } else {
                        (vec![], vec![])
                    };
                let ports: Vec<_> = self.ports.pop().unwrap();

                let ret = arg
                    .function_declaration_opt1
                    .as_ref()
                    .map(|x| (&*x.scalar_type).into());
                if let Some(ret) = &ret
                    && !self.check_identifer_with_type(&arg.identifier, ret)
                {
                    self.pop_type_dag_cand(None);
                    return Ok(());
                }

                let range = TokenRange::new(
                    &arg.function.function_token,
                    &arg.statement_block.r_brace.r_brace_token,
                );

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
                    self.push_declaration_item(id);
                    self.pop_type_dag_cand(Some((id, Context::Function, false)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn import_declaration(&mut self, arg: &ImportDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_import = true;

                let namespace = self.get_namespace(&arg.scoped_identifier.identifier().token);
                let path: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
                let path: GenericSymbolPathNamesapce = (&path, &namespace).into();
                let wildcard = arg.import_declaration_opt.is_some();

                let import = SymImport {
                    path: path.clone(),
                    namespace: path.1.clone(),
                    wildcard,
                };

                if self.affiliation.is_empty() {
                    if wildcard {
                        self.file_scope_import_wildcard.push(path);
                    } else {
                        self.file_scope_import_item.push(path);
                    }
                } else {
                    symbol_table::add_import(import);
                }
            }
            HandlerPoint::After => self.in_import = false,
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;
        match self.point {
            HandlerPoint::Before => {
                self.namespace.push(name);
                if arg.module_declaration_opt.is_some() {
                    self.generic_context.push();
                }
                self.parameters.push(Vec::new());
                self.ports.push(Vec::new());
                self.affiliation.push(VariableAffiliation::Module);
                self.module_namspace_depth = self.namespace.depth();
                self.function_ids.clear();
                self.exist_clock_without_domain = false;

                self.apply_file_scope_import();
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();
                self.module_namspace_depth = 0;

                let (generic_parameters, generic_references) =
                    if arg.module_declaration_opt.is_some() {
                        self.generic_context.pop()
                    } else {
                        (vec![], vec![])
                    };
                let parameters: Vec<_> = self.parameters.pop().unwrap();
                let ports: Vec<_> = self.ports.pop().unwrap();

                let default_clock = if self.default_clock.is_some() {
                    self.default_clock
                } else if self.default_clock_candidates.len() == 1 {
                    Some(self.default_clock_candidates[0])
                } else {
                    None
                };
                let default_reset = if self.default_reset.is_some() {
                    self.default_reset
                } else if self.defualt_reset_candidates.len() == 1 {
                    Some(self.defualt_reset_candidates[0])
                } else {
                    None
                };

                self.default_clock = None;
                self.default_clock_candidates.clear();
                self.default_reset = None;
                self.defualt_reset_candidates.clear();

                let proto = if let Some(x) = arg.module_declaration_opt0.as_ref() {
                    let path: GenericSymbolPath = x.scoped_identifier.as_ref().into();
                    if !self.check_identifer_with_type_path(&arg.identifier, &path) {
                        self.pop_type_dag_cand(None);
                        return Ok(());
                    }
                    Some(path)
                } else {
                    None
                };

                let definition = definition_table::insert(Definition::Module(arg.clone()));
                let property = ModuleProperty {
                    range: arg.into(),
                    proto,
                    generic_parameters,
                    generic_references,
                    parameters,
                    ports,
                    default_clock,
                    default_reset,
                    definition,
                };
                if let Some(id) = self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Module(property),
                    self.is_public,
                ) {
                    self.pop_type_dag_cand(Some((id, Context::Module, true)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn generate_for_declaration(&mut self, arg: &GenerateForDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.for_identifier = Some(arg.identifier.identifier_token.token);
        }
        Ok(())
    }

    fn generate_named_block(&mut self, arg: &GenerateNamedBlock) -> Result<(), ParolError> {
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

    fn generate_optional_named_block(
        &mut self,
        arg: &GenerateOptionalNamedBlock,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let name = if let Some(ref x) = arg.generate_optional_named_block_opt {
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
                if arg.interface_declaration_opt.is_some() {
                    self.generic_context.push();
                }
                self.parameters.push(Vec::new());
                self.affiliation.push(VariableAffiliation::Interface);
                self.variable_ids.clear();
                self.function_ids.clear();
                self.modport_member_ids.clear();
                self.modport_ids.clear();

                self.apply_file_scope_import();
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();

                let (generic_parameters, generic_references) =
                    if arg.interface_declaration_opt.is_some() {
                        self.generic_context.pop()
                    } else {
                        (vec![], vec![])
                    };
                let parameters: Vec<_> = self.parameters.pop().unwrap();

                let proto = if let Some(x) = arg.interface_declaration_opt0.as_ref() {
                    let path: GenericSymbolPath = x.scoped_identifier.as_ref().into();
                    if !self.check_identifer_with_type_path(&arg.identifier, &path) {
                        return Ok(());
                    }

                    Some(path)
                } else {
                    None
                };

                let definition = definition_table::insert(Definition::Interface(arg.clone()));
                let property = InterfaceProperty {
                    range: arg.into(),
                    proto,
                    generic_parameters,
                    generic_references,
                    parameters,
                    members: self.declaration_items.drain(..).collect(),
                    definition,
                };
                if let Some(id) = self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Interface(property),
                    self.is_public,
                ) {
                    self.link_modport_members();
                    self.expand_modport_default_member(id);
                    self.pop_type_dag_cand(Some((id, Context::Interface, true)));
                } else {
                    self.pop_type_dag_cand(None);
                };
            }
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        let name = arg.identifier.identifier_token.token.text;
        match self.point {
            HandlerPoint::Before => {
                self.namespace.push(name);
                if arg.package_declaration_opt.is_some() {
                    self.generic_context.push();
                }
                self.affiliation.push(VariableAffiliation::Package);
                self.function_ids.clear();
                self.apply_file_scope_import();
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();

                let (generic_parameters, generic_references) =
                    if arg.package_declaration_opt.is_some() {
                        self.generic_context.pop()
                    } else {
                        (vec![], vec![])
                    };

                let range = TokenRange::new(&arg.package.package_token, &arg.r_brace.r_brace_token);
                let proto = if let Some(x) = arg.package_declaration_opt0.as_ref() {
                    let path: GenericSymbolPath = x.scoped_identifier.as_ref().into();
                    if !self.check_identifer_with_type_path(&arg.identifier, &path) {
                        self.pop_type_dag_cand(None);
                        return Ok(());
                    }

                    Some(path)
                } else {
                    None
                };

                let property = PackageProperty {
                    range,
                    proto,
                    generic_parameters,
                    generic_references,
                    members: self.declaration_items.drain(..).collect(),
                };
                if let Some(id) = self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::Package(property),
                    self.is_public,
                ) {
                    self.pop_type_dag_cand(Some((id, Context::Package, true)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    /// Semantic action for non-terminal 'AliasDeclaration'
    fn alias_declaration(&mut self, arg: &AliasDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                let target: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
                if !self.check_identifer_with_type_path(&arg.identifier, &target) {
                    return Ok(());
                }

                let kind = match &*arg.alias_declaration_group {
                    AliasDeclarationGroup::Module(_) => {
                        let property = AliasModuleProperty { target };
                        SymbolKind::AliasModule(property)
                    }
                    AliasDeclarationGroup::Interface(_) => {
                        let property = AliasInterfaceProperty { target };
                        SymbolKind::AliasInterface(property)
                    }
                    AliasDeclarationGroup::Package(_) => {
                        let property = AliasPackageProperty { target };
                        SymbolKind::AliasPackage(property)
                    }
                };
                if let Some(id) =
                    self.insert_symbol(&arg.identifier.identifier_token.token, kind, self.is_public)
                {
                    self.push_declaration_item(id);
                    self.pop_type_dag_cand(Some((id, Context::Alias, false)));
                } else {
                    self.pop_type_dag_cand(None);
                }
            }
        }
        Ok(())
    }

    fn proto_module_declaration(&mut self, arg: &ProtoModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.namespace
                    .push(arg.identifier.identifier_token.token.text);
                self.affiliation.push(VariableAffiliation::Module);
                self.in_proto = true;
                self.parameters.push(Vec::new());
                self.ports.push(Vec::new());
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();
                self.in_proto = false;

                let parameters: Vec<_> = self.parameters.pop().unwrap();
                let ports: Vec<_> = self.ports.pop().unwrap();

                let property = ProtoModuleProperty {
                    range: arg.into(),
                    parameters,
                    ports,
                };
                self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::ProtoModule(property),
                    self.is_public,
                );
            }
        }
        Ok(())
    }

    fn proto_interface_declaration(
        &mut self,
        arg: &ProtoInterfaceDeclaration,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.namespace
                    .push(arg.identifier.identifier_token.token.text);
                self.affiliation.push(VariableAffiliation::Interface);
                self.parameters.push(Vec::new());
                self.function_ids.clear();
                self.apply_file_scope_import();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();

                let parameters: Vec<_> = self.parameters.pop().unwrap();
                let property = ProtoInterfaceProperty {
                    range: arg.into(),
                    parameters,
                    members: self.declaration_items.drain(..).collect(),
                };
                if let Some(interface_id) = self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::ProtoInterface(property),
                    self.is_public,
                ) {
                    self.link_modport_members();
                    self.expand_modport_default_member(interface_id);
                }
            }
        }
        Ok(())
    }

    fn proto_package_declaration(
        &mut self,
        arg: &ProtoPackageDeclaration,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.namespace
                    .push(arg.identifier.identifier_token.token.text);
                self.affiliation.push(VariableAffiliation::Package);
                self.function_ids.clear();
                self.apply_file_scope_import();
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();

                let property = ProtoPackageProperty {
                    range: arg.into(),
                    members: self.declaration_items.drain(..).collect(),
                };
                self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::ProtoPackage(property),
                    self.is_public,
                );
            }
        }

        Ok(())
    }

    fn proto_const_declaration(&mut self, arg: &ProtoConstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let r#type = match &*arg.proto_const_declaration_group {
                ProtoConstDeclarationGroup::ArrayType(x) => x.array_type.as_ref().into(),
                ProtoConstDeclarationGroup::Type(_) => SymType {
                    modifier: vec![],
                    kind: TypeKind::Type,
                    width: vec![],
                    array: vec![],
                    array_type: None,
                    is_const: false,
                },
            };
            if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                return Ok(());
            }

            let property = ProtoConstProperty { token, r#type };
            let kind = SymbolKind::ProtoConst(property);
            if let Some(id) = self.insert_symbol(&token, kind, false) {
                self.push_declaration_item(id);
            }
        }
        Ok(())
    }

    fn proto_type_def_declaration(
        &mut self,
        arg: &ProtoTypeDefDeclaration,
    ) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.identifier.identifier_token.token;
            let proprety = if let Some(ref x) = arg.proto_type_def_declaration_opt {
                let r#type = x.array_type.as_ref().into();
                if !self.check_identifer_with_type(&arg.identifier, &r#type) {
                    self.pop_type_dag_cand(None);
                    return Ok(());
                }
                ProtoTypeDefProperty {
                    r#type: Some(r#type),
                }
            } else {
                ProtoTypeDefProperty { r#type: None }
            };
            let kind = SymbolKind::ProtoTypeDef(proprety);
            if let Some(id) = self.insert_symbol(&token, kind, false) {
                self.push_declaration_item(id);
            }
        }
        Ok(())
    }

    fn proto_function_declaration(
        &mut self,
        arg: &ProtoFunctionDeclaration,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let name = arg.identifier.identifier_token.token.text;
                self.namespace.push(name);
                if arg.proto_function_declaration_opt.is_some() {
                    self.generic_context.push();
                }
                self.ports.push(Vec::new());
                self.affiliation.push(VariableAffiliation::Function);
            }
            HandlerPoint::After => {
                self.namespace.pop();
                self.affiliation.pop();

                let (generic_parameters, generic_references) =
                    if arg.proto_function_declaration_opt.is_some() {
                        self.generic_context.pop()
                    } else {
                        (vec![], vec![])
                    };
                let ports: Vec<_> = self.ports.pop().unwrap();

                let ret = arg
                    .proto_function_declaration_opt1
                    .as_ref()
                    .map(|x| (&*x.scalar_type).into());
                if let Some(ret) = &ret
                    && !self.check_identifer_with_type(&arg.identifier, ret)
                {
                    return Ok(());
                }

                let range =
                    TokenRange::new(&arg.function.function_token, &arg.semicolon.semicolon_token);

                let property = FunctionProperty {
                    range,
                    generic_parameters,
                    generic_references,
                    ports,
                    ret,
                };

                if let Some(id) = self.insert_symbol(
                    &arg.identifier.identifier_token.token,
                    SymbolKind::ProtoFunction(property),
                    false,
                ) {
                    self.function_ids
                        .insert(arg.identifier.identifier_token.token.text, id);
                    self.push_declaration_item(id);
                }
            }
        }
        Ok(())
    }

    fn proto_alias_declaration(&mut self, arg: &ProtoAliasDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let target: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
            if !self.check_identifer_with_type_path(&arg.identifier, &target) {
                return Ok(());
            }

            let kind = match &*arg.proto_alias_declaration_group {
                ProtoAliasDeclarationGroup::Module(_) => {
                    let property = AliasModuleProperty { target };
                    SymbolKind::ProtoAliasModule(property)
                }
                ProtoAliasDeclarationGroup::Interface(_) => {
                    let property = AliasInterfaceProperty { target };
                    SymbolKind::ProtoAliasInterface(property)
                }
                ProtoAliasDeclarationGroup::Package(_) => {
                    let property = AliasPackageProperty { target };
                    SymbolKind::ProtoAliasPackage(property)
                }
            };
            if let Some(id) =
                self.insert_symbol(&arg.identifier.identifier_token.token, kind, false)
            {
                self.push_declaration_item(id);
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

            let content = &arg.embed_content;
            let r#type = match way.as_str() {
                "inline" => Some(TestType::Inline),
                "cocotb" => Some(TestType::CocotbEmbed(content.clone())),
                _ => None,
            };

            if let (Some((token, top)), Some(r#type)) = (test_attr, r#type) {
                let content_source = content.triple_l_brace.triple_l_brace_token.token.source;
                let path = if let TokenSource::File { path, .. } = content_source {
                    path
                } else {
                    unreachable!()
                };

                if top.is_none() && way == "cocotb" {
                    self.errors
                        .push(AnalyzerError::invalid_test("`cocotb` test requires top module name at the second argument of `#[test]` attribute", &token.into()));
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
                let path = if let TokenSource::File { path, .. } = content.source {
                    path
                } else {
                    unreachable!()
                };

                if top.is_none() && way == "cocotb" {
                    self.errors
                        .push(AnalyzerError::invalid_test("`cocotb` test requires top module name at the second argument of `#[test]` attribute", &token.into()));
                }

                let property = TestProperty { r#type, path, top };
                self.insert_symbol(&token, SymbolKind::Test(property), false);
            }
        }
        Ok(())
    }

    fn description_item(&mut self, arg: &DescriptionItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) = &arg {
                    self.is_public = x.description_item_opt.is_some();
                }
            }
            HandlerPoint::After => {
                self.is_public = false;
            }
        }

        Ok(())
    }

    fn veryl(&mut self, _arg: &Veryl) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.push_type_dag_cand();
            }
            HandlerPoint::After => {
                self.pop_type_dag_cand(None);
            }
        }

        Ok(())
    }
}
