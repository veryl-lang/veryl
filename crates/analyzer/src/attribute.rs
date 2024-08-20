use std::cell::RefCell;
use std::fmt;
use veryl_parser::resource_table::{self, PathId, StrId};
use veryl_parser::veryl_token::TokenSource;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Attribute {
    Ifdef(StrId),
    Ifndef(StrId),
    Sv(StrId),
    Allow(AllowItem),
    EnumMemberPrefix(StrId),
    Test(StrId, PathId),
}

impl fmt::Display for Attribute {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Attribute::Ifdef(x) => format!("ifdef({})", x),
            Attribute::Ifndef(x) => format!("ifndef({})", x),
            Attribute::Sv(x) => format!("sv(\"{}\")", x),
            Attribute::Allow(x) => format!("allow({})", x),
            Attribute::EnumMemberPrefix(x) => format!("enum_member_prefix({})", x),
            Attribute::Test(x, _) => format!("test({})", x),
        };
        text.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub enum AttributeError {
    UnknownAttribute,
    MismatchArgs(&'static str),
    InvalidAllow(StrId),
}

fn get_arg_ident(args: &Option<veryl_parser::veryl_grammar_trait::AttributeOpt>) -> Option<StrId> {
    use veryl_parser::veryl_grammar_trait as g;

    if let Some(ref x) = args {
        let args: Vec<g::AttributeItem> = x.attribute_list.as_ref().into();
        if args.len() != 1 {
            None
        } else if let g::AttributeItem::Identifier(ref x) = args[0] {
            Some(x.identifier.identifier_token.token.text)
        } else {
            None
        }
    } else {
        None
    }
}

fn get_arg_string(args: &Option<veryl_parser::veryl_grammar_trait::AttributeOpt>) -> Option<StrId> {
    use veryl_parser::veryl_grammar_trait as g;

    if let Some(ref x) = args {
        let args: Vec<g::AttributeItem> = x.attribute_list.as_ref().into();
        if args.len() != 1 {
            None
        } else if let g::AttributeItem::StringLiteral(ref x) = args[0] {
            Some(x.string_literal.string_literal_token.token.text)
        } else {
            None
        }
    } else {
        None
    }
}

struct Pattern {
    pub ifdef: StrId,
    pub ifndef: StrId,
    pub sv: StrId,
    pub allow: StrId,
    pub missing_port: StrId,
    pub missing_reset_statement: StrId,
    pub unused_variable: StrId,
    pub enum_member_prefix: StrId,
    pub test: StrId,
}

impl Pattern {
    fn new() -> Self {
        Self {
            ifdef: resource_table::insert_str("ifdef"),
            ifndef: resource_table::insert_str("ifndef"),
            sv: resource_table::insert_str("sv"),
            allow: resource_table::insert_str("allow"),
            missing_port: resource_table::insert_str("missing_port"),
            missing_reset_statement: resource_table::insert_str("missing_reset_statement"),
            unused_variable: resource_table::insert_str("unused_variable"),
            enum_member_prefix: resource_table::insert_str("enum_member_prefix"),
            test: resource_table::insert_str("test"),
        }
    }
}

thread_local!(static PAT: RefCell<Pattern> = RefCell::new(Pattern::new()));

impl TryFrom<&veryl_parser::veryl_grammar_trait::Attribute> for Attribute {
    type Error = AttributeError;

    fn try_from(value: &veryl_parser::veryl_grammar_trait::Attribute) -> Result<Self, Self::Error> {
        PAT.with_borrow(|pat| match value.identifier.identifier_token.token.text {
            x if x == pat.ifdef || x == pat.ifndef => {
                let arg = get_arg_ident(&value.attribute_opt);

                if let Some(arg) = arg {
                    if x == pat.ifdef {
                        Ok(Attribute::Ifdef(arg))
                    } else {
                        Ok(Attribute::Ifndef(arg))
                    }
                } else {
                    Err(AttributeError::MismatchArgs("single identifier"))
                }
            }
            x if x == pat.sv => {
                let arg = get_arg_string(&value.attribute_opt);

                if let Some(arg) = arg {
                    Ok(Attribute::Sv(arg))
                } else {
                    Err(AttributeError::MismatchArgs("single string"))
                }
            }
            x if x == pat.allow => {
                let arg = get_arg_ident(&value.attribute_opt);

                if let Some(arg) = arg {
                    match arg {
                        x if x == pat.missing_port => Ok(Attribute::Allow(AllowItem::MissingPort)),
                        x if x == pat.missing_reset_statement => {
                            Ok(Attribute::Allow(AllowItem::MissingResetStatement))
                        }
                        x if x == pat.unused_variable => {
                            Ok(Attribute::Allow(AllowItem::UnusedVariable))
                        }
                        _ => Err(AttributeError::InvalidAllow(arg)),
                    }
                } else {
                    Err(AttributeError::MismatchArgs("allowable rule"))
                }
            }
            x if x == pat.enum_member_prefix => {
                let arg = get_arg_ident(&value.attribute_opt);

                if let Some(arg) = arg {
                    Ok(Attribute::EnumMemberPrefix(arg))
                } else {
                    Err(AttributeError::MismatchArgs("single identifier"))
                }
            }
            x if x == pat.test => {
                let arg = get_arg_ident(&value.attribute_opt);
                let path =
                    if let TokenSource::File(x) = value.identifier.identifier_token.token.source {
                        x
                    } else {
                        unreachable!();
                    };

                if let Some(arg) = arg {
                    Ok(Attribute::Test(arg, path))
                } else {
                    Err(AttributeError::MismatchArgs("single identifier"))
                }
            }
            _ => Err(AttributeError::UnknownAttribute),
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AllowItem {
    MissingPort,
    MissingResetStatement,
    UnusedVariable,
}

impl fmt::Display for AllowItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            AllowItem::MissingPort => "missing_port",
            AllowItem::MissingResetStatement => "missing_reset_statement",
            AllowItem::UnusedVariable => "unused_variable",
        };
        text.fmt(f)
    }
}
