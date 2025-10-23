use crate::analyzer::AnalyzerPass2Expression;
use crate::analyzer_error::AnalyzerError;
use crate::connect_operation_table::{self, ConnectOperand};
use crate::definition_table::{self, Definition};
use crate::evaluator::{Evaluated, EvaluatedError, EvaluatedType, Evaluator};
use crate::instance_history::{self, InstanceHistoryError, InstanceSignature};
use crate::symbol::{Direction, ModuleProperty, Symbol, SymbolId, SymbolKind, TypeKind};
use crate::symbol_table;
use std::collections::{HashMap, HashSet};
use veryl_parser::ParolError;
use veryl_parser::resource_table::StrId;
use veryl_parser::resource_table::TokenId;
use veryl_parser::token_range::{TokenExt, TokenRange};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Context {
    Assignment,
    PortConnection,
    ParameterConnection,
}

#[derive(Default)]
pub struct CheckExpression {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    evaluator: Evaluator,
    port_direction: Option<Direction>,
    in_proto: bool,
    in_input_port_default_value: bool,
    disable: bool,
    disable_block_beg: HashSet<TokenId>,
    disable_block_end: HashSet<TokenId>,
    inst_context: Vec<TokenRange>,
}

impl CheckExpression {
    pub fn new(inst_context: Vec<TokenRange>) -> Self {
        Self {
            inst_context,
            ..Default::default()
        }
    }

    fn evaluated_error(&mut self, errors: &[EvaluatedError]) {
        for e in errors {
            self.errors
                .push(AnalyzerError::evaluated_error(e, &self.inst_context));
        }
    }

    fn inst_history_error(&mut self, error: InstanceHistoryError, token: &TokenRange) {
        let error = match error {
            InstanceHistoryError::ExceedDepthLimit => {
                AnalyzerError::exceed_limit("hierarchy depth limit", token)
            }
            InstanceHistoryError::ExceedTotalLimit => {
                AnalyzerError::exceed_limit("total instance limit", token)
            }
            InstanceHistoryError::InfiniteRecursion => AnalyzerError::infinite_recursion(token),
        };
        self.errors.push(error);
    }

    fn evaluate_expression(
        &mut self,
        expression: &Expression,
        in_type_expression: bool,
    ) -> Evaluated {
        let mut result = self.evaluator.expression(expression);
        self.evaluated_error(&result.errors);

        if result.errors.is_empty() && result.is_type() != in_type_expression {
            let range: TokenRange = expression.into();
            self.errors.push(AnalyzerError::invalid_factor(
                None,
                &result.r#type.to_string(),
                &range,
                &self.inst_context,
            ));
            result.set_unknown();
        }

        result
    }

    fn evaluate_connection(
        &mut self,
        _context: Context,
        src: &Expression,
        dst: &Symbol,
        dst_last_select: &[Select],
        token: &TokenRange,
    ) -> Evaluated {
        let dst_type = dst.kind.get_type();
        let in_type_expression = dst_type
            .map(|x| matches!(x.kind, TypeKind::Type))
            .unwrap_or(false);
        let src = self.evaluate_expression(src, in_type_expression);

        if let Some(dst_type) = dst.kind.get_type()
            && src.errors.is_empty()
            && src.r#type != EvaluatedType::Type
            && src.r#type != EvaluatedType::Unknown
        {
            // check array dimension
            let src_array_dim = src.get_array().unwrap().len();
            let mut dst_array_dim = dst_type.array.len();

            for s in dst_last_select {
                let (_, _, single) = self.evaluator.evaluate_select(s);
                if single {
                    dst_array_dim = dst_array_dim.saturating_sub(1);
                }
            }

            if src_array_dim != dst_array_dim {
                self.errors.push(AnalyzerError::mismatch_assignment(
                    &format!("{src_array_dim}-D array"),
                    &format!("{dst_array_dim}-D array"),
                    token,
                    &self.inst_context,
                ));
            }

            if dst_type.kind.is_2state() && src.is_4state() {
                self.errors.push(AnalyzerError::mismatch_assignment(
                    "4-state value",
                    "2-state variable",
                    token,
                    &self.inst_context,
                ));
            }

            if let TypeKind::UserDefined(x) = &dst_type.kind {
                let Some(dst_symbol) = x.symbol else {
                    return src;
                };
                let dst_symbol = symbol_table::get(dst_symbol).unwrap();
                if let SymbolKind::Modport(dst) = &dst_symbol.kind {
                    let dst_interface = symbol_table::get(dst.interface).unwrap();
                    if let EvaluatedType::UserDefined(src) = &src.r#type {
                        let src_symbol = symbol_table::get(src.symbol).unwrap();
                        if dst.interface != src.symbol {
                            self.errors.push(AnalyzerError::mismatch_assignment(
                                &format!("instance of {}", src_symbol.token),
                                &format!("modport of {}", dst_interface.token),
                                token,
                                &self.inst_context,
                            ));
                        }
                    } else {
                        self.errors.push(AnalyzerError::mismatch_assignment(
                            "non-interface",
                            &format!("modport of {}", dst_interface.token),
                            token,
                            &self.inst_context,
                        ));
                    }
                }
            }

            // TODO type checks
        }

        src
    }

    fn check_inst(&mut self, arg: &ComponentInstantiation) {
        if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref())
            && matches!(
                symbol.found.kind,
                SymbolKind::Module(_) | SymbolKind::Interface(_)
            )
        {
            let parameters = symbol.found.kind.get_parameters();
            let definition = symbol.found.kind.get_definition().unwrap();

            let mut sig = InstanceSignature::new(symbol.found.id);

            // Push override parameters
            let params = self.get_overridden_params(arg);
            for x in parameters {
                if let Some(value) = params.get(&x.name) {
                    symbol_table::push_override(x.symbol, value.clone());
                    sig.add_param(x.name, value.value.clone());
                }
            }

            symbol_table::clear_evaluated_cache(&symbol.found.inner_namespace());

            if let SymbolKind::Module(x) = &symbol.found.kind {
                self.check_port_connection(arg, x);
            }

            match instance_history::push(sig) {
                Ok(true) => {
                    // Check expression with overridden parameters
                    if let Some(def) = definition_table::get(definition) {
                        match def {
                            Definition::Module(x) => {
                                let mut inst_context = self.inst_context.clone();
                                inst_context.push(arg.identifier.as_ref().into());
                                let mut analyzer = AnalyzerPass2Expression::new(inst_context);
                                analyzer.module_declaration(&x);
                                self.errors.append(&mut analyzer.get_errors());
                            }
                            Definition::Interface(x) => {
                                let mut inst_context = self.inst_context.clone();
                                inst_context.push(arg.identifier.as_ref().into());
                                let mut analyzer = AnalyzerPass2Expression::new(inst_context);
                                analyzer.interface_declaration(&x);
                                self.errors.append(&mut analyzer.get_errors());
                            }
                        }
                    }
                    instance_history::pop();
                }
                // Skip duplicated signature
                Ok(false) => (),
                Err(x) => self.inst_history_error(x, &arg.identifier.as_ref().into()),
            }

            symbol_table::clear_evaluated_cache(&symbol.found.inner_namespace());

            // Pop override parameters
            for x in parameters {
                if params.contains_key(&x.name) {
                    symbol_table::pop_override(x.symbol);
                }
            }
        }
    }

    fn get_overridden_params(&mut self, arg: &ComponentInstantiation) -> HashMap<StrId, Evaluated> {
        let mut ret = HashMap::new();

        let Ok(component_namespace) = symbol_table::resolve(arg.scoped_identifier.as_ref())
            .map(|x| x.found.inner_namespace())
        else {
            return ret;
        };

        let params = if let Some(ref x) = arg.component_instantiation_opt1 {
            if let Some(x) = &x.inst_parameter.inst_parameter_opt {
                x.inst_parameter_list.as_ref().into()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        for param in params {
            let name = param.identifier.identifier_token.token.text;

            let Ok(target) =
                symbol_table::resolve((param.identifier.as_ref(), &component_namespace))
                    .map(|x| x.found)
            else {
                ret.insert(name, Evaluated::create_unknown());
                continue;
            };

            let range: TokenRange = (&param).into();
            let value = if let Some(x) = &param.inst_parameter_item_opt {
                self.evaluate_connection(
                    Context::ParameterConnection,
                    &x.expression,
                    &target,
                    &[],
                    &range,
                )
            } else {
                let src: Expression = param.identifier.as_ref().into();
                self.evaluate_connection(Context::ParameterConnection, &src, &target, &[], &range)
            };

            ret.insert(name, value);
        }

        ret
    }

    fn check_port_connection(&mut self, arg: &ComponentInstantiation, module: &ModuleProperty) {
        let connections = if let Some(ref x) = arg.component_instantiation_opt2 {
            if let Some(ref x) = x.inst_port.inst_port_opt {
                x.inst_port_list.as_ref().into()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let mut ports = HashMap::new();
        for x in &module.ports {
            let name = x.name();
            let symbol = x.symbol();
            ports.insert(name, symbol);
        }

        for connect in connections {
            let token: TokenRange = (&connect).into();
            let dst = connect.identifier.identifier_token.token.text;

            match (connect.inst_port_item_opt, ports.get(&dst)) {
                (Some(src), Some(dst)) => {
                    self.evaluate_connection(
                        Context::PortConnection,
                        &src.expression,
                        dst,
                        &[],
                        &token,
                    );
                }
                (Some(src), _) => {
                    self.evaluate_expression(&src.expression, false);
                }
                (_, Some(dst)) => {
                    let src: Expression = connect.identifier.as_ref().into();
                    self.evaluate_connection(Context::PortConnection, &src, dst, &[], &token);
                }
                (_, _) => {
                    let src: Expression = connect.identifier.as_ref().into();
                    self.evaluate_expression(&src, false);
                }
            }
        }
    }
}

impl Handler for CheckExpression {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn is_if_expression(expression: &Expression) -> bool {
    !expression.if_expression.if_expression_list.is_empty()
}

fn is_defined_in_package(full_path: &[SymbolId]) -> bool {
    for path in full_path {
        let symbol = symbol_table::get(*path).unwrap();
        if matches!(symbol.kind, SymbolKind::Package(_)) {
            return true;
        }
    }

    let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();
    if let Some(parent) = symbol.get_parent() {
        if matches!(parent.kind, SymbolKind::Package(_)) {
            return true;
        } else {
            return is_defined_in_package(&[parent.id]);
        }
    }

    false
}

impl VerylGrammarTrait for CheckExpression {
    fn l_brace(&mut self, arg: &LBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && self.disable_block_beg.remove(&arg.l_brace_token.token.id)
        {
            self.disable = true;
        }

        Ok(())
    }

    fn r_brace(&mut self, arg: &RBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && self.disable_block_end.remove(&arg.r_brace_token.token.id)
        {
            self.disable = false;
        }

        Ok(())
    }

    fn if_expression(&mut self, arg: &IfExpression) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.if_expression_list {
                if is_if_expression(&x.expression) {
                    let range: TokenRange = x.expression.as_ref().into();
                    self.errors
                        .push(AnalyzerError::unenclosed_inner_if_expression(&range));
                }

                if is_if_expression(&x.expression0) {
                    let range: TokenRange = x.expression0.as_ref().into();
                    self.errors
                        .push(AnalyzerError::unenclosed_inner_if_expression(&range));
                }
            }
        }

        Ok(())
    }

    fn identifier_factor(&mut self, arg: &IdentifierFactor) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            let expid = arg.expression_identifier.as_ref();
            if let Ok(rr) = symbol_table::resolve(expid) {
                // Only generic const or globally visible identifier can be used as port default value
                if self.in_input_port_default_value {
                    let port_default_available = match &rr.found.kind {
                        SymbolKind::SystemFunction(_) => true,
                        SymbolKind::GenericParameter(x) => x
                            .bound
                            .resolve_proto_bound(&rr.found.namespace)
                            .map(|x| x.is_variable_type())
                            .unwrap_or(false),
                        _ => is_defined_in_package(&rr.full_path),
                    };

                    if !port_default_available {
                        let token: TokenRange = arg.expression_identifier.as_ref().into();
                        let kind_name = rr.found.kind.to_kind_name();

                        self.errors.push(AnalyzerError::invalid_factor(
                            Some(&rr.found.token.to_string()),
                            &kind_name,
                            &token,
                            &self.inst_context,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            if let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref()) {
                self.evaluate_connection(
                    Context::Assignment,
                    &arg.expression,
                    &dst.found,
                    &[],
                    &arg.into(),
                );
            } else {
                self.evaluate_expression(&arg.expression, false);
            }
        }

        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            match arg.identifier_statement_group.as_ref() {
                IdentifierStatementGroup::FunctionCall(x) => {
                    self.evaluate_expression(
                        &(arg.expression_identifier.as_ref(), x.function_call.as_ref()).into(),
                        false,
                    );
                }
                IdentifierStatementGroup::Assignment(x) => {
                    let token = arg.expression_identifier.identifier().token;
                    let (is_rhs_expression, is_connect_operation) =
                        if let Some(x) = connect_operation_table::get(&token) {
                            (matches!(x.rhs, ConnectOperand::Expression(_)), true)
                        } else {
                            (true, false)
                        };

                    if !is_connect_operation {
                        // Evaluate expressions in select of LHS
                        self.evaluate_expression(&arg.expression_identifier.as_ref().into(), false);
                    }

                    if is_connect_operation && !is_rhs_expression {
                        // RHS operand is modport so no checks will be skipped.
                        return Ok(());
                    }

                    if !is_connect_operation
                        && let Ok(dst) = symbol_table::resolve(arg.expression_identifier.as_ref())
                    {
                        let last_select = arg.expression_identifier.last_select();
                        self.evaluate_connection(
                            Context::Assignment,
                            &x.assignment.expression,
                            &dst.found,
                            &last_select,
                            &arg.into(),
                        );
                    } else {
                        // connect operation requires no compatibility check
                        self.evaluate_expression(&x.assignment.expression, false);
                    }
                }
            }
        }

        Ok(())
    }

    fn if_statement(&mut self, arg: &IfStatement) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            self.evaluate_expression(&arg.expression, false);

            // TODO type check

            for x in &arg.if_statement_list {
                self.evaluate_expression(&x.expression, false);

                // TODO type check
            }
        }

        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            for x in &arg.if_reset_statement_list {
                self.evaluate_expression(&x.expression, false);

                // TODO type check
            }
        }

        Ok(())
    }

    fn return_statement(&mut self, arg: &ReturnStatement) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            self.evaluate_expression(&arg.expression, false);

            // TODO type check
        }

        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            self.evaluate_expression(&arg.range.expression, false);

            // TODO type check

            if let Some(x) = &arg.range.range_opt {
                self.evaluate_expression(&x.expression, false);

                // TODO type check
            }

            if let Some(x) = &arg.for_statement_opt0 {
                self.evaluate_expression(&x.expression, false);

                // TODO type check
            }
        }

        Ok(())
    }

    fn case_statement(&mut self, arg: &CaseStatement) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            self.evaluate_expression(&arg.expression, false);
        }

        Ok(())
    }

    fn case_condition(&mut self, arg: &CaseCondition) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            let range_items: Vec<RangeItem> = arg.into();

            for x in range_items {
                let exp = self.evaluate_expression(&x.range.expression, false);

                // TODO type check

                if !exp.is_known_static() {
                    self.errors
                        .push(AnalyzerError::invalid_case_condition_non_elaborative(
                            &x.range.expression.as_ref().into(),
                        ));
                }

                if let Some(x) = &x.range.range_opt {
                    let exp = self.evaluate_expression(&x.expression, false);

                    // TODO type check

                    if !exp.is_known_static() {
                        self.errors
                            .push(AnalyzerError::invalid_case_condition_non_elaborative(
                                &x.expression.as_ref().into(),
                            ));
                    }
                }
            }
        }

        Ok(())
    }

    fn switch_condition(&mut self, arg: &SwitchCondition) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            let expressions: Vec<Expression> = arg.into();

            for x in expressions {
                self.evaluate_expression(&x, false);

                // TODO type check
            }
        }

        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            if let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref()) {
                self.evaluate_connection(
                    Context::Assignment,
                    &arg.expression,
                    &dst.found,
                    &[],
                    &arg.into(),
                );
            } else {
                self.evaluate_expression(&arg.expression, false);
            }
        }

        Ok(())
    }

    fn const_declaration(&mut self, arg: &ConstDeclaration) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            if let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref()) {
                self.evaluate_connection(
                    Context::Assignment,
                    &arg.expression,
                    &dst.found,
                    &[],
                    &arg.into(),
                );
            } else {
                let type_expression =
                    matches!(*arg.const_declaration_group, ConstDeclarationGroup::Type(_));
                self.evaluate_expression(&arg.expression, type_expression);
            }
        }

        Ok(())
    }

    fn assign_declaration(&mut self, arg: &AssignDeclaration) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            if let AssignDestination::HierarchicalIdentifier(x) = arg.assign_destination.as_ref()
                && let Ok(dst) = symbol_table::resolve(x.hierarchical_identifier.as_ref())
            {
                let last_select = x.hierarchical_identifier.last_select();
                self.evaluate_connection(
                    Context::Assignment,
                    &arg.expression,
                    &dst.found,
                    &last_select,
                    &arg.into(),
                );
            } else {
                // TODO check concatenation
                self.evaluate_expression(&arg.expression, false);
            }
        }

        Ok(())
    }

    fn enum_item(&mut self, arg: &EnumItem) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
            && let Some(x) = &arg.enum_item_opt
        {
            self.evaluate_expression(&x.expression, false);

            // TODO type check
        }

        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if !self.disable && matches!(self.point, HandlerPoint::Before) {
            self.check_inst(&arg.component_instantiation);
        }
        Ok(())
    }

    fn bind_declaration(&mut self, arg: &BindDeclaration) -> Result<(), ParolError> {
        if !self.disable
            && matches!(self.point, HandlerPoint::Before)
            && let Ok(_) = symbol_table::resolve(arg.scoped_identifier.as_ref())
        {
            self.check_inst(&arg.component_instantiation);
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            if let Some(x) = &arg.with_parameter_item_opt {
                let type_expression = matches!(
                    *arg.with_parameter_item_group0,
                    WithParameterItemGroup0::Type(_)
                );
                if !type_expression && let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref())
                {
                    self.evaluate_connection(
                        Context::ParameterConnection,
                        &x.expression,
                        &dst.found,
                        &[],
                        &arg.into(),
                    );
                } else {
                    self.evaluate_expression(&x.expression, type_expression);
                }
                // TODO type check
            } else if !self.in_proto {
                self.errors.push(AnalyzerError::missing_default_argument(
                    &arg.identifier.identifier_token.token.to_string(),
                    &arg.identifier.as_ref().into(),
                ));
            }
        }

        Ok(())
    }

    fn with_generic_parameter_item(
        &mut self,
        arg: &WithGenericParameterItem,
    ) -> Result<(), ParolError> {
        if !self.disable
            && matches!(&*arg.generic_bound, GenericBound::Type(_))
            && let Some(x) = &arg.with_generic_parameter_item_opt
            && let HandlerPoint::Before = self.point
        {
            let expression: Expression = match &*x.with_generic_argument_item {
                WithGenericArgumentItem::GenericArgIdentifier(x) => {
                    x.generic_arg_identifier.as_ref().into()
                }
                WithGenericArgumentItem::FixedType(x) => x.fixed_type.as_ref().into(),
                WithGenericArgumentItem::Number(x) => x.number.as_ref().into(),
                WithGenericArgumentItem::BooleanLiteral(x) => x.boolean_literal.as_ref().into(),
            };
            self.evaluate_expression(&expression, true);
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if !self.disable {
            match self.point {
                HandlerPoint::Before => {
                    if let PortDeclarationItemGroup::PortTypeConcrete(x) =
                        &*arg.port_declaration_item_group
                    {
                        let port_type = &x.port_type_concrete;
                        self.port_direction = Some(port_type.direction.as_ref().into());

                        if let Some(x) = &port_type.port_type_concrete_opt0 {
                            let default_value = &x.port_default_value.expression;
                            if let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref()) {
                                let range = port_type.as_ref().into();
                                self.evaluate_connection(
                                    Context::Assignment,
                                    default_value,
                                    &dst.found,
                                    &[],
                                    &range,
                                );
                            } else {
                                self.evaluate_expression(default_value, false);
                            }
                        }
                    }
                }
                HandlerPoint::After => self.port_direction = None,
            }
        }
        Ok(())
    }

    fn port_default_value(&mut self, _arg: &PortDefaultValue) -> Result<(), ParolError> {
        if !self.disable {
            match self.point {
                HandlerPoint::Before => {
                    self.in_input_port_default_value =
                        matches!(self.port_direction.unwrap(), Direction::Input)
                }
                HandlerPoint::After => self.in_input_port_default_value = false,
            }
        }

        Ok(())
    }

    fn generate_if_declaration(&mut self, arg: &GenerateIfDeclaration) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            let exp = self.evaluate_expression(&arg.expression, false);

            let mut already_enabled = false;
            if let Some(value) = exp.get_value() {
                if value == 0.into() {
                    let beg = arg.generate_named_block.l_brace.id();
                    let end = arg.generate_named_block.r_brace.id();
                    self.disable_block_beg.insert(beg);
                    self.disable_block_end.insert(end);
                } else {
                    already_enabled = true;
                }
            }

            // TODO type check

            for x in &arg.generate_if_declaration_list {
                let exp = self.evaluate_expression(&x.expression, false);

                if let Some(value) = exp.get_value() {
                    if value == 0.into() || already_enabled {
                        let beg = x.generate_optional_named_block.l_brace.id();
                        let end = x.generate_optional_named_block.r_brace.id();
                        self.disable_block_beg.insert(beg);
                        self.disable_block_end.insert(end);
                    } else {
                        already_enabled = true;
                    }
                }

                // TODO type check
            }

            if let Some(x) = &arg.generate_if_declaration_opt
                && already_enabled
            {
                let beg = x.generate_optional_named_block.l_brace.id();
                let end = x.generate_optional_named_block.r_brace.id();
                self.disable_block_beg.insert(beg);
                self.disable_block_end.insert(end);
            }
        }

        Ok(())
    }

    fn generate_for_declaration(&mut self, arg: &GenerateForDeclaration) -> Result<(), ParolError> {
        if !self.disable
            && let HandlerPoint::Before = self.point
        {
            self.evaluate_expression(&arg.range.expression, false);

            // TODO type check

            if let Some(x) = &arg.range.range_opt {
                self.evaluate_expression(&x.expression, false);

                // TODO type check
            }

            if let Some(x) = &arg.generate_for_declaration_opt0 {
                self.evaluate_expression(&x.expression, false);

                // TODO type check
            }
        }

        Ok(())
    }

    fn proto_module_declaration(
        &mut self,
        _arg: &ProtoModuleDeclaration,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_proto = true,
            HandlerPoint::After => self.in_proto = false,
        }
        Ok(())
    }

    fn proto_interface_declaration(
        &mut self,
        _arg: &ProtoInterfaceDeclaration,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_proto = true,
            HandlerPoint::After => self.in_proto = false,
        }
        Ok(())
    }
}
