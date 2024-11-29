use crate::analyzer_error::AnalyzerError;
use crate::attribute::AttributeError;
use crate::attribute_table;
use veryl_parser::last_token::LastToken;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};
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

impl Handler for CheckAttribute<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckAttribute<'_> {
    fn attribute(&mut self, arg: &Attribute) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let attr: Result<crate::attribute::Attribute, crate::attribute::AttributeError> =
                arg.try_into();

            match attr {
                Ok(attr) => {
                    attribute_table::begin(arg.hash.hash_token.token, Some(attr));
                }
                Err(err) => {
                    attribute_table::begin(arg.hash.hash_token.token, None);
                    match err {
                        AttributeError::UnknownAttribute => {
                            self.errors.push(AnalyzerError::unknown_attribute(
                                &arg.identifier.identifier_token.to_string(),
                                self.text,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                        AttributeError::MismatchArgs(x) => {
                            self.errors.push(AnalyzerError::mismatch_attribute_args(
                                &arg.identifier.identifier_token.to_string(),
                                x,
                                self.text,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                        AttributeError::InvalidAllow(x) => {
                            self.errors.push(AnalyzerError::invalid_allow(
                                &x.to_string(),
                                self.text,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                        AttributeError::InvalidEnumEncoding(x) => {
                            self.errors.push(AnalyzerError::invalid_enum_encoding(
                                &x.to_string(),
                                self.text,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                        AttributeError::InvalidCondType(x) => {
                            self.errors.push(AnalyzerError::invalid_cond_type(
                                &x.to_string(),
                                self.text,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn statement_block_group(&mut self, arg: &StatementBlockGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.statement_block_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.statement_block_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn modport_group(&mut self, arg: &ModportGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.modport_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.modport_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn enum_group(&mut self, arg: &EnumGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.enum_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.enum_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn struct_union_group(&mut self, arg: &StructUnionGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.struct_union_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.struct_union_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn inst_parameter_group(&mut self, arg: &InstParameterGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.inst_parameter_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.inst_parameter_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn inst_port_group(&mut self, arg: &InstPortGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.inst_port_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.inst_port_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn with_parameter_group(&mut self, arg: &WithParameterGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.with_parameter_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.with_parameter_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn port_declaration_group(&mut self, arg: &PortDeclarationGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.port_declaration_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.port_declaration_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn module_group(&mut self, arg: &ModuleGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.module_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.module_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn interface_group(&mut self, arg: &InterfaceGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.interface_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.interface_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn generate_group(&mut self, arg: &GenerateGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.generate_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.generate_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn package_group(&mut self, arg: &PackageGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.package_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.package_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }

    fn description_group(&mut self, arg: &DescriptionGroup) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let mut last_token = LastToken::default();
            last_token.description_group(arg);
            let last_token = last_token.token().unwrap();

            for _ in &arg.description_group_list {
                attribute_table::end(last_token);
            }
        }
        Ok(())
    }
}
