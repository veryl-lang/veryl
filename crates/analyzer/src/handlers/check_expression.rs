use crate::analyzer::AnalyzerPass2Expression;
use crate::analyzer_error::AnalyzerError;
use crate::definition_table::{self, Definition};
use crate::evaluator::{Evaluated, EvaluatedError, EvaluatedType, EvaluatedValue, Evaluator};
use crate::symbol::{Direction, GenericBoundKind, ModuleProperty, Symbol, SymbolId, SymbolKind};
use crate::symbol_table;
use std::collections::{HashMap, HashSet};
use veryl_parser::resource_table::StrId;
use veryl_parser::resource_table::TokenId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::TokenRange;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};
use veryl_parser::ParolError;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct InstanceSignature {
    symbol: SymbolId,
    params: Vec<(StrId, EvaluatedValue)>,
}

impl InstanceSignature {
    fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            params: Vec::new(),
        }
    }

    fn add_param(&mut self, id: StrId, value: EvaluatedValue) {
        self.params.push((id, value));
    }

    fn normalize(&mut self) {
        self.params.sort();
    }
}

#[derive(Default)]
pub struct InstanceHistory {
    pub depth_limit: usize,
    pub total_limit: usize,
    pub hierarchy: Vec<InstanceSignature>,
    full: HashSet<InstanceSignature>,
}

impl InstanceHistory {
    fn push(&mut self, mut sig: InstanceSignature) -> Result<bool, InstanceHistoryError> {
        sig.normalize();
        if self.hierarchy.len() > self.depth_limit {
            return Err(InstanceHistoryError::ExceedDepthLimit);
        }
        if self.full.len() > self.total_limit {
            return Err(InstanceHistoryError::ExceedTotalLimit);
        }
        if self.hierarchy.iter().any(|x| *x == sig)
            && sig.params.iter().all(|x| x.1.get_value().is_some())
        {
            return Err(InstanceHistoryError::InfiniteRecursion);
        }
        if self.full.contains(&sig) {
            Ok(false)
        } else {
            self.hierarchy.push(sig.clone());
            self.full.insert(sig);
            Ok(true)
        }
    }

    fn pop(&mut self) {
        self.hierarchy.pop();
    }
}

#[derive(Debug)]
pub enum InstanceHistoryError {
    ExceedDepthLimit,
    ExceedTotalLimit,
    InfiniteRecursion,
}

impl InstanceHistoryError {}

pub struct CheckExpression<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    evaluator: Evaluator,
    in_inst_declaration: bool,
    port_direction: Option<Direction>,
    in_input_port_default_value: bool,
    disable: bool,
    disable_block_beg: HashSet<TokenId>,
    disable_block_end: HashSet<TokenId>,
    inst_context: Vec<TokenRange>,
    inst_history: &'a mut InstanceHistory,
}

impl<'a> CheckExpression<'a> {
    pub fn new(
        text: &'a str,
        inst_context: Vec<TokenRange>,
        inst_history: &'a mut InstanceHistory,
    ) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::default(),
            evaluator: Evaluator::default(),
            in_inst_declaration: false,
            port_direction: None,
            in_input_port_default_value: false,
            disable: false,
            disable_block_beg: HashSet::new(),
            disable_block_end: HashSet::new(),
            inst_context,
            inst_history,
        }
    }

    fn evaluated_error(&mut self, errors: &[EvaluatedError]) {
        for e in errors {
            self.errors.push(AnalyzerError::evaluated_error(
                self.text,
                e,
                &self.inst_context,
            ));
        }
    }

    fn inst_history_error(&mut self, error: InstanceHistoryError, token: &TokenRange) {
        let error = match error {
            InstanceHistoryError::ExceedDepthLimit => {
                AnalyzerError::exceed_limit("hierarchy depth limit", self.text, token)
            }
            InstanceHistoryError::ExceedTotalLimit => {
                AnalyzerError::exceed_limit("total instance limit", self.text, token)
            }
            InstanceHistoryError::InfiniteRecursion => {
                AnalyzerError::infinite_recursion(self.text, token)
            }
        };
        self.errors.push(error);
    }

    fn check_assignment(
        &mut self,
        src: &Evaluated,
        dst: &Symbol,
        dst_last_select: &[Select],
        token: &TokenRange,
    ) {
        if let Some(dst_type) = dst.kind.get_type() {
            if src.r#type != EvaluatedType::Unknown {
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
                        &format!("{}-D array", src_array_dim),
                        &format!("{}-D array", dst_array_dim),
                        self.text,
                        token,
                        &self.inst_context,
                    ));
                }

                if dst_type.kind.is_2state() && src.is_4state() {
                    self.errors.push(AnalyzerError::mismatch_assignment(
                        "4-state value",
                        "2-state variable",
                        self.text,
                        token,
                        &self.inst_context,
                    ));
                }

                // TODO type checks
            }
        }
    }

    fn get_overridden_params(&mut self, arg: &InstDeclaration) -> HashMap<StrId, Evaluated> {
        let mut ret = HashMap::new();

        let params = if let Some(x) = &arg.inst_declaration_opt0 {
            if let Some(x) = &x.inst_parameter.inst_parameter_opt {
                x.inst_parameter_list.as_ref().into()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        for param in params {
            let value = if let Some(x) = &param.inst_parameter_item_opt {
                self.evaluator.expression(&x.expression)
            } else if let Ok(symbol) = symbol_table::resolve(param.identifier.as_ref()) {
                symbol.found.evaluate()
            } else {
                Evaluated::create_unknown()
            };

            let name = param.identifier.identifier_token.token.text;
            ret.insert(name, value);
        }

        ret
    }

    fn check_port_connection(&mut self, _arg: &InstDeclaration, _module: &ModuleProperty) {
        // TODO check port connection
        //
    }
}

impl Handler for CheckExpression<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
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

impl VerylGrammarTrait for CheckExpression<'_> {
    fn l_brace(&mut self, arg: &LBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.disable_block_beg.remove(&arg.l_brace_token.token.id) {
                self.disable = true;
            }
        }

        Ok(())
    }

    fn r_brace(&mut self, arg: &RBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.disable_block_end.remove(&arg.r_brace_token.token.id) {
                self.disable = false;
            }
        }

        Ok(())
    }

    fn identifier_factor(&mut self, arg: &IdentifierFactor) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let expid = arg.expression_identifier.as_ref();
                if let Ok(rr) = symbol_table::resolve(expid) {
                    // Only generic const or globally visible identifier can be used as port default value
                    if self.in_input_port_default_value {
                        let port_default_available = match &rr.found.kind {
                            SymbolKind::SystemFunction => true,
                            SymbolKind::GenericParameter(x) => {
                                matches!(x.bound, GenericBoundKind::Const)
                            }
                            _ => is_defined_in_package(&rr.full_path),
                        };

                        if !port_default_available {
                            let identifier = rr.found.token.to_string();
                            let token: TokenRange = arg.expression_identifier.as_ref().into();
                            let kind_name = rr.found.kind.to_kind_name();

                            self.errors.push(AnalyzerError::invalid_factor(
                                &identifier,
                                &kind_name,
                                self.text,
                                &token,
                                &self.inst_context,
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                if let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref()) {
                    self.check_assignment(&exp, &dst.found, &[], &arg.into());
                }
            }
        }

        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                match arg.identifier_statement_group.as_ref() {
                    IdentifierStatementGroup::FunctionCall(_) => {
                        // TODO function check
                    }
                    IdentifierStatementGroup::Assignment(x) => {
                        let exp = self.evaluator.expression(&x.assignment.expression);
                        self.evaluated_error(&exp.errors);

                        if let Ok(dst) = symbol_table::resolve(arg.expression_identifier.as_ref()) {
                            let dst_last_select = arg.expression_identifier.last_select();
                            self.check_assignment(&exp, &dst.found, &dst_last_select, &arg.into());
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn if_statement(&mut self, arg: &IfStatement) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                // TODO type check

                for x in &arg.if_statement_list {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }
            }
        }

        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                for x in &arg.if_reset_statement_list {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }
            }
        }

        Ok(())
    }

    fn return_statement(&mut self, arg: &ReturnStatement) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                // TODO type check
            }
        }

        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.range.expression);
                self.evaluated_error(&exp.errors);

                // TODO type check

                if let Some(x) = &arg.range.range_opt {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }

                if let Some(x) = &arg.for_statement_opt {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }
            }
        }

        Ok(())
    }

    fn case_statement(&mut self, arg: &CaseStatement) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);
            }
        }

        Ok(())
    }

    fn case_condition(&mut self, arg: &CaseCondition) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let range_items: Vec<RangeItem> = arg.into();

                for x in range_items {
                    let exp = self.evaluator.expression(&x.range.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check

                    if !exp.is_known_static() {
                        self.errors
                            .push(AnalyzerError::invalid_case_condition_non_elaborative(
                                self.text,
                                &x.range.expression.as_ref().into(),
                            ));
                    }

                    if let Some(x) = &x.range.range_opt {
                        let exp = self.evaluator.expression(&x.expression);
                        self.evaluated_error(&exp.errors);

                        // TODO type check

                        if !exp.is_known_static() {
                            self.errors.push(
                                AnalyzerError::invalid_case_condition_non_elaborative(
                                    self.text,
                                    &x.expression.as_ref().into(),
                                ),
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn switch_condition(&mut self, arg: &SwitchCondition) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let expressions: Vec<Expression> = arg.into();

                for x in expressions {
                    let exp = self.evaluator.expression(&x);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }
            }
        }

        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                if let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref()) {
                    self.check_assignment(&exp, &dst.found, &[], &arg.into());
                }
            }
        }

        Ok(())
    }

    fn const_declaration(&mut self, arg: &ConstDeclaration) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                if let Ok(dst) = symbol_table::resolve(arg.identifier.as_ref()) {
                    self.check_assignment(&exp, &dst.found, &[], &arg.into());
                }
            }
        }

        Ok(())
    }

    fn assign_declaration(&mut self, arg: &AssignDeclaration) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                if let Ok(dst) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
                    let dst_last_select = arg.hierarchical_identifier.last_select();
                    self.check_assignment(&exp, &dst.found, &dst_last_select, &arg.into());
                }
            }
        }

        Ok(())
    }

    fn enum_item(&mut self, arg: &EnumItem) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                if let Some(x) = &arg.enum_item_opt {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }
            }
        }

        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if !self.disable {
            match self.point {
                HandlerPoint::Before => {
                    self.in_inst_declaration = true;

                    if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                        if matches!(
                            symbol.found.kind,
                            SymbolKind::Module(_) | SymbolKind::Interface(_)
                        ) {
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

                            match self.inst_history.push(sig) {
                                Ok(true) => {
                                    // Check expression with overridden parameters
                                    let def = definition_table::get(definition).unwrap();
                                    if let Definition::Module { text, decl } = def {
                                        let mut inst_context = self.inst_context.clone();
                                        inst_context.push(arg.identifier.as_ref().into());
                                        let mut analyzer = AnalyzerPass2Expression::new(
                                            &text,
                                            inst_context,
                                            self.inst_history,
                                        );
                                        analyzer.module_declaration(&decl);
                                        self.errors.append(&mut analyzer.get_errors());
                                    }
                                    self.inst_history.pop();
                                }
                                // Skip duplicated signature
                                Ok(false) => (),
                                Err(x) => {
                                    self.inst_history_error(x, &arg.identifier.as_ref().into())
                                }
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
                }
                HandlerPoint::After => self.in_inst_declaration = false,
            }
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                // TODO type check
            }
        }

        Ok(())
    }

    fn port_type_concrete(&mut self, arg: &PortTypeConcrete) -> Result<(), ParolError> {
        if !self.disable {
            match self.point {
                HandlerPoint::Before => {
                    self.port_direction = Some(arg.direction.as_ref().into());

                    if let Some(x) = &arg.port_type_concrete_opt0 {
                        let exp = self.evaluator.expression(&x.port_default_value.expression);
                        self.evaluated_error(&exp.errors);

                        // TODO type check
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
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.expression);
                self.evaluated_error(&exp.errors);

                let mut already_enabled = false;
                if let Some(value) = exp.get_value() {
                    if value == 0 {
                        let beg = arg.generate_named_block.l_brace.as_ref().into();
                        let end = arg.generate_named_block.r_brace.as_ref().into();
                        self.disable_block_beg.insert(beg);
                        self.disable_block_end.insert(end);
                    } else {
                        already_enabled = true;
                    }
                }

                // TODO type check

                for x in &arg.generate_if_declaration_list {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    if let Some(value) = exp.get_value() {
                        if value == 0 || already_enabled {
                            let beg = x.generate_optional_named_block.l_brace.as_ref().into();
                            let end = x.generate_optional_named_block.r_brace.as_ref().into();
                            self.disable_block_beg.insert(beg);
                            self.disable_block_end.insert(end);
                        } else {
                            already_enabled = true;
                        }
                    }

                    // TODO type check
                }

                if let Some(x) = &arg.generate_if_declaration_opt {
                    if already_enabled {
                        let beg = x.generate_optional_named_block.l_brace.as_ref().into();
                        let end = x.generate_optional_named_block.r_brace.as_ref().into();
                        self.disable_block_beg.insert(beg);
                        self.disable_block_end.insert(end);
                    }
                }
            }
        }

        Ok(())
    }

    fn generate_for_declaration(&mut self, arg: &GenerateForDeclaration) -> Result<(), ParolError> {
        if !self.disable {
            if let HandlerPoint::Before = self.point {
                let exp = self.evaluator.expression(&arg.range.expression);
                self.evaluated_error(&exp.errors);

                // TODO type check

                if let Some(x) = &arg.range.range_opt {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }

                if let Some(x) = &arg.generate_for_declaration_opt {
                    let exp = self.evaluator.expression(&x.expression);
                    self.evaluated_error(&exp.errors);

                    // TODO type check
                }
            }
        }

        Ok(())
    }
}
