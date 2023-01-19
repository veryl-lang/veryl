pub use crate::generated::veryl_grammar_trait::*;
use paste::paste;

macro_rules! list_to_item {
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

list_to_item!(Modport);
list_to_item!(Enum);
list_to_item!(Struct);
list_to_item!(InstParameter);
list_to_item!(InstPort);
list_to_item!(WithParameter);
list_to_item!(PortDeclaration);
group_to_item!(Module);
group_to_item!(Interface);
group_to_item!(Package);
group_to_item!(Description);
