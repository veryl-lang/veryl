use crate::analyzer_error::AnalyzerError;
use crate::attribute::Attribute as Attr;
use crate::attribute::AttributeError;
use crate::attribute_table;
use veryl_parser::ParolError;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::{TokenExt, TokenRange};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default, PartialEq, Eq)]
enum IfdefState {
    #[default]
    None,
    SingleIfdef,
    MultiIfdef,
    Elsif,
}

#[derive(Default)]
pub struct CheckAttribute {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    ifdef_state: IfdefState,
    ifdef_pos: Vec<StrId>,
    ifdef_neg: Vec<StrId>,
}

impl CheckAttribute {
    pub fn new() -> Self {
        Self::default()
    }

    fn gen_attrs(&mut self, args: &[&Attribute]) -> Vec<Option<(Attr, TokenRange)>> {
        let mut ret = Vec::new();

        for arg in args {
            let attr: Result<crate::attribute::Attribute, crate::attribute::AttributeError> =
                (*arg).try_into();
            match attr {
                Ok(attr) => {
                    ret.push(Some((attr, arg.range())));
                }
                Err(err) => {
                    ret.push(None);
                    match err {
                        AttributeError::UnknownAttribute => {
                            self.errors.push(AnalyzerError::unknown_attribute(
                                &arg.identifier.identifier_token.to_string(),
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                        AttributeError::MismatchArgs(x) => {
                            self.errors.push(AnalyzerError::mismatch_attribute_args(
                                &arg.identifier.identifier_token.to_string(),
                                x,
                                &arg.identifier.as_ref().into(),
                            ));
                        }
                    }
                }
            }
        }

        ret
    }

    fn set_attrs(&mut self, attrs: Vec<Option<(Attr, TokenRange)>>, range: TokenRange) {
        for attr in attrs {
            if let Some((attr, _)) = attr {
                attribute_table::begin(range.beg, Some(attr));
            } else {
                attribute_table::begin(range.beg, None);
            }
            attribute_table::end(range.end);
        }
    }

    fn check_ifdef(&mut self, attrs: &mut [Option<(Attr, TokenRange)>]) {
        let mut attrs: Vec<_> = attrs
            .iter_mut()
            .filter_map(|x| x.as_mut().filter(|x| x.0.is_ifdef()))
            .collect();

        #[allow(clippy::comparison_chain)]
        if attrs.len() > 1 {
            if attrs
                .iter()
                .all(|x| matches!(x.0, Attr::Ifdef(_) | Attr::Ifndef(_)))
            {
                self.ifdef_state = IfdefState::MultiIfdef;
            } else {
                let (_, range) = &attrs[0];
                self.errors.push(AnalyzerError::ambiguous_elsif(
                    "mixed ifdef/ifndef/elsif/else in the same position",
                    range,
                ));
            }
        } else if attrs.len() == 1 {
            let (attr, range) = &mut attrs[0];
            match attr {
                Attr::Ifdef(x) => {
                    self.ifdef_state = IfdefState::SingleIfdef;
                    self.ifdef_pos.clear();
                    self.ifdef_neg.clear();
                    self.ifdef_neg.push(*x);
                }
                Attr::Ifndef(x) => {
                    self.ifdef_state = IfdefState::SingleIfdef;
                    self.ifdef_pos.clear();
                    self.ifdef_neg.clear();
                    self.ifdef_pos.push(*x);
                }
                Attr::Elsif(x, _, _) => {
                    if matches!(
                        self.ifdef_state,
                        IfdefState::SingleIfdef | IfdefState::Elsif
                    ) {
                        self.ifdef_state = IfdefState::Elsif;
                        let x = *x;
                        *attr = Attr::Elsif(x, self.ifdef_pos.clone(), self.ifdef_neg.clone());
                        self.ifdef_neg.push(x);
                    } else {
                        let msg = if self.ifdef_state == IfdefState::MultiIfdef {
                            "elsif can't be used with multiple ifdef/ifndef"
                        } else {
                            "elsif should be placed just after ifdef/ifndef"
                        };
                        self.errors.push(AnalyzerError::ambiguous_elsif(msg, range));
                    }
                }
                Attr::Else(_, _) => {
                    if matches!(
                        self.ifdef_state,
                        IfdefState::SingleIfdef | IfdefState::Elsif
                    ) {
                        *attr = Attr::Else(self.ifdef_pos.clone(), self.ifdef_neg.clone());
                        self.ifdef_state = IfdefState::None;
                    } else {
                        // Error
                        let msg = if self.ifdef_state == IfdefState::MultiIfdef {
                            "else can't be used with multiple ifdef/ifndef"
                        } else {
                            "else should be placed just after ifdef/ifndef"
                        };
                        self.errors.push(AnalyzerError::ambiguous_elsif(msg, range));
                    }
                }
                _ => unreachable!(),
            }
        } else {
            self.ifdef_state = IfdefState::None;
        }
    }

    fn attrs(&mut self, args: &[&Attribute], range: TokenRange) {
        let mut attrs = self.gen_attrs(args);
        self.check_ifdef(&mut attrs);
        self.set_attrs(attrs, range);
    }
}

impl Handler for CheckAttribute {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckAttribute {
    fn statement_block(&mut self, arg: &StatementBlock) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.statement_block_list {
                let x = x.statement_block_group.as_ref();
                let attrs: Vec<_> = x
                    .statement_block_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn modport_list(&mut self, arg: &ModportList) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut groups = vec![arg.modport_group.as_ref()];
            groups.extend(
                arg.modport_list_list
                    .iter()
                    .map(|x| x.modport_group.as_ref()),
            );
            for x in &groups {
                let attrs: Vec<_> = x
                    .modport_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn enum_list(&mut self, arg: &EnumList) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut groups = vec![arg.enum_group.as_ref()];
            groups.extend(arg.enum_list_list.iter().map(|x| x.enum_group.as_ref()));
            for x in &groups {
                let attrs: Vec<_> = x
                    .enum_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn struct_union_list(&mut self, arg: &StructUnionList) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut groups = vec![arg.struct_union_group.as_ref()];
            groups.extend(
                arg.struct_union_list_list
                    .iter()
                    .map(|x| x.struct_union_group.as_ref()),
            );
            for x in &groups {
                let attrs: Vec<_> = x
                    .struct_union_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn inst_parameter_list(&mut self, arg: &InstParameterList) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut groups = vec![arg.inst_parameter_group.as_ref()];
            groups.extend(
                arg.inst_parameter_list_list
                    .iter()
                    .map(|x| x.inst_parameter_group.as_ref()),
            );
            for x in &groups {
                let attrs: Vec<_> = x
                    .inst_parameter_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn inst_port_list(&mut self, arg: &InstPortList) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut groups = vec![arg.inst_port_group.as_ref()];
            groups.extend(
                arg.inst_port_list_list
                    .iter()
                    .map(|x| x.inst_port_group.as_ref()),
            );
            for x in &groups {
                let attrs: Vec<_> = x
                    .inst_port_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn with_parameter_list(&mut self, arg: &WithParameterList) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut groups = vec![arg.with_parameter_group.as_ref()];
            groups.extend(
                arg.with_parameter_list_list
                    .iter()
                    .map(|x| x.with_parameter_group.as_ref()),
            );
            for x in &groups {
                let attrs: Vec<_> = x
                    .with_parameter_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn port_declaration_list(&mut self, arg: &PortDeclarationList) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut groups = vec![arg.port_declaration_group.as_ref()];
            groups.extend(
                arg.port_declaration_list_list
                    .iter()
                    .map(|x| x.port_declaration_group.as_ref()),
            );
            for x in &groups {
                let attrs: Vec<_> = x
                    .port_declaration_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.module_declaration_list {
                let x = x.module_group.as_ref();
                let attrs: Vec<_> = x
                    .module_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.interface_declaration_list {
                let x = x.interface_group.as_ref();
                let attrs: Vec<_> = x
                    .interface_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn generate_named_block(&mut self, arg: &GenerateNamedBlock) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.generate_named_block_list {
                let x = x.generate_group.as_ref();
                let attrs: Vec<_> = x
                    .generate_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn generate_optional_named_block(
        &mut self,
        arg: &GenerateOptionalNamedBlock,
    ) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.generate_optional_named_block_list {
                let x = x.generate_group.as_ref();
                let attrs: Vec<_> = x
                    .generate_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.package_declaration_list {
                let x = x.package_group.as_ref();
                let attrs: Vec<_> = x
                    .package_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }

    fn veryl(&mut self, arg: &Veryl) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.veryl_list {
                let x = x.description_group.as_ref();
                let attrs: Vec<_> = x
                    .description_group_list
                    .iter()
                    .map(|x| x.attribute.as_ref())
                    .collect();
                self.attrs(&attrs, x.range());
            }
        }
        Ok(())
    }
}
