pub use crate::generated::veryl_grammar_trait::*;
use paste::paste;

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
