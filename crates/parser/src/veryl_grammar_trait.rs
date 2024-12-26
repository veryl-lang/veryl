pub use crate::generated::veryl_grammar_trait::*;
use crate::veryl_token::is_anonymous_token;
use paste::paste;
use std::fmt;

macro_rules! list_group_to_item {
    ($x:ident) => {
        paste! {
            impl From<&[<$x List>]> for Vec<[<$x Item>]> {
                fn from(x: &[<$x List>]) -> Self {
                    let mut ret = Vec::new();
                    {
                        let mut x: Vec<[<$x Item>]> = x.[<$x:snake _group>].as_ref().into();
                        ret.append(&mut x);
                    }
                    for x in &x.[<$x:snake _list_list>] {
                        let mut x: Vec<[<$x Item>]> = x.[<$x:snake _group>].as_ref().into();
                        ret.append(&mut x);
                    }
                    ret
                }
            }

            impl From<&[<$x Group>]> for Vec<[<$x Item>]> {
                fn from(x: &[<$x Group>]) -> Self {
                    let mut ret = Vec::new();
                    match &*x.[<$x:snake _group_group>] {
                        [<$x GroupGroup>]::[<LBrace $x ListRBrace>](x) => {
                            let mut x: Vec<[<$x Item>]> = x.[<$x:snake _list>].as_ref().into();
                            ret.append(&mut x);
                        }
                        [<$x GroupGroup>]::[<$x Item>](x) => {
                            ret.push(x.[<$x:snake _item>].as_ref().clone());
                        }
                    }
                    ret
                }
            }
        }
    };
}

macro_rules! list_to_item {
    ($x:ident) => {
        paste! {
            impl From<&[<$x List>]> for Vec<[<$x Item>]> {
                fn from(x: &[<$x List>]) -> Self {
                    let mut ret = Vec::new();
                    {
                        ret.push(x.[<$x:snake _item>].as_ref().clone());
                    }
                    for x in &x.[<$x:snake _list_list>] {
                        ret.push(x.[<$x:snake _item>].as_ref().clone());
                    }
                    ret
                }
            }
        }
    };
}

macro_rules! group_to_item {
    ($x:ident) => {
        paste! {
            impl From<&[<$x Group>]> for Vec<[<$x Item>]> {
                fn from(x: &[<$x Group>]) -> Self {
                    let mut ret = Vec::new();
                    match &*x.[<$x:snake _group_group>] {
                        [<$x GroupGroup>]::[<LBrace $x GroupGroupListRBrace>](x) => {
                            for x in &x.[<$x:snake _group_group_list>] {
                                let mut x: Vec<[<$x Item>]> = x.[<$x:snake _group>].as_ref().into();
                                ret.append(&mut x);
                            }
                        }
                        [<$x GroupGroup>]::[<$x Item>](x) => {
                            ret.push(x.[<$x:snake _item>].as_ref().clone());
                        }
                    }
                    ret
                }
            }
        }
    };
}

impl From<&ScopedIdentifier> for Vec<Option<WithGenericArgument>> {
    fn from(value: &ScopedIdentifier) -> Self {
        let mut ret = Vec::new();
        match value.scoped_identifier_group.as_ref() {
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(ref x) => {
                if let Some(ref x) = x.scoped_identifier_opt {
                    ret.push(Some(x.with_generic_argument.as_ref().clone()));
                } else {
                    ret.push(None);
                }
            }
            ScopedIdentifierGroup::DollarIdentifier(_) => {
                ret.push(None);
            }
        }
        for x in &value.scoped_identifier_list {
            if let Some(ref x) = x.scoped_identifier_opt0 {
                ret.push(Some(x.with_generic_argument.as_ref().clone()));
            } else {
                ret.push(None);
            }
        }
        ret
    }
}

list_group_to_item!(Modport);
list_group_to_item!(Enum);
list_group_to_item!(StructUnion);
list_group_to_item!(InstParameter);
list_group_to_item!(InstPort);
list_group_to_item!(WithParameter);
list_group_to_item!(PortDeclaration);
list_to_item!(WithGenericParameter);
list_to_item!(WithGenericArgument);
list_to_item!(Attribute);
group_to_item!(Module);
group_to_item!(Interface);
group_to_item!(Generate);
group_to_item!(Package);
group_to_item!(Description);
group_to_item!(StatementBlock);

pub fn is_anonymous_expression(arg: &Expression) -> bool {
    if !arg.expression_list.is_empty() {
        return false;
    }

    let exp = &*arg.expression01;
    if !exp.expression01_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression02;
    if !exp.expression02_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression03;
    if !exp.expression03_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression04;
    if !exp.expression04_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression05;
    if !exp.expression05_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression06;
    if !exp.expression06_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression07;
    if !exp.expression07_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression08;
    if !exp.expression08_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression09;
    if !exp.expression09_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression10;
    if !exp.expression10_list.is_empty() {
        return false;
    }

    let exp = &*exp.expression11;
    if exp.expression11_opt.is_some() {
        return false;
    }

    let exp = &*exp.expression12;
    if !exp.expression12_list.is_empty() {
        return false;
    }

    match &*exp.factor {
        Factor::IdentifierFactor(x) => {
            let factor = &x.identifier_factor;

            if factor.identifier_factor_opt.is_some() {
                return false;
            }

            let exp_identifier = &*factor.expression_identifier;
            if exp_identifier.expression_identifier_opt.is_some() {
                return false;
            }
            if !exp_identifier.expression_identifier_list.is_empty() {
                return false;
            }
            if !exp_identifier.expression_identifier_list0.is_empty() {
                return false;
            }

            let scoped_identifier = &*exp_identifier.scoped_identifier;
            if !scoped_identifier.scoped_identifier_list.is_empty() {
                return false;
            }

            let token = scoped_identifier.identifier().token;
            is_anonymous_token(&token)
        }
        _ => false,
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let token = match self {
            Direction::Input(x) => &x.input.input_token,
            Direction::Output(x) => &x.output.output_token,
            Direction::Inout(x) => &x.inout.inout_token,
            Direction::Ref(x) => &x.r#ref.ref_token,
            Direction::Modport(x) => &x.modport.modport_token,
            Direction::Import(x) => &x.import.import_token,
        };
        token.fmt(f)
    }
}
