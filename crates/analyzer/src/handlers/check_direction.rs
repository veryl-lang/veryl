use crate::analyzer_error::AnalyzerError;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckDirection<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    in_function: bool,
    in_module: bool,
    in_modport: bool,
    is_interface_port: bool,
}

impl<'a> CheckDirection<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckDirection<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn is_interface_type(arg: &ArrayType) -> bool {
    if let ScalarTypeGroup::VariableTypeScalarTypeOpt(x) = &*arg.scalar_type.scalar_type_group {
        if let VariableType::ScopedIdentifier(x) = x.variable_type.as_ref() {
            let symbol = symbol_table::resolve(x.scoped_identifier.as_ref()).unwrap();
            return matches!(symbol.found.kind, SymbolKind::Interface(_));
        }
    }

    false
}

impl<'a> VerylGrammarTrait for CheckDirection<'a> {
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let PortDeclarationItemGroup::PortTypeConcrete(x) =
                arg.port_declaration_item_group.as_ref()
            {
                let x = x.port_type_concrete.as_ref();
                let r#type = &x.array_type;

                self.is_interface_port = is_interface_type(r#type);
                if let Direction::Inout(_) = x.direction.as_ref() {
                    let is_tri = r#type
                        .scalar_type
                        .scalar_type_list
                        .iter()
                        .any(|x| matches!(x.type_modifier.as_ref(), TypeModifier::Tri(_)));

                    if !is_tri {
                        self.errors.push(AnalyzerError::missing_tri(
                            self.text,
                            &r#type.as_ref().into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn direction(&mut self, arg: &Direction) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match arg {
                Direction::Input(x) => {
                    if self.is_interface_port {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "input",
                            self.text,
                            &x.input.input_token.token.into(),
                        ));
                    }
                }
                Direction::Output(x) => {
                    if self.is_interface_port {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "output",
                            self.text,
                            &x.output.output_token.token.into(),
                        ));
                    }
                }
                Direction::Inout(x) => {
                    if self.is_interface_port {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "inout",
                            self.text,
                            &x.inout.inout_token.token.into(),
                        ));
                    }
                }
                Direction::Ref(x) => {
                    if !self.in_function || self.is_interface_port {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "ref",
                            self.text,
                            &x.r#ref.ref_token.token.into(),
                        ));
                    }
                }
                Direction::Modport(x) => {
                    if !self.in_module || self.in_function {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "modport",
                            self.text,
                            &x.modport.modport_token.token.into(),
                        ));
                    }
                }
                Direction::Import(x) => {
                    if !self.in_modport || self.is_interface_port {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "import",
                            self.text,
                            &x.import.import_token.token.into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, _arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_function = true,
            HandlerPoint::After => self.in_function = false,
        }
        Ok(())
    }

    fn module_declaration(&mut self, _arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_module = true,
            HandlerPoint::After => self.in_module = false,
        }
        Ok(())
    }

    fn modport_declaration(&mut self, _arg: &ModportDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_modport = true,
            HandlerPoint::After => self.in_modport = false,
        }
        Ok(())
    }
}
