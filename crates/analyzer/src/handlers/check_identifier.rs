use crate::analyzer_error::AnalyzerError;
use crate::symbol::Direction as SymDirection;
use crate::symbol_table::is_sv_keyword;
use veryl_metadata::{Case, Lint};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{is_anonymous_token, Token};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckIdentifier<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    lint_opt: &'a Lint,
    point: HandlerPoint,
    in_always_comb: bool,
    in_always_ff: bool,
    in_function: bool,
}

enum Kind {
    ClockDomain,
    Enum,
    Function,
    FunctionInout,
    FunctionInput,
    FunctionOutput,
    FunctionRef,
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
    Union,
    Var,
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
            in_function: false,
        }
    }

    fn check(&mut self, token: &Token, kind: Kind) {
        let opt = &self.lint_opt.naming;

        let prefix = match kind {
            Kind::ClockDomain => &opt.prefix_clock_domain,
            Kind::Enum => &opt.prefix_enum,
            Kind::Function => &opt.prefix_function,
            Kind::FunctionInout => &opt.prefix_function_inout,
            Kind::FunctionInput => &opt.prefix_function_input,
            Kind::FunctionOutput => &opt.prefix_function_output,
            Kind::FunctionRef => &opt.prefix_function_ref,
            Kind::Instance => &opt.prefix_instance,
            Kind::Interface => &opt.prefix_interface,
            Kind::Modport => &opt.prefix_modport,
            Kind::Module => &opt.prefix_module,
            Kind::Package => &opt.prefix_package,
            Kind::Parameter => &opt.prefix_parameter,
            Kind::PortInout => &opt.prefix_port_inout,
            Kind::PortInput => &opt.prefix_port_input,
            Kind::PortModport => &opt.prefix_port_modport,
            Kind::PortOutput => &opt.prefix_port_output,
            Kind::Reg => &opt.prefix_reg,
            Kind::Struct => &opt.prefix_struct,
            Kind::Union => &opt.prefix_union,
            Kind::Var => &opt.prefix_var,
            Kind::Wire => &opt.prefix_wire,
        };

        let suffix = match kind {
            Kind::ClockDomain => &opt.suffix_clock_domain,
            Kind::Enum => &opt.suffix_enum,
            Kind::Function => &opt.suffix_function,
            Kind::FunctionInout => &opt.suffix_function_inout,
            Kind::FunctionInput => &opt.suffix_function_input,
            Kind::FunctionOutput => &opt.suffix_function_output,
            Kind::FunctionRef => &opt.suffix_function_ref,
            Kind::Instance => &opt.suffix_instance,
            Kind::Interface => &opt.suffix_interface,
            Kind::Modport => &opt.suffix_modport,
            Kind::Module => &opt.suffix_module,
            Kind::Package => &opt.suffix_package,
            Kind::Parameter => &opt.suffix_parameter,
            Kind::PortInout => &opt.suffix_port_inout,
            Kind::PortInput => &opt.suffix_port_input,
            Kind::PortModport => &opt.suffix_port_modport,
            Kind::PortOutput => &opt.suffix_port_output,
            Kind::Reg => &opt.suffix_reg,
            Kind::Struct => &opt.suffix_struct,
            Kind::Union => &opt.suffix_union,
            Kind::Var => &opt.suffix_var,
            Kind::Wire => &opt.suffix_wire,
        };

        let case = match kind {
            Kind::ClockDomain => &opt.case_clock_domain,
            Kind::Enum => &opt.case_enum,
            Kind::Function => &opt.case_function,
            Kind::FunctionInout => &opt.case_function_inout,
            Kind::FunctionInput => &opt.case_function_input,
            Kind::FunctionOutput => &opt.case_function_output,
            Kind::FunctionRef => &opt.case_function_ref,
            Kind::Instance => &opt.case_instance,
            Kind::Interface => &opt.case_interface,
            Kind::Modport => &opt.case_modport,
            Kind::Module => &opt.case_module,
            Kind::Package => &opt.case_package,
            Kind::Parameter => &opt.case_parameter,
            Kind::PortInout => &opt.case_port_inout,
            Kind::PortInput => &opt.case_port_input,
            Kind::PortModport => &opt.case_port_modport,
            Kind::PortOutput => &opt.case_port_output,
            Kind::Reg => &opt.case_reg,
            Kind::Struct => &opt.case_struct,
            Kind::Union => &opt.case_union,
            Kind::Var => &opt.case_var,
            Kind::Wire => &opt.case_wire,
        };

        let re_required = match kind {
            Kind::ClockDomain => &opt.re_required_clock_domain,
            Kind::Enum => &opt.re_required_enum,
            Kind::Function => &opt.re_required_function,
            Kind::FunctionInout => &opt.re_required_function_inout,
            Kind::FunctionInput => &opt.re_required_function_input,
            Kind::FunctionOutput => &opt.re_required_function_output,
            Kind::FunctionRef => &opt.re_required_function_ref,
            Kind::Instance => &opt.re_required_instance,
            Kind::Interface => &opt.re_required_interface,
            Kind::Modport => &opt.re_required_modport,
            Kind::Module => &opt.re_required_module,
            Kind::Package => &opt.re_required_package,
            Kind::Parameter => &opt.re_required_parameter,
            Kind::PortInout => &opt.re_required_port_inout,
            Kind::PortInput => &opt.re_required_port_input,
            Kind::PortModport => &opt.re_required_port_modport,
            Kind::PortOutput => &opt.re_required_port_output,
            Kind::Reg => &opt.re_required_reg,
            Kind::Struct => &opt.re_required_struct,
            Kind::Union => &opt.re_required_union,
            Kind::Var => &opt.re_required_var,
            Kind::Wire => &opt.re_required_wire,
        };

        let re_forbidden = match kind {
            Kind::ClockDomain => &opt.re_forbidden_clock_domain,
            Kind::Enum => &opt.re_forbidden_enum,
            Kind::Function => &opt.re_forbidden_function,
            Kind::FunctionInout => &opt.re_forbidden_function_inout,
            Kind::FunctionInput => &opt.re_forbidden_function_input,
            Kind::FunctionOutput => &opt.re_forbidden_function_output,
            Kind::FunctionRef => &opt.re_forbidden_function_ref,
            Kind::Instance => &opt.re_forbidden_instance,
            Kind::Interface => &opt.re_forbidden_interface,
            Kind::Modport => &opt.re_forbidden_modport,
            Kind::Module => &opt.re_forbidden_module,
            Kind::Package => &opt.re_forbidden_package,
            Kind::Parameter => &opt.re_forbidden_parameter,
            Kind::PortInout => &opt.re_forbidden_port_inout,
            Kind::PortInput => &opt.re_forbidden_port_input,
            Kind::PortModport => &opt.re_forbidden_port_modport,
            Kind::PortOutput => &opt.re_forbidden_port_output,
            Kind::Reg => &opt.re_forbidden_reg,
            Kind::Struct => &opt.re_forbidden_struct,
            Kind::Union => &opt.re_forbidden_union,
            Kind::Var => &opt.re_forbidden_var,
            Kind::Wire => &opt.re_forbidden_wire,
        };

        let identifier = token.to_string();

        if !matches!(kind, Kind::ClockDomain) && is_anonymous_token(token) {
            self.errors.push(AnalyzerError::anonymous_identifier_usage(
                self.text,
                &token.into(),
            ));
        }

        if identifier.starts_with("__") {
            self.errors.push(AnalyzerError::reserved_identifier(
                &identifier,
                self.text,
                &token.into(),
            ));
        }

        if is_sv_keyword(&identifier) {
            self.errors.push(AnalyzerError::sv_keyword_usage(
                &identifier,
                self.text,
                &token.into(),
            ))
        }

        if let Some(x) = identifier.strip_prefix("r#") {
            if is_sv_keyword(x) {
                self.errors.push(AnalyzerError::sv_keyword_usage(
                    &identifier,
                    self.text,
                    &token.into(),
                ))
            }
        }

        if let Some(prefix) = prefix {
            if !identifier.starts_with(prefix) {
                self.errors.push(AnalyzerError::invalid_identifier(
                    &identifier,
                    &format!("prefix: {prefix}"),
                    self.text,
                    &token.into(),
                ));
            }
        }
        if let Some(suffix) = suffix {
            if !identifier.ends_with(suffix) {
                self.errors.push(AnalyzerError::invalid_identifier(
                    &identifier,
                    &format!("suffix: {suffix}"),
                    self.text,
                    &token.into(),
                ));
            }
        }
        if let Some(case) = case {
            let pass = match case {
                Case::Snake => is_snake_case(&identifier),
                Case::ScreamingSnake => is_screaming_snake_case(&identifier),
                Case::UpperCamel => is_upper_camel_case(&identifier),
                Case::LowerCamel => is_lower_camel_case(&identifier),
            };
            if !pass {
                self.errors.push(AnalyzerError::invalid_identifier(
                    &identifier,
                    &format!("case: {case}"),
                    self.text,
                    &token.into(),
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
                    &token.into(),
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
                    &token.into(),
                ));
            }
        }
    }
}

impl Handler for CheckIdentifier<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckIdentifier<'_> {
    fn clock_domain(&mut self, arg: &ClockDomain) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::ClockDomain);
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = arg.expression_identifier.identifier().token;
            if self.in_always_comb {
                self.check(&token, Kind::Wire);
            }
            if self.in_always_ff {
                self.check(&token, Kind::Reg);
            }
        }
        Ok(())
    }

    fn const_declaration(&mut self, arg: &ConstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Parameter);
        }
        Ok(())
    }

    fn modport_declaration(&mut self, arg: &ModportDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Modport);
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Enum);
        }
        Ok(())
    }

    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match &*arg.struct_union {
                StructUnion::Struct(_) => {
                    self.check(&arg.identifier.identifier_token.token, Kind::Struct);
                }
                StructUnion::Union(_) => {
                    self.check(&arg.identifier.identifier_token.token, Kind::Union);
                }
            }
            self.check(&arg.identifier.identifier_token.token, Kind::Struct);
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Instance);
        }
        Ok(())
    }

    fn with_parameter_item(&mut self, arg: &WithParameterItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Parameter);
        }
        Ok(())
    }

    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let direction = match &*arg.port_declaration_item_group {
                PortDeclarationItemGroup::PortTypeConcrete(x) => {
                    let x = x.port_type_concrete.as_ref();
                    let direction: SymDirection = x.direction.as_ref().into();
                    direction
                }
                PortDeclarationItemGroup::PortTypeAbstract(_) => SymDirection::Interface,
            };
            let kind = if self.in_function {
                match direction {
                    SymDirection::Inout => Some(Kind::FunctionInout),
                    SymDirection::Input => Some(Kind::FunctionInput),
                    SymDirection::Output => Some(Kind::FunctionOutput),
                    SymDirection::Ref => Some(Kind::FunctionRef),
                    _ => None,
                }
            } else {
                match direction {
                    SymDirection::Inout => Some(Kind::PortInout),
                    SymDirection::Input => Some(Kind::PortInput),
                    SymDirection::Modport => Some(Kind::PortModport),
                    SymDirection::Output => Some(Kind::PortOutput),
                    _ => None,
                }
            };

            if let Some(kind) = kind {
                self.check(&arg.identifier.identifier_token.token, kind);
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
        match self.point {
            HandlerPoint::Before => {
                self.check(&arg.identifier.identifier_token.token, Kind::Function);
                self.in_function = true;
            }
            HandlerPoint::After => self.in_function = false,
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Module);
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Interface);
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Package);
        }
        Ok(())
    }

    fn var_declaration(&mut self, arg: &VarDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.check(&arg.identifier.identifier_token.token, Kind::Var)
        }
        Ok(())
    }
}

fn is_lower_camel_case(text: &str) -> bool {
    let head = text.chars().next().unwrap();
    !text.contains('_') && head.is_ascii_lowercase()
}

fn is_upper_camel_case(text: &str) -> bool {
    let head = text.chars().next().unwrap();
    !text.contains('_') && head.is_ascii_uppercase()
}

fn is_snake_case(text: &str) -> bool {
    text.to_ascii_lowercase() == text
}

fn is_screaming_snake_case(text: &str) -> bool {
    text.to_ascii_uppercase() == text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_camel_case() {
        assert_eq!(is_lower_camel_case("abc"), true);
        assert_eq!(is_lower_camel_case("aBc"), true);
        assert_eq!(is_lower_camel_case("AbC"), false);
        assert_eq!(is_lower_camel_case("ABc"), false);
        assert_eq!(is_lower_camel_case("ABC"), false);
        assert_eq!(is_lower_camel_case("a_b_c"), false);
        assert_eq!(is_lower_camel_case("a_B_c"), false);
        assert_eq!(is_lower_camel_case("A_B_C"), false);
    }

    #[test]
    fn upper_camel_case() {
        assert_eq!(is_upper_camel_case("abc"), false);
        assert_eq!(is_upper_camel_case("aBc"), false);
        assert_eq!(is_upper_camel_case("AbC"), true);
        assert_eq!(is_upper_camel_case("ABc"), true);
        assert_eq!(is_upper_camel_case("ABC"), true);
        assert_eq!(is_upper_camel_case("a_b_c"), false);
        assert_eq!(is_upper_camel_case("a_B_c"), false);
        assert_eq!(is_upper_camel_case("A_B_C"), false);
    }

    #[test]
    fn snake_case() {
        assert_eq!(is_snake_case("abc"), true);
        assert_eq!(is_snake_case("aBc"), false);
        assert_eq!(is_snake_case("AbC"), false);
        assert_eq!(is_snake_case("ABc"), false);
        assert_eq!(is_snake_case("ABC"), false);
        assert_eq!(is_snake_case("a_b_c"), true);
        assert_eq!(is_snake_case("a_B_c"), false);
        assert_eq!(is_snake_case("A_B_C"), false);
    }

    #[test]
    fn screaming_snake_case() {
        assert_eq!(is_screaming_snake_case("abc"), false);
        assert_eq!(is_screaming_snake_case("aBc"), false);
        assert_eq!(is_screaming_snake_case("AbC"), false);
        assert_eq!(is_screaming_snake_case("ABc"), false);
        assert_eq!(is_screaming_snake_case("ABC"), true);
        assert_eq!(is_screaming_snake_case("a_b_c"), false);
        assert_eq!(is_screaming_snake_case("a_B_c"), false);
        assert_eq!(is_screaming_snake_case("A_B_C"), true);
    }
}
