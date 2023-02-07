use crate::allow_table;
use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckAttribute<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckAttribute<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl<'a> Handler for CheckAttribute<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckAttribute<'a> {
    fn attribute(&mut self, arg: &Attribute) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let identifier = arg.identifier.identifier_token.text();
            match identifier.as_str() {
                "ifdef" | "ifndef" => {
                    let valid_arg = if let Some(ref x) = arg.attribute_opt {
                        let args: Vec<AttributeItem> = x.attribute_list.as_ref().into();
                        if args.len() != 1 {
                            false
                        } else {
                            matches!(args[0], AttributeItem::Identifier(_))
                        }
                    } else {
                        false
                    };

                    if !valid_arg {
                        self.errors.push(AnalyzerError::mismatch_attribute_args(
                            &identifier,
                            "single identifier",
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
                "sv" => {
                    let valid_arg = if let Some(ref x) = arg.attribute_opt {
                        let args: Vec<AttributeItem> = x.attribute_list.as_ref().into();
                        if args.len() != 1 {
                            false
                        } else {
                            matches!(args[0], AttributeItem::StringLiteral(_))
                        }
                    } else {
                        false
                    };

                    if !valid_arg {
                        self.errors.push(AnalyzerError::mismatch_attribute_args(
                            &identifier,
                            "single string",
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
                "allow" => {
                    if let Some(ref x) = arg.attribute_opt {
                        let items: Vec<AttributeItem> = x.attribute_list.as_ref().into();
                        for x in &items {
                            match x {
                                AttributeItem::Identifier(x) => {
                                    allow_table::push(x.identifier.identifier_token.token.text);
                                }
                                AttributeItem::StringLiteral(_) => {
                                    self.errors.push(AnalyzerError::mismatch_attribute_args(
                                        &identifier,
                                        "identifiers",
                                        self.text,
                                        &arg.identifier.identifier_token,
                                    ));
                                }
                            }
                        }
                    } else {
                        self.errors.push(AnalyzerError::mismatch_attribute_args(
                            &identifier,
                            "identifiers",
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
                _ => {
                    self.errors.push(AnalyzerError::unknown_attribute(
                        &identifier,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }
            }
        }
        Ok(())
    }

    fn modport_group(&mut self, arg: &ModportGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.modport_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn enum_group(&mut self, arg: &EnumGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.enum_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn struct_group(&mut self, arg: &StructGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.struct_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn inst_parameter_group(&mut self, arg: &InstParameterGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.inst_parameter_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn inst_port_group(&mut self, arg: &InstPortGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.inst_port_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn with_parameter_group(&mut self, arg: &WithParameterGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.with_parameter_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn port_declaration_group(&mut self, arg: &PortDeclarationGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.port_declaration_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn module_group(&mut self, arg: &ModuleGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.module_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn interface_group(&mut self, arg: &InterfaceGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.interface_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn package_group(&mut self, arg: &PackageGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.package_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }

    fn description_group(&mut self, arg: &DescriptionGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            for x in &arg.description_group_list {
                let identifier = x.attribute.identifier.identifier_token.text();
                if identifier.as_str() == "allow" {
                    allow_table::pop();
                }
            }
        }
        Ok(())
    }
}
