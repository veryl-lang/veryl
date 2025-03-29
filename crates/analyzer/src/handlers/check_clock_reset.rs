use crate::analyzer_error::AnalyzerError;
use crate::evaluator::{EvaluatedValue, Evaluator};
use crate::symbol::{SymbolKind, Type, TypeKind};
use crate::symbol_table;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckClockReset {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    in_always_ff: bool,
    in_if_reset: bool,
    if_reset_brace: usize,
    n_of_select: usize,
    has_default_clock: bool,
    has_default_reset: bool,
    evaluator: Evaluator,
}

impl CheckClockReset {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckClockReset {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckClockReset {
    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Ok(found) = symbol_table::resolve(arg.identifier.as_ref()) {
                    if let SymbolKind::Module(x) = found.found.kind {
                        self.has_default_clock = x.default_clock.is_some();
                        self.has_default_reset = x.default_reset.is_some();
                    }
                }
            }
            HandlerPoint::After => {
                self.has_default_clock = false;
                self.has_default_reset = false;
            }
        }
        Ok(())
    }

    fn l_brace(&mut self, _arg: &LBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_reset {
                self.if_reset_brace += 1;
            }
        }
        Ok(())
    }

    fn r_brace(&mut self, _arg: &RBrace) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_reset {
                self.if_reset_brace -= 1;
                if self.if_reset_brace == 0 {
                    self.in_if_reset = false;
                }
            }
        }
        Ok(())
    }

    fn if_reset(&mut self, _arg: &IfReset) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.in_if_reset = true;
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                //  check if clock signal exists
                if !(self.has_default_clock || arg.has_explicit_clock()) {
                    self.errors
                        .push(AnalyzerError::missing_clock_signal(&arg.into()))
                }

                // Check first if_reset when reset signel exists
                if arg.has_explicit_reset() && !arg.has_if_reset() {
                    self.errors
                        .push(AnalyzerError::missing_if_reset(&arg.into()));
                }

                self.in_always_ff = true;
            }
            HandlerPoint::After => {
                // Check reset signal when if_reset exists
                let missing_reset =
                    arg.has_if_reset() && !(self.has_default_reset || arg.has_explicit_reset());
                if missing_reset {
                    self.errors
                        .push(AnalyzerError::missing_reset_signal(&arg.into()));
                }

                self.in_always_ff = false;
            }
        }
        Ok(())
    }

    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) -> Result<(), ParolError> {
        fn is_valid_clock(x: Type, n_of_selected: usize) -> bool {
            let n_of_selectable = x.width.len() + x.array.len();
            match x.kind {
                TypeKind::Clock | TypeKind::ClockPosedge | TypeKind::ClockNegedge => {
                    n_of_selectable == n_of_selected
                }
                _ => false,
            }
        }

        match self.point {
            HandlerPoint::Before => self.n_of_select = 0,
            HandlerPoint::After => {
                if let Ok(found) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
                    let symbol = found.found;
                    let valid_clock = match symbol.kind {
                        SymbolKind::Port(x) => is_valid_clock(x.r#type, self.n_of_select),
                        SymbolKind::Variable(x) => is_valid_clock(x.r#type, self.n_of_select),
                        SymbolKind::ModportVariableMember(x) => {
                            let symbol = symbol_table::get(x.variable).unwrap();
                            if let SymbolKind::Variable(x) = symbol.kind {
                                is_valid_clock(x.r#type, self.n_of_select)
                            } else {
                                false
                            }
                        }
                        _ => false,
                    };

                    if !valid_clock {
                        let token = &arg
                            .hierarchical_identifier
                            .identifier
                            .identifier_token
                            .token;
                        self.errors.push(AnalyzerError::invalid_clock(
                            &token.to_string(),
                            &arg.hierarchical_identifier.as_ref().into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) -> Result<(), ParolError> {
        fn is_valid_reset(x: Type, n_of_selected: usize) -> bool {
            let n_of_selectable = x.width.len() + x.array.len();
            match x.kind {
                TypeKind::Reset
                | TypeKind::ResetAsyncHigh
                | TypeKind::ResetAsyncLow
                | TypeKind::ResetSyncHigh
                | TypeKind::ResetSyncLow => n_of_selectable == n_of_selected,
                _ => false,
            }
        }

        match self.point {
            HandlerPoint::Before => self.n_of_select = 0,
            HandlerPoint::After => {
                if let Ok(found) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
                    let symbol = found.found;
                    let valid_reset = match symbol.kind {
                        SymbolKind::Port(x) => is_valid_reset(x.r#type, self.n_of_select),
                        SymbolKind::Variable(x) => is_valid_reset(x.r#type, self.n_of_select),
                        SymbolKind::ModportVariableMember(x) => {
                            let symbol = symbol_table::get(x.variable).unwrap();
                            if let SymbolKind::Variable(x) = symbol.kind {
                                is_valid_reset(x.r#type, self.n_of_select)
                            } else {
                                false
                            }
                        }
                        _ => false,
                    };

                    if !valid_reset {
                        let token = &arg
                            .hierarchical_identifier
                            .identifier
                            .identifier_token
                            .token;
                        self.errors.push(AnalyzerError::invalid_reset(
                            &token.to_string(),
                            &arg.hierarchical_identifier.as_ref().into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn select(&mut self, _arg: &Select) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.n_of_select += 1;
        }
        Ok(())
    }

    fn dot(&mut self, _arg: &Dot) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.n_of_select = 0;
        }
        Ok(())
    }

    fn assignment(&mut self, arg: &Assignment) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_reset {
                // Check to see right hand side of reset is const evaluable
                match self.evaluator.expression(&arg.expression).value {
                    EvaluatedValue::UnknownStatic | EvaluatedValue::Fixed(_) => (),
                    _ => {
                        self.errors
                            .push(AnalyzerError::invalid_reset_non_elaborative(
                                &arg.expression.as_ref().into(),
                            ));
                    }
                }
            }
        }
        Ok(())
    }

    fn expression12(&mut self, arg: &Expression12) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Some(x) = &arg.expression12_opt {
                let src = self.evaluator.expression13(&arg.expression13);
                match x.casting_type.as_ref() {
                    CastingType::Clock(_)
                    | CastingType::ClockPosedge(_)
                    | CastingType::ClockNegedge(_) => {
                        if !src.is_clock() {
                            self.errors.push(AnalyzerError::invalid_cast(
                                "non-clock type",
                                "clock type",
                                &arg.into(),
                            ));
                        }
                    }
                    CastingType::Reset(_)
                    | CastingType::ResetAsyncHigh(_)
                    | CastingType::ResetAsyncLow(_)
                    | CastingType::ResetSyncHigh(_)
                    | CastingType::ResetSyncLow(_) => {
                        if !src.is_reset() {
                            self.errors.push(AnalyzerError::invalid_cast(
                                "non-reset type",
                                "reset type",
                                &arg.into(),
                            ));
                        }
                    }
                    _ => (),
                }
            }
        }
        Ok(())
    }
}
