use crate::analyzer_error::AnalyzerError;
use crate::msb_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::Type as SymType;
use crate::symbol::{Direction, SymbolKind, TypeKind};
use crate::symbol_path::{SymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckMsbLsb<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    identifier_path: Vec<SymbolPathNamespace>,
    select_dimension: Vec<usize>,
    in_expression_identifier: bool,
    in_select: bool,
}

impl<'a> CheckMsbLsb<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            identifier_path: Vec::new(),
            select_dimension: Vec::new(),
            in_expression_identifier: false,
            in_select: false,
        }
    }
}

impl Handler for CheckMsbLsb<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn trace_type(r#type: &SymType, namespace: &Namespace) -> Vec<(SymType, Option<SymbolKind>)> {
    let mut ret = vec![(r#type.clone(), None)];
    if let TypeKind::UserDefined(ref x) = r#type.kind {
        if let Ok(symbol) = symbol_table::resolve((&SymbolPath::new(x), namespace)) {
            ret.last_mut().unwrap().1 = Some(symbol.found.kind.clone());
            if let SymbolKind::TypeDef(ref x) = symbol.found.kind {
                ret.append(&mut trace_type(&x.r#type, namespace));
            }
        }
    }
    ret
}

impl VerylGrammarTrait for CheckMsbLsb<'_> {
    fn lsb(&mut self, arg: &Lsb) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !(self.in_expression_identifier && self.in_select) {
                self.errors.push(AnalyzerError::invalid_lsb(
                    self.text,
                    &arg.lsb_token.token.into(),
                ));
            }
        }
        Ok(())
    }

    fn msb(&mut self, arg: &Msb) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_expression_identifier && self.in_select {
                let resolved = if let Ok(x) =
                    symbol_table::resolve(self.identifier_path.last().unwrap().clone())
                {
                    let namespace = &x.found.namespace;

                    let via_interface = x.full_path.iter().any(|path| {
                        let symbol = symbol_table::get(*path).unwrap();
                        match symbol.kind {
                            SymbolKind::Port(x) => {
                                matches!(x.direction, Direction::Interface | Direction::Modport)
                            }
                            SymbolKind::Instance(_) => true,
                            _ => false,
                        }
                    });
                    let r#type = if !via_interface {
                        match x.found.kind {
                            SymbolKind::Variable(x) => Some(x.r#type),
                            SymbolKind::Port(x) => x.r#type,
                            SymbolKind::Parameter(x) => Some(x.r#type),
                            SymbolKind::StructMember(x) => Some(x.r#type),
                            SymbolKind::UnionMember(x) => Some(x.r#type),
                            _ => None,
                        }
                    } else {
                        None
                    };

                    if let Some(x) = r#type {
                        let types = trace_type(&x, namespace);
                        let mut select_dimension = *self.select_dimension.last().unwrap();

                        let mut demension_number = None;
                        for (i, (t, k)) in types.iter().enumerate() {
                            if select_dimension < t.array.len() {
                                demension_number = Some(select_dimension + 1);
                                break;
                            }
                            select_dimension -= t.array.len();

                            if select_dimension < t.width.len() {
                                demension_number = Some(select_dimension + 1);
                                break;
                            }
                            select_dimension -= t.width.len();

                            if select_dimension == 0
                                && (i + 1) == types.len()
                                && matches!(
                                    k,
                                    Some(SymbolKind::Enum(_))
                                        | Some(SymbolKind::Struct(_))
                                        | Some(SymbolKind::Union(_))
                                )
                            {
                                demension_number = Some(0);
                                break;
                            }
                        }

                        if let Some(demension_number) = demension_number {
                            msb_table::insert(arg.msb_token.token.id, demension_number);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !resolved {
                    self.errors.push(AnalyzerError::unknown_msb(
                        self.text,
                        &arg.msb_token.token.into(),
                    ));
                }
            } else {
                self.errors.push(AnalyzerError::invalid_msb(
                    self.text,
                    &arg.msb_token.token.into(),
                ));
            }
        }
        Ok(())
    }

    fn identifier(&mut self, arg: &Identifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_expression_identifier {
                self.identifier_path
                    .last_mut()
                    .unwrap()
                    .0
                    .push(arg.identifier_token.token.text);
            }
        }
        Ok(())
    }

    fn select(&mut self, _arg: &Select) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_select = true;
            }
            HandlerPoint::After => {
                self.in_select = false;
                if self.in_expression_identifier {
                    *self.select_dimension.last_mut().unwrap() += 1;
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let namespace = namespace_table::get(arg.identifier().token.id).unwrap();
                let symbol_path = SymbolPath::default();
                self.identifier_path
                    .push(SymbolPathNamespace(symbol_path, namespace));
                self.select_dimension.push(0);
                self.in_expression_identifier = true;
            }
            HandlerPoint::After => {
                self.identifier_path.pop();
                self.select_dimension.pop();
                self.in_expression_identifier = false;
            }
        }
        Ok(())
    }
}
