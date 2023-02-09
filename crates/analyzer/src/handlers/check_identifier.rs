use crate::analyzer_error::AnalyzerError;
use crate::symbol::Direction as SymDirection;
use inflector::cases::{
    camelcase::is_camel_case, pascalcase::is_pascal_case,
    screamingsnakecase::is_screaming_snake_case, snakecase::is_snake_case,
};
use veryl_metadata::{Case, Lint};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckIdentifier<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    lint_opt: &'a Lint,
    point: HandlerPoint,
    in_always_comb: bool,
    in_always_ff: bool,
}

enum Kind {
    Enum,
    Function,
    Instance,
    Interface,
    Modport,
    Module,
    Package,
    Parameter,
    PortInout,
    PortInput,
    PortModport,
    PortOutput,
    Reg,
    Struct,
    Wire,
}

impl<'a> CheckIdentifier<'a> {
    pub fn new(text: &'a str, lint_opt: &'a Lint) -> Self {
        Self {
            errors: Vec::new(),
            text,
            lint_opt,
            point: HandlerPoint::Before,
            in_always_comb: false,
            in_always_ff: false,
        }
    }

    fn check(&mut self, token: &VerylToken, kind: Kind) {
        let prefix = match kind {
            Kind::Enum => &self.lint_opt.prefix_enum,
            Kind::Function => &self.lint_opt.prefix_function,
            Kind::Instance => &self.lint_opt.prefix_instance,
            Kind::Interface => &self.lint_opt.prefix_interface,
            Kind::Modport => &self.lint_opt.prefix_modport,
            Kind::Module => &self.lint_opt.prefix_module,
            Kind::Package => &self.lint_opt.prefix_package,
            Kind::Parameter => &self.lint_opt.prefix_parameter,
            Kind::PortInout => &self.lint_opt.prefix_port_inout,
            Kind::PortInput => &self.lint_opt.prefix_port_input,
            Kind::PortModport => &self.lint_opt.prefix_port_modport,
            Kind::PortOutput => &self.lint_opt.prefix_port_output,
            Kind::Reg => &self.lint_opt.prefix_reg,
            Kind::Struct => &self.lint_opt.prefix_struct,
            Kind::Wire => &self.lint_opt.prefix_wire,
        };

        let case = match kind {
            Kind::Enum => &self.lint_opt.case_enum,
            Kind::Function => &self.lint_opt.case_function,
            Kind::Instance => &self.lint_opt.case_instance,
            Kind::Interface => &self.lint_opt.case_interface,
            Kind::Modport => &self.lint_opt.case_modport,
            Kind::Module => &self.lint_opt.case_module,
            Kind::Package => &self.lint_opt.case_package,
            Kind::Parameter => &self.lint_opt.case_parameter,
            Kind::PortInout => &self.lint_opt.case_port_inout,
            Kind::PortInput => &self.lint_opt.case_port_input,
            Kind::PortModport => &self.lint_opt.case_port_modport,
            Kind::PortOutput => &self.lint_opt.case_port_output,
            Kind::Reg => &self.lint_opt.case_reg,
            Kind::Struct => &self.lint_opt.case_struct,
            Kind::Wire => &self.lint_opt.case_wire,
        };

        let re_required = match kind {
            Kind::Enum => &self.lint_opt.re_required_enum,
            Kind::Function => &self.lint_opt.re_required_function,
            Kind::Instance => &self.lint_opt.re_required_instance,
            Kind::Interface => &self.lint_opt.re_required_interface,
            Kind::Modport => &self.lint_opt.re_required_modport,
            Kind::Module => &self.lint_opt.re_required_module,
            Kind::Package => &self.lint_opt.re_required_package,
            Kind::Parameter => &self.lint_opt.re_required_parameter,
            Kind::PortInout => &self.lint_opt.re_required_port_inout,
            Kind::PortInput => &self.lint_opt.re_required_port_input,
            Kind::PortModport => &self.lint_opt.re_required_port_modport,
            Kind::PortOutput => &self.lint_opt.re_required_port_output,
            Kind::Reg => &self.lint_opt.re_required_reg,
            Kind::Struct => &self.lint_opt.re_required_struct,
            Kind::Wire => &self.lint_opt.re_required_wire,
        };

        let re_forbidden = match kind {
            Kind::Enum => &self.lint_opt.re_forbidden_enum,
            Kind::Function => &self.lint_opt.re_forbidden_function,
            Kind::Instance => &self.lint_opt.re_forbidden_instance,
            Kind::Interface => &self.lint_opt.re_forbidden_interface,
            Kind::Modport => &self.lint_opt.re_forbidden_modport,
            Kind::Module => &self.lint_opt.re_forbidden_module,
            Kind::Package => &self.lint_opt.re_forbidden_package,
            Kind::Parameter => &self.lint_opt.re_forbidden_parameter,
            Kind::PortInout => &self.lint_opt.re_forbidden_port_inout,
            Kind::PortInput => &self.lint_opt.re_forbidden_port_input,
            Kind::PortModport => &self.lint_opt.re_forbidden_port_modport,
            Kind::PortOutput => &self.lint_opt.re_forbidden_port_output,
            Kind::Reg => &self.lint_opt.re_forbidden_reg,
            Kind::Struct => &self.lint_opt.re_forbidden_struct,
            Kind::Wire => &self.lint_opt.re_forbidden_wire,
        };

        let identifier = token.text();
        if let Some(prefix) = prefix {
            if !identifier.starts_with(prefix) {
                self.errors.push(AnalyzerError::invalid_identifier(
                    &identifier,
                    &format!("prefix: {prefix}"),
                    self.text,
                    token,
                ));
            }
        }
        if let Some(case) = case {
            let pass = match case {
                Case::Snake => is_snake_case(&identifier),
                Case::ScreamingSnake => is_screaming_snake_case(&identifier),
                Case::UpperCamel => is_pascal_case(&identifier),
                Case::LowerCamel => is_camel_case(&identifier),
            };
            if !pass {
                self.errors.push(AnalyzerError::invalid_identifier(
                    &identifier,
                    &format!("case: {case}"),
                    self.text,
                    token,
                ));
            }
        }
        if let Some(re_required) = re_required {
            let pass = if let Some(m) = re_required.find(&identifier) {
                m.start() == 0 && m.end() == identifier.len()
            } else {
                false
            };
            if !pass {
                self.errors.push(AnalyzerError::invalid_identifier(
                    &identifier,
                    &format!("re_required: {re_required}"),
                    self.text,
                    token,
                ));
            }
        }
        if let Some(re_forbidden) = re_forbidden {
            let fail = if let Some(m) = re_forbidden.find(&identifier) {
                m.start() == 0 && m.end() == identifier.len()
            } else {
                false
            };
            if fail {
                self.errors.push(AnalyzerError::invalid_identifier(
                    &identifier,
                    &format!("re_forbidden: {re_forbidden}"),
                    self.text,
                    token,
                ));
            }
        }
    }
}

impl<'a> Handler for CheckIdentifier<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckIdentifier<'a> {
    fn assignment_statement(&mut self, arg: &AssignmentStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_always_comb {
                self.check(
                    &arg.hierarchical_identifier.identifier.identifier_token,
                    Kind::Wire,
                );
            }
            if self.in_always_ff {
                self.check(
                    &arg.hierarchical_identifier.identifier.identifier_token,
                    Kind::Reg,
                );
            }
        }
        Ok(())
    }

    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Parameter);
        }
        Ok(())
    }

    fn modport_declaration(&mut self, arg: &ModportDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Modport);
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Enum);
        }
        Ok(())
    }

    fn struct_declaration(&mut self, arg: &StructDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Struct);
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Instance);
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Parameter);
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let direction = match &*arg.port_declaration_item_group {
                PortDeclarationItemGroup::DirectionArrayType(x) => {
                    let direction: SymDirection = x.direction.as_ref().into();
                    direction
                }
                PortDeclarationItemGroup::InterfacePortDeclarationItemOpt(_) => {
                    SymDirection::Interface
                }
            };
            let kind = match direction {
                SymDirection::Input => Some(Kind::PortInput),
                SymDirection::Output => Some(Kind::PortOutput),
                SymDirection::Inout => Some(Kind::PortInout),
                SymDirection::Modport => Some(Kind::PortModport),
                _ => None,
            };

            if let Some(kind) = kind {
                self.check(&arg.identifier.identifier_token, kind);
            }
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, _arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_always_ff = true,
            HandlerPoint::After => self.in_always_ff = false,
        }
        Ok(())
    }

    fn always_comb_declaration(&mut self, _arg: &AlwaysCombDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_always_comb = true,
            HandlerPoint::After => self.in_always_comb = false,
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Function);
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Module);
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Interface);
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token, Kind::Package);
        }
        Ok(())
    }
}
