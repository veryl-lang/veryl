use crate::analyzer_error::AnalyzerError;
use crate::evaluator::{Evaluated, Evaluator};
use crate::symbol::{Direction, GenericBoundKind, SymbolId, SymbolKind};
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::TokenRange;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckExpression<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    case_condition_depth: usize,
    evaluator: Evaluator,
    in_inst_declaration: bool,
    port_direction: Option<Direction>,
    in_input_port_default_value: bool,
}

impl<'a> CheckExpression<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
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
    fn case_condition(&mut self, _arg: &CaseCondition) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.case_condition_depth += 1,
            HandlerPoint::After => self.case_condition_depth -= 1,
        }
        Ok(())
    }

    fn expression(&mut self, arg: &Expression) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.case_condition_depth >= 1 {
                let result = matches!(self.evaluator.expression(arg), Evaluated::Variable { .. });
                if result {
                    self.errors
                        .push(AnalyzerError::invalid_case_condition_non_elaborative(
                            self.text,
                            &arg.into(),
                        ));
                }
            }
        }
        Ok(())
    }

    fn identifier_factor(&mut self, arg: &IdentifierFactor) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let expid = arg.expression_identifier.as_ref();
            if let Ok(rr) = symbol_table::resolve(expid) {
                let identifier = rr.found.token.to_string();
                let token: TokenRange = arg.expression_identifier.as_ref().into();
                let kind_name = rr.found.kind.to_kind_name();

                // Closure to delay AnalyzerError constructor to actual error
                let error =
                    || AnalyzerError::invalid_factor(&identifier, &kind_name, self.text, &token);

                match rr.found.kind {
                    SymbolKind::Function(_) | SymbolKind::ModportFunctionMember(_) => {
                        if arg.identifier_factor_opt.is_none() {
                            self.errors.push(error());
                        }
                    }
                    SymbolKind::SystemFunction => {
                        if arg.identifier_factor_opt.is_none() {
                            self.errors.push(error());
                        }
                    }
                    // instance can be used as factor in inst_declaration
                    SymbolKind::Instance(_) if self.in_inst_declaration => (),
                    SymbolKind::Module(_)
                    | SymbolKind::ProtoModule(_)
                    | SymbolKind::Interface(_)
                    | SymbolKind::Instance(_)
                    | SymbolKind::Block
                    | SymbolKind::Package(_)
                    | SymbolKind::Modport(_)
                    | SymbolKind::Namespace
                    | SymbolKind::ClockDomain
                    | SymbolKind::Test(_) => {
                        self.errors.push(error());
                    }
                    SymbolKind::Port(x) => {
                        if !self.in_inst_declaration {
                            match x.direction {
                                Direction::Interface | Direction::Modport => {
                                    // modport and interface direction can be used as factor in inst_declaration
                                    self.errors.push(error());
                                }
                                _ => {}
                            }
                        } else if self.in_input_port_default_value {
                            // port cannot be used for port default value
                            self.errors.push(error());
                        }
                    }
                    SymbolKind::Parameter(_)
                    | SymbolKind::EnumMember(_)
                    | SymbolKind::StructMember(_)
                    | SymbolKind::UnionMember(_)
                        if self.in_input_port_default_value =>
                    {
                        if !is_defined_in_package(&rr.full_path) {
                            self.errors.push(error());
                        }
                    }
                    SymbolKind::GenericParameter(x) if self.in_input_port_default_value => {
                        if !matches!(x.bound, GenericBoundKind::Const) {
                            self.errors.push(error());
                        }
                    }
                    _ if self.in_input_port_default_value => {
                        self.errors.push(error());
                    }
                    _ => {}
                }
            }

            if arg.identifier_factor_opt.is_some() {
                // Must be a function call
                let expid = arg.expression_identifier.as_ref();
                if let Ok(rr) = symbol_table::resolve(expid) {
                    let is_function = match &rr.found.kind {
                        SymbolKind::Function(_)
                        | SymbolKind::SystemVerilog
                        | SymbolKind::ModportFunctionMember(..)
                        | SymbolKind::SystemFunction => true,
                        SymbolKind::GenericInstance(x) => {
                            let base = symbol_table::get(x.base).unwrap();
                            matches!(
                                base.kind,
                                SymbolKind::Function(_)
                                    | SymbolKind::SystemVerilog
                                    | SymbolKind::ModportFunctionMember(..)
                                    | SymbolKind::SystemFunction
                            )
                        }
                        _ => false,
                    };

                    if !is_function {
                        let identifier = rr.found.token.to_string();
                        let token: TokenRange = arg.expression_identifier.as_ref().into();
                        self.errors.push(AnalyzerError::call_non_function(
                            &identifier,
                            &rr.found.kind.to_kind_name(),
                            self.text,
                            &token,
                        ));
                    } else if self.in_input_port_default_value
                        && !is_defined_in_package(&rr.full_path)
                    {
                        self.errors.push(AnalyzerError::invalid_factor(
                            &rr.found.token.to_string(),
                            &rr.found.kind.to_kind_name(),
                            self.text,
                            &arg.expression_identifier.as_ref().into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, _arg: &InstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_inst_declaration = true,
            HandlerPoint::After => self.in_inst_declaration = false,
        }
        Ok(())
    }

    fn port_type_concrete(&mut self, arg: &PortTypeConcrete) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.port_direction = Some(arg.direction.as_ref().into()),
            HandlerPoint::After => self.port_direction = None,
        }
        Ok(())
    }

    /// Semantic action for non-terminal 'PortDefaultValue'
    fn port_default_value(&mut self, _arg: &PortDefaultValue) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_input_port_default_value =
                    matches!(self.port_direction.unwrap(), Direction::Input)
            }
            HandlerPoint::After => self.in_input_port_default_value = false,
        }
        Ok(())
    }
}
