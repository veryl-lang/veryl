use crate::analyzer_error::AnalyzerError;
use crate::symbol::{ProtoIncompatible, SymbolKind};
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckProto<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckProto<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl Handler for CheckProto<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckProto<'_> {
    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Some(ref x) = arg.module_declaration_opt1 {
                if let Ok(symbol) = symbol_table::resolve(x.scoped_identifier.as_ref()) {
                    if let SymbolKind::ProtoModule(proto) = symbol.found.kind {
                        if let Ok(module) = symbol_table::resolve(arg.identifier.as_ref()) {
                            if let SymbolKind::Module(module) = module.found.kind {
                                let errors = proto.check_compat(&module);
                                for error in errors {
                                    let cause = match error {
                                        ProtoIncompatible::MissingParam(x) => {
                                            format!("parameter {x} is missing")
                                        }
                                        ProtoIncompatible::MissingPort(x) => {
                                            format!("port {x} is missing")
                                        }
                                        ProtoIncompatible::UnnecessaryParam(x) => {
                                            format!("parameter {x} is unnecessary")
                                        }
                                        ProtoIncompatible::UnnecessaryPort(x) => {
                                            format!("port {x} is unnecessary")
                                        }
                                        ProtoIncompatible::IncompatibleParam(x) => {
                                            format!("parameter {x} has incompatible type")
                                        }
                                        ProtoIncompatible::IncompatiblePort(x) => {
                                            format!("port {x} has incompatible type")
                                        }
                                    };
                                    self.errors.push(AnalyzerError::incompat_proto(
                                        &arg.identifier.identifier_token.to_string(),
                                        &symbol.found.token.to_string(),
                                        &cause,
                                        self.text,
                                        &arg.identifier.identifier_token.token.into(),
                                    ));
                                }
                            }
                        }
                    } else {
                        self.errors.push(AnalyzerError::mismatch_type(
                            &symbol.found.token.to_string(),
                            "module prototype",
                            &symbol.found.kind.to_kind_name(),
                            self.text,
                            &x.scoped_identifier.identifier().token.into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
